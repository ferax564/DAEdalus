//! Dynamic state selection.
//!
//! The dummy-derivative method picks, per block of differentiated constraints, which
//! derivative variables the constraints are solved for (the dummies) and which stay state
//! derivatives. The backend picks once, at the start values. When the constraint Jacobian
//! depends on the solution, that choice can become singular during a run: a body whose
//! rotation is constrained to one axis, with `R.T[3,1] = sin(phi)` as its state, loses
//! `phi` at `phi = pi/2`, where `R.T[1,1] = cos(phi)` is the state to have (Mattsson and
//! Söderlind; Dymola switches such state sets at run time).
//!
//! Here a [`Monitor`] watches these sets after every accepted step. For a set with
//! constraint Jacobian `H` (equations by candidates) it takes an orthonormal basis `N`
//! of the null space of `H` (columns scaled to unit length): the rows of `N` belonging to the states
//! must form a well-conditioned matrix, as `|det N_S|` is proportional to `|det H_D|`,
//! the determinant of the block solved for the dummies. When the current choice has
//! become [`SWITCH`] times worse conditioned than it was when the run started with it,
//! and another choice of states (keeping `stateSelect = always` states, no weaker
//! `stateSelect` than the current ones, picked by maximum-volume pivoting on the rows of
//! `N`) is [`SWITCH`] times better, the run stops at that point, the model
//! is compiled again with the new dummies, and the run continues from the values
//! reached ([`crate::sim::simulate_switching`]). A model whose sets all have constant
//! Jacobians gets no monitor and runs exactly as before.

use crate::backend::{self, System, eval};
use crate::codegen::Model;
use crate::expr::{Arena, ExprId, Node, VarId};
use crate::flat::{FlatModel, VarKind};
use crate::results::Results;
use crate::sim::{SimError, SimOptions, SimStats, StateMonitor, StateSwitch};
use rustc_hash::FxHashMap;

/// A choice is switched when it degraded this much and another one is this much better
/// (`|det N_S|`): hysteresis against switching back and forth.
pub const SWITCH: f64 = 10.0;

/// Largest set watched (the null space is computed densely).
const MAX_SET: usize = 64;

/// The state sets of a compiled model whose constraint Jacobian depends on the solution.
pub struct Monitor {
    arena: Arena,
    sets: Vec<Set>,
}

struct Set {
    /// Derivative level (1 for the highest derivatives).
    level: usize,
    /// `jac[row][candidate]`: derivative of the residual by the candidate.
    jac: Vec<Vec<ExprId>>,
    /// Names of the candidates (the derivative variables).
    names: Vec<String>,
    /// Names of the variables the candidates are derivatives of (for messages).
    bases: Vec<String>,
    /// `stateSelect` class of each candidate (lower is more wanted as a state).
    class: Vec<i64>,
    dummy: Vec<bool>,
    /// Conditioning of the choice when the run started with it (see [`sigma_min`]).
    sigma0: Option<f64>,
}

impl Monitor {
    /// The monitor of a compiled system, or `None` when no state set with a choice has a
    /// solution-dependent Jacobian.
    pub fn new(sys: &System) -> Option<Monitor> {
        let mut arena: Option<Arena> = None;
        let mut sets = Vec::new();
        let base_of: FxHashMap<VarId, VarId> = sys
            .der_of
            .iter()
            .enumerate()
            .filter_map(|(v, d)| d.map(|d| (d, v as VarId)))
            .collect();
        for s in &sys.state_sets {
            if s.eqs.len() > MAX_SET || s.candidates.len() > 2 * MAX_SET {
                continue;
            }
            let a = arena.get_or_insert_with(|| sys.arena.clone());
            let jac: Vec<Vec<ExprId>> = s
                .eqs
                .iter()
                .map(|&(l, r)| {
                    let res = a.sub(l, r);
                    s.candidates
                        .iter()
                        .map(|&z| a.diff(res, Node::Var(z), &mut FxHashMap::default()))
                        .collect()
                })
                .collect();
            let dependent = jac.iter().flatten().any(|&d| {
                a.vars_in(&[d]).iter().any(|&v| {
                    matches!(
                        sys.vars[v as usize].kind,
                        VarKind::Continuous | VarKind::Discrete
                    )
                })
            });
            if !dependent {
                continue;
            }
            // Only constraints whose Jacobian depends on lower-order variables (positions
            // for a set of accelerations, `G_q(q) der(q, 2) = ..`) can become singular
            // with a choice that was fine: a Jacobian in the set's own bases (the currents
            // multiplying `der(gamma)` in a Park transform) mixes scales that vanish at
            // rest, and its conditioning says nothing about solvability.
            let bases: Vec<VarId> = s.candidates.iter().filter_map(|z| base_of.get(z).copied()).collect();
            if jac.iter().flatten().any(|&d| a.vars_in(&[d]).iter().any(|v| bases.contains(v))) {
                continue;
            }
            let name = |v: VarId| sys.vars[v as usize].name.clone();
            sets.push(Set {
                level: s.level,
                jac,
                names: s.candidates.iter().map(|&z| name(z)).collect(),
                bases: s
                    .candidates
                    .iter()
                    .map(|&z| base_of.get(&z).map_or_else(|| name(z), |&b| name(b)))
                    .collect(),
                class: s.tiers.iter().map(|t| t / 4).collect(),
                dummy: s.candidates.iter().map(|z| s.dummies.contains(z)).collect(),
                sigma0: None,
            });
        }
        if sets.is_empty() {
            return None;
        }
        Some(Monitor {
            arena: arena.unwrap(),
            sets,
        })
    }

    /// The new states of set `i` at the point `r`, when they are [`SWITCH`] times better
    /// than the current ones.
    fn better(&mut self, i: usize, r: &[f64], t: f64) -> Option<Vec<usize>> {
        let s = &self.sets[i];
        let vals = eval::Values {
            var: &|v| r[v as usize],
            pre: &|v| r[v as usize],
            time: t,
            initial: false,
            rel: None,
        };
        let k = s.jac.len();
        let n = s.names.len();
        let mut h: Vec<Vec<f64>> = s
            .jac
            .iter()
            .map(|row| {
                let mut memo = FxHashMap::default();
                row.iter()
                    .map(|&d| eval::eval(&self.arena, d, &vals, &mut memo))
                    .collect()
            })
            .collect();
        if h.iter().flatten().any(|x| !x.is_finite()) {
            return None;
        }
        // Scale the columns to unit length: a candidate whose coefficients grow with the
        // solution (`der(gamma)` times the currents of a Park transform) must not look
        // like a degrading choice, nor the choice depend on the variables' units.
        for c in 0..n {
            let norm = (0..k).map(|r| h[r][c] * h[r][c]).sum::<f64>().sqrt();
            if norm > 0.0 {
                for row in h.iter_mut() {
                    row[c] /= norm;
                }
            }
        }
        let null = null_basis(&h, n)?;
        let states: Vec<usize> = (0..n).filter(|&c| !s.dummy[c]).collect();
        if states.len() != n - k {
            return None;
        }
        let rows = |sel: &[usize]| -> Vec<Vec<f64>> {
            sel.iter().map(|&c| null[c].clone()).collect()
        };
        // How strongly the dummies respond to the states: `|H_D^-1 H_S| ~ 1 / sigma`.
        // A choice is kept while it stays about as well-conditioned as when the run
        // started with it (the static choice may be poorly scaled from the start without
        // being singular): only one that degraded is switched.
        let sigma = sigma_min(rows(&states));
        let sigma0 = *self.sets[i].sigma0.get_or_insert(sigma);
        if sigma >= sigma0 / SWITCH {
            return None;
        }
        let s = &self.sets[i];
        let current = det_abs(rows(&states));
        // `stateSelect = always` states stay states; the others may be replaced by
        // candidates whose stateSelect is no weaker.
        let worst = states.iter().map(|&c| s.class[c]).max()?;
        let forced: Vec<usize> = states.iter().copied().filter(|&c| s.class[c] == 0).collect();
        let allowed: Vec<usize> = (0..n)
            .filter(|&c| s.class[c] <= worst && !forced.contains(&c))
            .collect();
        let (mut pick, best) = max_volume(&null, &forced, &allowed, n - k);
        if pick.len() == n - k && best > 1e-6 && best > SWITCH * current {
            pick.sort_unstable();
            Some(pick)
        } else {
            None
        }
    }
}

impl StateMonitor for Monitor {
    fn check(&mut self, r: &[f64], t: f64) -> Option<StateSwitch> {
        let mut switched = Vec::new();
        for i in 0..self.sets.len() {
            if let Some(p) = self.better(i, r, t) {
                switched.push((i, p));
            }
        }
        if switched.is_empty() {
            return None;
        }
        // The candidates of a lower-order set are the bases of the dummies of the set
        // above it: switch the highest-order sets first; the sets below are picked again
        // at this point when the model is compiled again.
        let level = switched.iter().map(|(i, _)| self.sets[*i].level).min()?;
        switched.retain(|(i, _)| self.sets[*i].level == level);
        let mut msg = format!("dynamic state selection at t = {t}:");
        let mut choice = Vec::new();
        let mut sets = Vec::new();
        for (i, s) in self.sets.iter().enumerate() {
            if s.level > level {
                continue;
            }
            let dummy: Vec<bool> = match switched.iter().find(|(j, _)| *j == i) {
                Some((_, pick)) => {
                    let old: Vec<&str> = (0..s.names.len())
                        .filter(|&c| !s.dummy[c])
                        .map(|c| s.bases[c].as_str())
                        .collect();
                    let new: Vec<&str> = pick.iter().map(|&c| s.bases[c].as_str()).collect();
                    msg.push_str(&format!(
                        " states [{}] -> [{}]",
                        old.join(", "),
                        new.join(", ")
                    ));
                    sets.push(choice.len());
                    (0..s.names.len()).map(|c| !pick.contains(&c)).collect()
                }
                None => s.dummy.clone(),
            };
            choice.push(
                (0..s.names.len())
                    .filter(|&c| dummy[c])
                    .map(|c| s.names[c].clone())
                    .collect(),
            );
        }
        Some(StateSwitch {
            choice,
            switched: sets,
            message: msg,
        })
    }
}

/// Orthonormal basis of the null space of `h` (`k` rows, `n` columns, rank `k`), as
/// `n` rows of `n - k` entries; `None` when `h` is rank deficient.
fn null_basis(h: &[Vec<f64>], n: usize) -> Option<Vec<Vec<f64>>> {
    let k = h.len();
    if k >= n {
        return None;
    }
    // Householder QR of h^T (n x k): Q^T h^T = R; the last n - k rows of Q^T span the
    // null space of h.
    let mut a: Vec<Vec<f64>> = (0..n).map(|i| (0..k).map(|j| h[j][i]).collect()).collect();
    let mut qt: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    let scale = h.iter().flatten().fold(0.0f64, |m, x| m.max(x.abs()));
    if scale == 0.0 {
        return None;
    }
    for j in 0..k {
        let norm = (j..n).map(|i| a[i][j] * a[i][j]).sum::<f64>().sqrt();
        if norm <= 1e-10 * scale {
            return None;
        }
        let alpha = if a[j][j] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (j..n).map(|i| a[i][j]).collect();
        v[0] -= alpha;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        if vv == 0.0 {
            continue;
        }
        for c in j..k {
            let d: f64 = (j..n).map(|i| v[i - j] * a[i][c]).sum::<f64>() * 2.0 / vv;
            for i in j..n {
                a[i][c] -= d * v[i - j];
            }
        }
        for c in 0..n {
            let d: f64 = (j..n).map(|i| v[i - j] * qt[i][c]).sum::<f64>() * 2.0 / vv;
            for i in j..n {
                qt[i][c] -= d * v[i - j];
            }
        }
    }
    Some((0..n).map(|c| (k..n).map(|r| qt[r][c]).collect()).collect())
}

/// Smallest singular value of a square matrix, estimated as `1 / |A^-1|_F` (0 when
/// singular): how strongly the dummies respond to the states, `|H_D^-1 H_S| ~ 1 / sigma`.
fn sigma_min(mut a: Vec<Vec<f64>>) -> f64 {
    let m = a.len();
    let mut inv: Vec<Vec<f64>> = (0..m)
        .map(|i| (0..m).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for c in 0..m {
        let p = (c..m)
            .max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))
            .unwrap();
        if a[p][c] == 0.0 {
            return 0.0;
        }
        a.swap(c, p);
        inv.swap(c, p);
        let d = a[c][c];
        for x in 0..m {
            a[c][x] /= d;
            inv[c][x] /= d;
        }
        for r in 0..m {
            if r != c {
                let f = a[r][c];
                if f != 0.0 {
                    for x in 0..m {
                        a[r][x] -= f * a[c][x];
                        inv[r][x] -= f * inv[c][x];
                    }
                }
            }
        }
    }
    let f = inv.iter().flatten().map(|x| x * x).sum::<f64>().sqrt();
    if f.is_finite() && f > 0.0 { 1.0 / f } else { 0.0 }
}

/// `|det|` of a square matrix (Gaussian elimination with partial pivoting).
fn det_abs(mut a: Vec<Vec<f64>>) -> f64 {
    let m = a.len();
    let mut det = 1.0;
    for c in 0..m {
        let p = (c..m)
            .max_by(|&x, &y| a[x][c].abs().total_cmp(&a[y][c].abs()))
            .unwrap();
        if a[p][c] == 0.0 {
            return 0.0;
        }
        a.swap(c, p);
        det *= a[c][c].abs();
        for r in c + 1..m {
            let f = a[r][c] / a[c][c];
            for x in c..m {
                a[r][x] -= f * a[c][x];
            }
        }
    }
    det
}

/// The `forced` rows of `null` and `m - forced.len()` of the `allowed` ones, together
/// spanning a large volume (Gaussian elimination with complete pivoting, the forced rows
/// first), and the `|det|` of those `m` rows.
fn max_volume(null: &[Vec<f64>], forced: &[usize], allowed: &[usize], m: usize) -> (Vec<usize>, f64) {
    let rows: Vec<usize> = forced.iter().chain(allowed).copied().collect();
    let mut a: Vec<Vec<f64>> = rows.iter().map(|&r| null[r].clone()).collect();
    let mut used_row = vec![false; a.len()];
    let mut used_col = vec![false; m];
    let mut pick = Vec::new();
    let mut det = 1.0;
    for step in 0..m {
        let mut best = (usize::MAX, usize::MAX, 0.0f64);
        for (r, row) in a.iter().enumerate() {
            if used_row[r] || (step < forced.len()) != (r < forced.len()) {
                continue;
            }
            for (c, &x) in row.iter().enumerate() {
                if !used_col[c] && x.abs() > best.2 {
                    best = (r, c, x.abs());
                }
            }
        }
        let (pr, pc, pv) = best;
        if pr == usize::MAX {
            return (pick, 0.0);
        }
        used_row[pr] = true;
        used_col[pc] = true;
        pick.push(rows[pr]);
        det *= pv;
        let prow = a[pr].clone();
        for (r, row) in a.iter_mut().enumerate() {
            if used_row[r] {
                continue;
            }
            let f = row[pc] / prow[pc];
            for c in 0..m {
                row[c] -= f * prow[c];
            }
        }
    }
    (pick, det)
}

/// What a run with dynamic state selection needs to compile the model again.
pub struct Dynamic {
    flat: FlatModel,
    opts: backend::Options,
    monitor: Monitor,
}

/// [`crate::translate`], also returning what dynamic state selection needs when the
/// model has state sets whose best choice depends on the solution (and no FMU
/// components or delays, whose histories a recompiled model could not take over).
pub fn translate(
    lib: &crate::frontend::Library,
    model: &str,
    opts: &backend::Options,
) -> anyhow::Result<(Model, Option<Dynamic>)> {
    let flat = lib.flatten(model)?;
    let keep = flat.clone();
    let sys = backend::compile(flat, opts)?;
    let monitor = Monitor::new(&sys);
    let m = crate::codegen::compile(sys);
    let dynamic = monitor
        .filter(|_| m.delays.is_empty() && m.externals.is_empty())
        .map(|monitor| Dynamic {
            flat: keep,
            opts: opts.clone(),
            monitor,
        });
    Ok((m, dynamic))
}

/// Simulate `m` (translated by [`translate`]) with dynamic state selection.
pub fn simulate(m: &Model, d: Dynamic, opts: SimOptions) -> Result<(Results, SimStats), SimError> {
    let Dynamic {
        flat,
        opts: bopts,
        monitor,
    } = d;
    let mut recompile = |sw: &StateSwitch, at: &[f64]| {
        let mut o = bopts.clone();
        o.dummy_choice = sw.choice.clone();
        o.select_at = Some(at.to_vec());
        let sys = backend::compile(flat.clone(), &o).ok()?;
        // The switched sets must have taken their new dummies.
        let taken = sw.switched.iter().all(|&i| {
            let c = &sw.choice[i];
            sys.state_sets.iter().any(|s| {
                s.dummies.len() == c.len()
                    && s.dummies
                        .iter()
                        .all(|&z| c.iter().any(|n| *n == sys.vars[z as usize].name))
            })
        });
        if !taken {
            return None;
        }
        let mon = Monitor::new(&sys)?;
        Some((
            crate::codegen::compile(sys),
            Box::new(mon) as Box<dyn StateMonitor>,
        ))
    };
    crate::sim::simulate_switching(m, opts, Box::new(monitor), &mut recompile)
}
