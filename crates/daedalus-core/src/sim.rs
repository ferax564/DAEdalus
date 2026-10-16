//! Simulation driver: initialization, integration, event detection and
//! iteration, and result recording.

use crate::backend::AliasKind;
use crate::codegen::{Action, Model, Runtime};
use crate::expr::VarId;
use crate::flat::VarKind;
use crate::results::{ResultKind, ResultVar, Results};
use crate::solver::{Bdf, Dopri5, FixedMethod, FixedStep, Integrator, SolverStats};
use crate::solver_auto::AutoSwitch;
use crate::solver_esdirk::Esdirk;
pub use crate::solver_jac::JacobianMode;
use crate::solver_radau::Radau5;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Variable-order BDF (stiff; default, like Dassl in Dymola).
    Bdf,
    /// Dormand–Prince 5(4).
    Dopri5,
    Rk4,
    Euler,
    /// Radau IIA order 5 (stiff, L-stable, one-step).
    Radau,
    /// ESDIRK4(3)6L[2]SA (stiff, L-stable, one-step).
    Esdirk,
    /// Dopri5 with automatic switching to BDF when the problem turns stiff, and back.
    Auto,
}

impl Method {
    /// A DAEdalus method name, or the closest one to a Dymola `__Dymola_Algorithm`.
    pub fn parse(s: &str) -> Option<Method> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "bdf" | "dassl" | "ida" | "cvode" | "lsode" | "dlsode" => Method::Bdf,
            "auto" | "lsodar" | "lsoda" => Method::Auto,
            "radau" | "radau5" | "radauiia" | "radau iia" | "radau2a" => Method::Radau,
            "dopri5" | "dopri45" | "dopri853" | "dopri" | "rk45" | "cerk23" | "cerk34"
            | "cerk45" => Method::Dopri5,
            "rk4" | "rkfix4" | "rkfix3" | "rkfix2" | "rkfix" => Method::Rk4,
            "euler" | "expliciteuler" => Method::Euler,
            _ if s.starts_with("esdirk") || s.starts_with("sdirk") || s == "dirk" => Method::Esdirk,
            _ => return None,
        })
    }

    /// Lower-case name as `--method` takes it.
    pub fn name(self) -> &'static str {
        match self {
            Method::Bdf => "bdf",
            Method::Dopri5 => "dopri5",
            Method::Rk4 => "rk4",
            Method::Euler => "euler",
            Method::Radau => "radau",
            Method::Esdirk => "esdirk",
            Method::Auto => "auto",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimOptions {
    pub start_time: f64,
    pub stop_time: f64,
    /// Output interval; 0 means `(stop - start) / intervals`.
    pub interval: f64,
    pub intervals: usize,
    pub tolerance: f64,
    pub method: Method,
    /// Fixed step size for fixed-step methods (0 = output interval).
    pub fixed_step: f64,
    pub max_step: f64,
    /// Store values just before and after events.
    pub store_events: bool,
    /// Most rows stored at events; 0 means about 50 million values' worth (at least as many
    /// rows as output points), so a large model with dense switching keeps a bounded file.
    /// `usize::MAX` keeps every event.
    #[serde(default)]
    pub max_event_rows: usize,
    /// Parameter overrides by name.
    pub params: Vec<(String, f64)>,
    /// Abort after this many seconds of wall-clock time (0 = no limit).
    #[serde(default)]
    pub max_wall_time: f64,
    /// Record a performance profile and event log (see [`crate::profile`] and
    /// [`simulate_profiled`]). Off by default: it costs time.
    #[serde(default)]
    pub profile: bool,
    /// State Jacobian of the implicit methods: exact (tape tangents) or differences.
    #[serde(default)]
    pub jacobian: JacobianMode,
}

impl Default for SimOptions {
    fn default() -> Self {
        SimOptions {
            start_time: 0.0,
            stop_time: 1.0,
            interval: 0.0,
            intervals: 500,
            tolerance: 1e-6,
            method: Method::Bdf,
            fixed_step: 0.0,
            max_step: 0.0,
            store_events: true,
            max_event_rows: 0,
            params: Vec::new(),
            max_wall_time: 0.0,
            profile: false,
            jacobian: JacobianMode::Exact,
        }
    }
}

impl SimOptions {
    /// Defaults from the model's `experiment` annotation.
    pub fn from_model(m: &Model) -> Self {
        let mut o = SimOptions::default();
        let e = &m.experiment;
        if let Some(t) = e.start_time {
            o.start_time = t;
        }
        if let Some(t) = e.stop_time {
            o.stop_time = t;
        }
        if let Some(t) = e.tolerance {
            o.tolerance = t;
        }
        if let Some(t) = e.interval {
            o.interval = t;
        }
        if let Some(a) = &e.algorithm
            && let Some(m) = Method::parse(a)
        {
            o.method = m;
        }
        o
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SimStats {
    pub solver: SolverStats,
    pub events: u64,
    pub state_events: u64,
    pub newton_solves: u64,
    pub newton_iterations: u64,
    pub output_points: usize,
    pub states: usize,
    pub jacobian_colors: usize,
    pub init_ms: f64,
    pub sim_ms: f64,
    /// The integration method used (`--method` name).
    pub method: String,
}

/// Columns stored in the result: all time-varying, non-hidden variables.
struct Recorder {
    stored: Vec<VarId>,
    res: Results,
}

impl Recorder {
    fn new(m: &Model) -> (Recorder, Vec<usize>) {
        let mut stored = Vec::new();
        let mut col_of: FxHashMap<VarId, usize> = FxHashMap::default();
        let alias_of: FxHashMap<VarId, &AliasKind> =
            m.aliases.iter().map(|(v, k)| (*v, k)).collect();
        let is_param: Vec<bool> = {
            let mut p = vec![false; m.vars.len()];
            for &v in &m.param_vars {
                // Inputs are constant during a run, except those FMU components supply.
                p[v as usize] = m.externals.is_empty() || m.vars[v as usize].kind != VarKind::Input;
            }
            p
        };
        let hidden = |name: &str| name.starts_with('$');
        for (i, v) in m.vars.iter().enumerate() {
            let id = i as VarId;
            if is_param[i] || hidden(&v.name) {
                continue;
            }
            // An alias of another stored series shares its column.
            // An alias of a parameter is constant too.
            if let Some(AliasKind::Var { of, .. }) = alias_of.get(&id)
                && (is_param[*of as usize]
                    || (!hidden(&m.vars[*of as usize].name) && !alias_of.contains_key(of)))
            {
                continue;
            }
            col_of.insert(id, stored.len());
            stored.push(id);
        }
        let mut vars = Vec::new();
        let mut order = Vec::new();
        for (i, v) in m.vars.iter().enumerate() {
            let id = i as VarId;
            if hidden(&v.name) {
                continue;
            }
            let kind = if is_param[i] {
                ResultKind::Param(0.0)
            } else if let Some(&c) = col_of.get(&id) {
                ResultKind::Series(c)
            } else if let Some(AliasKind::Var { of, sign }) = alias_of.get(&id) {
                match col_of.get(of) {
                    Some(&c) => ResultKind::Alias {
                        col: c,
                        sign: *sign,
                    },
                    None => ResultKind::Param(0.0),
                }
            } else {
                ResultKind::Series(col_of[&id])
            };
            order.push(i);
            vars.push(ResultVar {
                name: v.name.clone(),
                description: v.description.clone(),
                unit: v.unit.clone(),
                kind,
            });
        }
        let n = stored.len();
        (
            Recorder {
                stored,
                res: Results {
                    model: m.name.clone(),
                    vars,
                    time: Vec::new(),
                    series: vec![Vec::new(); n],
                    log: Vec::new(),
                },
            },
            order,
        )
    }

    /// Drop the last stored row; returns its time.
    fn pop(&mut self) -> Option<f64> {
        for c in &mut self.res.series {
            c.pop();
        }
        self.res.time.pop()
    }

    fn record(&mut self, rt: &mut Runtime) {
        let m = rt.m;
        rt.run(&m.outputs);
        self.res.time.push(rt.time());
        for (c, &v) in self.stored.iter().enumerate() {
            self.res.series[c].push(rt.r[v as usize]);
        }
    }
}

pub struct Simulation<'m> {
    pub rt: Runtime<'m>,
    pub opts: SimOptions,
    cond_prev: Vec<Vec<bool>>,
    /// Largest |lhs - rhs| seen per relation: sets the hysteresis band of its crossing.
    zc_scale: Vec<f64>,
    terminated: Option<String>,
    log: Vec<String>,
    pub stats: SimStats,
    /// Pending time events: jumps of delayed signals coming out of `delay()`.
    delay_events: Vec<f64>,
    /// Event indicators of the external components at the last accepted point.
    ext_z: Vec<f64>,
    /// Performance profile being recorded (`SimOptions::profile`).
    prof: Option<Box<crate::profile::Profiler>>,
    /// The nonlinear solve that stopped the run, for the debugger ([`crate::debug`]).
    pub newton_failure: Option<NewtonFailure>,
}

/// A Newton block that did not converge: which one, where and how far off.
#[derive(Clone, Debug, Serialize)]
pub struct NewtonFailure {
    /// Index into `Model::newton`.
    pub block: usize,
    /// `initialization` or `simulation`.
    pub phase: String,
    pub time: f64,
    /// Largest absolute residual at the best iterate.
    pub residual: f64,
}

#[derive(Debug)]
pub struct SimError {
    pub message: String,
    pub partial: Option<Results>,
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SimError {}

impl<'m> Simulation<'m> {
    pub fn new(m: &'m Model, opts: SimOptions) -> Self {
        Simulation {
            rt: Runtime::new(m),
            opts,
            cond_prev: m.whens.iter().map(|w| vec![false; w.conds.len()]).collect(),
            zc_scale: vec![0.0; m.layout.n_rel],
            terminated: None,
            log: Vec::new(),
            stats: SimStats::default(),
            delay_events: Vec::new(),
            ext_z: Vec::new(),
            prof: None,
            newton_failure: None,
        }
    }

    /// Run `f` on the external components' hook (imported FMUs), if the model has one.
    fn with_ext<R>(
        &mut self,
        f: impl FnOnce(&mut dyn crate::external_component::Hook, &mut Runtime<'m>) -> R,
    ) -> Option<R> {
        let mut h = self.rt.ext.take()?;
        let r = f(&mut *h, &mut self.rt);
        self.rt.ext = Some(h);
        Some(r)
    }

    /// Event indicators of the external components at the current point (after
    /// `run_ode`).
    fn ext_indicators(&mut self) -> Vec<f64> {
        let n = self.rt.ext.as_ref().map_or(0, |h| h.n_indicators());
        let mut z = vec![0.0; n];
        if n > 0 {
            self.with_ext(|h, rt| h.indicators(rt, &mut z));
        }
        z
    }

    /// Event iteration including the external components: `begin` starts an event of
    /// theirs too (false right after their initialization, which leaves them in event
    /// mode).
    fn event_iteration_ext(&mut self, begin: bool) -> Result<(), String> {
        if self.rt.ext.is_none() {
            return self.event_iteration();
        }
        if begin {
            self.with_ext(|h, rt| h.begin_event(rt)).unwrap_or(Ok(()))?;
            self.rt.run_ode();
        }
        self.event_iteration()?;
        let mut settled = false;
        for _ in 0..100 {
            if !self
                .with_ext(|h, rt| h.update_event(rt))
                .unwrap_or(Ok(false))?
            {
                settled = true;
                break;
            }
            self.rt.run_ode();
            self.event_iteration()?;
        }
        if !settled {
            return Err(format!(
                "event iteration with the FMU components did not converge at t = {}",
                self.rt.time()
            ));
        }
        self.with_ext(|h, rt| h.end_event(rt)).unwrap_or(Ok(()))?;
        self.rt.run_ode();
        self.ext_z = self.ext_indicators();
        if let Some(Some(msg)) = self.rt.ext.as_ref().map(|h| h.terminated()) {
            self.terminated.get_or_insert(msg);
        }
        Ok(())
    }

    /// Earliest time event of the external components.
    fn ext_time_event(&self) -> Option<f64> {
        self.rt.ext.as_ref().and_then(|h| h.next_time_event())
    }

    /// Locate the first time in `(ta, tb]` where an event indicator of the external
    /// components changes its sign relative to `self.ext_z`.
    fn locate_ext(&mut self, solver: &dyn Integrator, ta: f64, tb: f64, xi: &mut [f64]) -> f64 {
        let z0 = self.ext_z.clone();
        let crossed = |z: &[f64]| {
            z.iter()
                .zip(&z0)
                .any(|(&a, &b)| (b > 0.0 && a <= 0.0) || (b < 0.0 && a >= 0.0))
        };
        let mut dx = vec![0.0; xi.len()];
        let (mut a, mut b) = (ta, tb);
        for _ in 0..200 {
            if b - a <= 1e-13 * b.abs().max(1.0) {
                break;
            }
            let c = 0.5 * (a + b);
            solver.interpolate(c, xi);
            self.rt.rhs(c, xi, &mut dx);
            let z = self.ext_indicators();
            if crossed(&z) {
                b = c;
            } else {
                a = c;
            }
        }
        b
    }

    /// Record the delayed variables at the current solution point.
    fn record_delays(&mut self) {
        if self.rt.m.delays.is_empty() {
            return;
        }
        let mut jumps = Vec::new();
        self.rt.record_delays(&mut jumps);
        let t = self.rt.time();
        for te in jumps {
            if te > t {
                let i = self.delay_events.partition_point(|&x| x < te);
                if self.delay_events.get(i) != Some(&te) {
                    self.delay_events.insert(i, te);
                }
            }
        }
    }

    fn eval_params(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        let overrides: FxHashMap<&str, f64> = self
            .opts
            .params
            .iter()
            .map(|(n, v)| (n.as_str(), *v))
            .collect();
        for (p, seg) in &m.params {
            let name = m.vars[*p as usize].name.as_str();
            if let Some(&v) = overrides.get(name) {
                self.rt.r[*p as usize] = v;
            } else {
                self.rt.run(seg);
            }
        }
        for (n, _) in &self.opts.params {
            if !m
                .vars
                .iter()
                .any(|v| &v.name == n && matches!(v.kind, VarKind::Parameter | VarKind::Input))
            {
                return Err(format!("unknown parameter '{n}'"));
            }
        }
        Ok(())
    }

    fn when_conds(&mut self) -> Vec<Vec<bool>> {
        let m = self.rt.m;
        self.rt.run(&m.when_prog);
        m.whens
            .iter()
            .map(|w| {
                w.conds
                    .iter()
                    .map(|&s| self.rt.r[s as usize] != 0.0)
                    .collect()
            })
            .collect()
    }

    /// Execute fired when-clauses. Returns true if any action ran.
    fn fire(&mut self, conds: &[Vec<bool>], prev: &[Vec<bool>]) -> bool {
        let m = self.rt.m;
        // Evaluate all new values first (simultaneous semantics), then assign.
        let mut updates: Vec<(usize, f64)> = Vec::new();
        let mut any = false;
        let mut fired: Vec<usize> = Vec::new();
        for &wi in &m.when_order {
            let w = &m.whens[wi];
            for (bi, acts) in w.actions.iter().enumerate() {
                if conds[wi][bi] && !prev[wi][bi] {
                    any = true;
                    // A clause reading what an earlier clause of this event assigned sees
                    // the new value (when-equations are solved together).
                    if !updates.is_empty() && m.when_deps[wi].iter().any(|d| fired.contains(d)) {
                        for (v, x) in updates.drain(..) {
                            self.rt.r[v] = x;
                        }
                        self.rt.run_ode();
                        self.rt.run(&m.when_prog);
                    }
                    fired.push(wi);
                    if !w.effects[bi].is_empty() {
                        self.rt.run(&w.effects[bi]);
                        self.drain_ext_log();
                    }
                    if let Some(p) = self.prof.as_deref_mut() {
                        p.when_fired(wi, bi);
                    }
                    let chained = w.chained[bi];
                    for (k, a) in acts.iter().enumerate() {
                        match a {
                            Action::Assign(v, s) | Action::Reinit(v, s) if chained => {
                                // Later actions of this branch read this value: commit it now.
                                if k > 0 {
                                    self.rt.run(&m.when_prog);
                                }
                                self.rt.r[*v as usize] = self.rt.r[*s as usize];
                            }
                            Action::Assign(v, s) | Action::Reinit(v, s) => {
                                updates.push((*v as usize, self.rt.r[*s as usize]));
                            }
                            Action::Terminate(msg) => self.terminated = Some(msg.clone()),
                            Action::Assert(c, msg) => {
                                if self.rt.r[*c as usize] == 0.0 {
                                    self.rt_log(format!(
                                        "assertion failed at t = {}: {msg}",
                                        self.rt.time()
                                    ));
                                }
                            }
                        }
                    }
                    break; // only the first active branch of an elsewhen chain
                }
                if conds[wi][bi] {
                    break;
                }
            }
        }
        for (v, x) in updates {
            self.rt.r[v] = x;
        }
        any
    }

    fn rt_log(&mut self, s: String) {
        self.log.push(s);
    }

    /// The terminal event at the end of a successful simulation: `terminal()` is true and
    /// the when-clauses it activates run (writing result files, for instance).
    fn terminal_event(&mut self) {
        let m = self.rt.m;
        self.rt.run(&m.ode);
        let prev = self.when_conds();
        self.rt.r[m.layout.terminal] = 1.0;
        let conds = self.when_conds();
        self.fire(&conds, &prev);
        self.rt.r[m.layout.terminal] = 0.0;
        self.drain_ext_log();
    }

    /// Move the lines printed by external functions (`print`, errors) to the log.
    fn drain_ext_log(&mut self) {
        if !self.rt.loops.ext.log.is_empty() {
            let lines = self.rt.loops.ext.take_log();
            self.log.extend(lines);
        }
    }

    /// Fire the when-clauses whose conditions rose against `prev`, then follow the
    /// relations and conditions the actions change until nothing more fires. Returns
    /// whether anything fired and the final condition values.
    fn fire_chain(
        &mut self,
        mut conds: Vec<Vec<bool>>,
        prev: &[Vec<bool>],
        rel_changed: &mut bool,
    ) -> (bool, Vec<Vec<bool>>) {
        let fired = self.fire(&conds, prev);
        // Relations read the values the actions just assigned (`time >= t_start +
        // waitTime` right after `t_start := time`), then the algebraics follow. A
        // condition that only becomes true with those values belongs to the same
        // solution of the event equations: fire it before anything is committed.
        if fired {
            for _ in 0..100 {
                self.rt.run_ode();
                *rel_changed |= self.rt.latch_relations();
                self.rt.run_ode();
                let now = self.when_conds();
                let seen = Self::or_conds(&conds, prev);
                let more = self.fire(&now, &seen);
                conds = now;
                if !more {
                    break;
                }
            }
        }
        (fired, conds)
    }

    fn or_conds(a: &[Vec<bool>], b: &[Vec<bool>]) -> Vec<Vec<bool>> {
        a.iter()
            .zip(b)
            .map(|(c, p)| c.iter().zip(p).map(|(&x, &y)| x || y).collect())
            .collect()
    }

    /// Event iteration at the current time until discrete values settle.
    fn event_iteration(&mut self) -> Result<(), String> {
        self.rt.hold_samples = true;
        self.rt.held_samples.clear();
        let r = self.event_iteration_inner();
        self.rt.hold_samples = false;
        self.rt.held_samples.clear();
        r
    }

    fn event_iteration_inner(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        let mut hist: Vec<Vec<f64>> = Vec::new();
        for _ in 0..100 {
            self.rt.save_pre();
            let n_rel = m.layout.n_rel;
            let rel = m.layout.rel0..m.layout.rel0 + n_rel;
            let rels0 = self.rt.r[rel.clone()].to_vec();
            let mut rel_changed = self.rt.latch_relations();
            // Whether the relations that just changed sit on their switching points.
            let on_point = (0..n_rel).all(|i| {
                self.rt.r[m.layout.rel0 + i] == rels0[i]
                    || self.rt.r[m.zc_g[i] as usize].abs() <= self.hysteresis(i)
            });
            self.rt.run_ode();
            let prev = std::mem::take(&mut self.cond_prev);
            let conds = self.when_conds();
            let (mut fired, mut conds) = self.fire_chain(conds, &prev, &mut rel_changed);
            // The when-clauses of this iteration have acted (`entryTime := time` of a
            // firing pulse timer): the relations settle on the values they assigned, not on
            // a transient that would switch a latching element (a thyristor) off.
            let rels1 = self.rt.r[rel.clone()].to_vec();
            let (moved, unresolved) = self.settle_relations(&rels1);
            rel_changed |= moved;
            // Relations on their switching points that are back at the values they had
            // before this iteration (a relation on an ill-conditioned loop whose rounding
            // flips it, and back) did not move.
            if on_point && !unresolved && self.rt.r[rel.clone()] == rels0[..] {
                rel_changed = false;
            }
            // Conditions on the relations the mixed system just moved (`u >= x` after a
            // jump of `u` at this instant) are edges of this iteration: once pre() moves
            // on they would be lost.
            if self.rt.r[rel.clone()] != rels1[..] {
                let seen = Self::or_conds(&conds, &prev);
                let now = self.when_conds();
                let (more, now) = self.fire_chain(now, &seen, &mut rel_changed);
                fired |= more;
                conds = now;
            }
            self.cond_prev = conds;
            // Recompute algebraics with the new discrete values.
            self.rt.run_ode();
            if std::env::var("DAEDALUS_DEBUG_EVENTS").is_ok() {
                let ch: Vec<String> = m
                    .discretes
                    .iter()
                    .filter(|&&d| self.rt.r[d as usize] != self.rt.r[m.layout.pre0 + d as usize])
                    .map(|&d| format!("{}={}", m.vars[d as usize].name, self.rt.r[d as usize]))
                    .collect();
                let rels: String = (0..m.layout.n_rel)
                    .map(|i| {
                        if self.rt.r[m.layout.rel0 + i] != 0.0 {
                            '1'
                        } else {
                            '0'
                        }
                    })
                    .collect();
                if std::env::var("DAEDALUS_DEBUG_EVENTS").as_deref() == Ok("2") {
                    for (i, t) in m.rel_text.iter().enumerate() {
                        eprintln!("  rel{i}: {t}");
                    }
                }
                eprintln!(
                    "t={} fired={fired} rel={rel_changed} [{rels}] {}",
                    self.rt.time(),
                    ch.join(" ")
                );
            }
            let disc_changed = m.discretes.iter().any(|&d| {
                let a = self.rt.r[d as usize];
                let b = self.rt.r[m.layout.pre0 + d as usize];
                a != b && !(a.is_nan() && b.is_nan())
            });
            if !rel_changed && !fired && !disc_changed {
                if !self.rt.held_samples.is_empty() {
                    // The event instant is over: sample() turns false again, and what
                    // depends on it settles in the following iterations.
                    self.rt.hold_samples = false;
                    self.rt.held_samples.clear();
                    continue;
                }
                self.rt.save_pre();
                return Ok(());
            }
            // Pure relation cycle (a Schmitt trigger switching between a saturated and the
            // linear branch, neither consistent): fixed-point iteration never visits the
            // consistent mode, so search the modes of the relations involved.
            if !fired && !disc_changed {
                let rels = self.rt.r[m.layout.rel0..m.layout.rel0 + m.layout.n_rel].to_vec();
                if let Some(k) = hist.iter().position(|h| *h == rels) {
                    let toggling: Vec<usize> = (0..rels.len())
                        .filter(|&i| hist[k..].iter().any(|h| h[i] != rels[i]))
                        .collect();
                    if self.mode_search(&toggling) {
                        self.rt.save_pre();
                        return Ok(());
                    }
                }
                hist.push(rels);
            }
        }
        Err(format!(
            "event iteration did not converge at t = {}",
            self.rt.time()
        ))
    }

    /// Solve the event equations for the current `pre` values: relations take the values
    /// of the variables they compute (MLS 8.6), so relations and the equations they switch
    /// form one mixed system at fixed `pre` (a stuck friction element: `locked` -> `sa` ->
    /// `sa > tau0_max` -> `startForward` -> `locked`). A friction element that becomes
    /// stuck tests `sa > tau0_max` on the locking torque, not on the value `sa` had while
    /// it was still sliding. Relations that cycle (forward sliding gives `sa < -tau0_max`,
    /// backward `sa > tau0_max`) skip the consistent mode in between: search for it.
    /// Returns whether the relations moved from `start`, and whether no consistent mode
    /// was found (the relations are then back at `start`).
    fn settle_relations(&mut self, start: &[f64]) -> (bool, bool) {
        let m = self.rt.m;
        let rel = m.layout.rel0..m.layout.rel0 + m.layout.n_rel;
        let mut hist: Vec<Vec<f64>> = vec![start.to_vec()];
        let mut unresolved = false;
        for _ in 0..50 {
            if !self.relatch_outside_band() {
                break;
            }
            self.rt.run_ode();
            let rels = self.rt.r[rel.clone()].to_vec();
            if let Some(k) = hist.iter().position(|h| *h == rels) {
                let toggling: Vec<usize> = (0..rels.len())
                    .filter(|&i| hist[k..].iter().any(|h| h[i] != rels[i]))
                    .collect();
                if !self.mode_search(&toggling) {
                    // No consistent mode of these relations alone (it involves other
                    // elements' modes too): leave it to the iteration on pre() from the
                    // values the when-clauses left.
                    self.rt.r[rel.clone()].copy_from_slice(start);
                    self.rt.run_ode();
                    unresolved = true;
                }
                break;
            }
            hist.push(rels);
        }
        (self.rt.r[rel] != start[..], unresolved)
    }

    /// Re-latch the relations on the values the last solve of the event equations gave,
    /// except those that sit inside their hysteresis band: a relation that put the
    /// system on its own switching point (a friction element that starts sliding with
    /// exactly the breakaway torque, the bearing next to a stuck gear) keeps its value
    /// instead of flipping on the rounding. Returns whether any relation changed.
    fn relatch_outside_band(&mut self) -> bool {
        let m = self.rt.m;
        self.rt.eval_zc();
        let mut changed = false;
        for i in 0..m.layout.n_rel {
            let s = m.layout.rel0 + i;
            let raw = self.rt.r[m.zc_raw[i] as usize];
            // A sample() tick held true for the event instant stays true (see
            // `Runtime::held_samples`).
            if self.rt.hold_samples && m.sample_rels.contains(&i) {
                if self.rt.held_samples.contains(&i) {
                    continue;
                }
                if raw != 0.0 {
                    self.rt.held_samples.push(i);
                }
            }
            if self.rt.r[s] != raw && !(self.rt.r[m.zc_g[i] as usize].abs() <= self.hysteresis(i)) {
                self.rt.r[s] = raw;
                changed = true;
            }
        }
        changed
    }

    /// Look for relation values that reproduce themselves: the relations that toggled in
    /// a cycle, plus those comparing the same expression (the other limit of a limiter),
    /// with duplicate copies of one relation kept equal. Tries the modes closest to the
    /// current one first; returns whether a consistent one was found and latched.
    fn mode_search(&mut self, toggling: &[usize]) -> bool {
        let m = self.rt.m;
        let lhs = |i: usize| {
            let t = &m.rel_text[i];
            let op = format!(" {:?} ", m.rel_ops[i]);
            t.split(op.as_str()).next().unwrap_or(t).to_string()
        };
        let lhs_set: FxHashSet<String> = toggling.iter().map(|&i| lhs(i)).collect();
        let mut atoms: Vec<&str> = Vec::new();
        for i in 0..m.layout.n_rel {
            if lhs_set.contains(&lhs(i)) && !atoms.contains(&m.rel_text[i].as_str()) {
                atoms.push(&m.rel_text[i]);
            }
        }
        if atoms.is_empty() || atoms.len() > 10 {
            return false;
        }
        let members: Vec<Vec<usize>> = atoms
            .iter()
            .map(|a| {
                (0..m.layout.n_rel)
                    .filter(|&i| m.rel_text[i] == *a)
                    .collect()
            })
            .collect();
        let base: Vec<f64> = self.rt.r[m.layout.rel0..m.layout.rel0 + m.layout.n_rel].to_vec();
        let cur: u32 = members
            .iter()
            .enumerate()
            .map(|(k, ms)| u32::from(base[ms[0]] != 0.0) << k)
            .sum();
        let mut masks: Vec<u32> = (0..1u32 << atoms.len()).collect();
        masks.sort_by_key(|&x| ((x ^ cur).count_ones(), x));
        for mask in masks {
            for (k, ms) in members.iter().enumerate() {
                let v = f64::from((mask >> k) & 1);
                for &i in ms {
                    self.rt.r[m.layout.rel0 + i] = v;
                }
            }
            self.rt.run_ode();
            if self.rt.error.is_some() {
                self.rt.error = None;
                continue;
            }
            self.rt.run(&m.zc);
            let ok = (0..m.layout.n_rel)
                .all(|i| self.rt.r[m.zc_raw[i] as usize] == self.rt.r[m.layout.rel0 + i]);
            if ok {
                return true;
            }
        }
        self.rt.r[m.layout.rel0..m.layout.rel0 + m.layout.n_rel].copy_from_slice(&base);
        self.rt.run_ode();
        self.rt.error = None;
        false
    }

    fn check_newton(&mut self, phase: &str) -> Result<(), String> {
        if let Some(e) = self.rt.error.take() {
            let b = &self.rt.m.newton[e.block];
            self.newton_failure = Some(NewtonFailure {
                block: e.block,
                phase: phase.into(),
                time: self.rt.time(),
                residual: e.residual,
            });
            return Err(format!(
                "{phase}: nonlinear system for [{}] did not converge at t = {} (residual {:.3e})",
                b.names.join(", "),
                self.rt.time(),
                e.residual
            ));
        }
        Ok(())
    }

    /// Solve the initial equations. With `homotopy()` in the model, start from the
    /// simplified equations (lambda = 0) and continue to the actual ones (lambda = 1),
    /// each solve starting from the last solution; fall back to a direct solve.
    fn solve_init(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        let lambda = m
            .vars
            .iter()
            .position(|v| v.name == crate::flat::HOMOTOPY_LAMBDA && v.kind == VarKind::Parameter);
        if let Some(l) = lambda {
            let saved = self.rt.r.clone();
            let mut ok = true;
            let mut at = 0.0f64;
            let mut h = 0.1f64;
            self.rt.r[l] = 0.0;
            self.rt.run(&m.init);
            if self.rt.error.take().is_some() {
                ok = false;
            }
            while ok && at < 1.0 {
                let next = (at + h).min(1.0);
                let before = self.rt.r.clone();
                self.rt.r[l] = next;
                self.rt.run(&m.init);
                if self.rt.error.take().is_some()
                    || self.rt.r[..m.layout.nv].iter().any(|x| x.is_nan())
                {
                    self.rt.r = before;
                    h /= 4.0;
                    if h < 1e-4 {
                        ok = false;
                    }
                } else {
                    at = next;
                    h = (h * 2.0).min(0.25);
                }
            }
            if ok {
                return Ok(());
            }
            self.rt.r = saved;
            self.rt.r[l] = 1.0;
        }
        self.rt.init_mode = true;
        self.rt.run(&m.init);
        self.rt.init_mode = false;
        self.check_newton("initialization")
    }

    /// Parameter values and start values at the start time, before the initial equations
    /// are solved (all registers, indexed by variable).
    pub fn start_values(&mut self) -> Result<Vec<f64>, String> {
        let m = self.rt.m;
        self.eval_params()?;
        self.rt.set_time(self.opts.start_time);
        self.rt.r[m.layout.initial] = 1.0;
        self.rt.run(&m.starts);
        Ok(self.rt.r[..m.layout.nv].to_vec())
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        let t0 = self.opts.start_time;
        self.eval_params()?;
        self.rt.set_time(t0);
        self.rt.r[m.layout.initial] = 1.0;
        self.rt.run(&m.starts);
        self.rt.save_pre();
        if !m.externals.is_empty() && self.rt.ext.is_none() {
            let make = crate::external_component::factory()
                .ok_or_else(|| crate::external_component::unsupported(m))?;
            self.rt.ext = Some(make(m)?);
        }
        let (stop, tol) = (self.opts.stop_time, self.opts.tolerance);
        self.with_ext(|h, rt| h.init(rt, t0, stop, tol))
            .unwrap_or(Ok(()))?;
        self.solve_init()?;
        if self.rt.ext.is_some() {
            // The FMUs entered initialization mode with the start values of their inputs:
            // pass the solved inputs while they can still initialize with them, solving
            // the model's initialization again until their outputs settle.
            let mut settled = false;
            for _ in 0..50 {
                if !self
                    .with_ext(|h, rt| h.init_update(rt))
                    .unwrap_or(Ok(false))?
                {
                    settled = true;
                    break;
                }
                self.solve_init()?;
            }
            if !settled {
                return Err(
                    "initialization: the outputs of the FMU components did not settle \
                            with the solved inputs"
                        .into(),
                );
            }
            // Their initial states and outputs after initialization (which may still depend
            // on the inputs): solve the model's initialization once more with them.
            if self
                .with_ext(|h, rt| h.exit_init(rt))
                .unwrap_or(Ok(false))?
            {
                self.solve_init()?;
            }
        }
        for &(v, p) in &m.pre_init {
            self.rt.r[m.layout.pre0 + v as usize] = self.rt.r[p as usize];
        }
        self.rt.latch_relations();
        for &r in &m.sample_rels {
            self.rt.r[m.layout.rel0 + r] = 0.0;
        }
        self.rt.run_ode();
        // when-clauses activated by initial(): the left limit of a continuous-time
        // variable at the start time, pre(x), is its initial value.
        let mut assigned = vec![false; m.layout.nv];
        for w in &m.whens {
            for acts in &w.actions {
                for a in acts {
                    if let Action::Assign(v, _) | Action::Reinit(v, _) = a {
                        assigned[*v as usize] = true;
                    }
                }
            }
        }
        for (v, var) in m.vars.iter().enumerate() {
            if var.kind == VarKind::Continuous && v < m.layout.nv && !assigned[v] {
                self.rt.r[m.layout.pre0 + v] = self.rt.r[v];
            }
        }
        let c1 = self.when_conds();
        self.rt.r[m.layout.initial] = 0.0;
        let c0 = self.when_conds();
        self.rt.r[m.layout.initial] = 1.0;
        let edge: Vec<Vec<bool>> = c0
            .iter()
            .zip(&m.whens)
            .map(|(c, w)| {
                c.iter()
                    .zip(&w.at_initial)
                    .map(|(&c, &i)| c && !i)
                    .collect()
            })
            .collect();
        let fired = self.fire(&c1, &edge);
        if fired {
            // The clauses fired by the initialization belong to it: the equations reading
            // their results (`change(y)`, `Q = S or pre(Q)`) see the start values in pre()
            // before the event iteration at the start time takes them over.
            self.rt.run_ode();
        }
        self.cond_prev = c0;
        self.rt.r[m.layout.initial] = 0.0;
        // Event iteration at the start time (handles sample() at t0 and initial() clauses).
        self.event_iteration_ext(false)?;
        self.check_newton("initialization")?;
        if !m.delays.is_empty() {
            // delay(x, d) = x(start) until start + d: settle the delays on the initial values
            self.rt.run_ode();
            for _ in 0..2 {
                for b in &mut self.rt.delays {
                    b.t.clear();
                    b.v.clear();
                }
                self.record_delays();
                self.rt.run_ode();
                self.event_iteration()?;
                self.check_newton("initialization")?;
            }
            self.delay_events.clear();
        }
        Ok(())
    }

    /// Run the whole simulation.
    pub fn run(self) -> Result<(Results, SimStats), SimError> {
        self.run_profiled().0
    }

    /// Run the whole simulation; with `SimOptions::profile` also return its performance
    /// profile (on failure too, up to where the run stopped).
    pub fn run_profiled(
        mut self,
    ) -> (
        Result<(Results, SimStats), SimError>,
        Option<crate::profile::Profile>,
    ) {
        let m = self.rt.m;
        if self.opts.profile {
            self.prof = Some(Box::new(crate::profile::Profiler::new(m)));
            self.rt.prof = Some(Box::new(crate::profile::BlockProf::new(m)));
        }
        let r = self.run_inner();
        let span = self.opts.stop_time - self.opts.start_time;
        let method = format!("{:?}", self.opts.method);
        let profile = self.prof.take().map(|p| {
            let s = match &r {
                Ok((_, s)) => s,
                Err(_) => &self.stats,
            };
            p.finish(m, self.rt.prof.take(), s, &method, span)
        });
        (r, profile)
    }

    fn run_inner(&mut self) -> Result<(Results, SimStats), SimError> {
        let m = self.rt.m;
        let (mut rec, order) = Recorder::new(m);
        let t_init = crate::backend::web_time_now();
        if let Err(e) = self.initialize() {
            return Err(SimError {
                message: e,
                partial: None,
            });
        }
        if let Some(p) = self.prof.as_deref_mut() {
            p.initial_event(self.rt.time());
        }
        // Parameter values into the result.
        self.param_values_into(&mut rec, &order);
        let t1 = crate::backend::web_time_now();
        self.stats.init_ms = (t1 - t_init) * 1000.0;
        let r = self.integrate(&mut rec);
        if r.is_ok() {
            self.terminal_event();
        }
        self.stats.sim_ms = (crate::backend::web_time_now() - t1) * 1000.0;
        self.stats.newton_solves = self.rt.newton_stats.solves;
        self.stats.newton_iterations = self.rt.newton_stats.iterations;
        self.stats.output_points = rec.res.time.len();
        self.stats.states = m.states.len();
        self.drain_ext_log();
        rec.res.log.append(&mut self.log);
        if let Some(msg) = &self.terminated {
            rec.res.log.push(format!("terminated: {msg}"));
        }
        match r {
            Ok(()) => Ok((rec.res, self.stats.clone())),
            Err(e) => Err(SimError {
                message: e,
                partial: Some(rec.res),
            }),
        }
    }

    fn integrate(&mut self, rec: &mut Recorder) -> Result<(), String> {
        let mut st = self.make_stepper();
        if self.prof.is_some()
            && let Some(p) = st.solver.probe()
        {
            p.enable(self.rt.m.states.len());
        }
        rec.record(&mut self.rt);
        self.restart(&mut st);
        st.event_budget = self.event_budget(rec);
        let stop = self.opts.stop_time;
        let r = self.advance(&mut st, stop, Some(&mut *rec));
        self.stats.solver = st.solver.stats();
        r?;
        self.stop_time_event(&mut st, rec)?;
        Ok(())
    }

    /// Most rows stored at events (see [`SimOptions::max_event_rows`]).
    fn event_budget(&self, rec: &Recorder) -> usize {
        if self.opts.max_event_rows > 0 {
            self.opts.max_event_rows
        } else {
            // About 10 million written values of event rows (every column, aliases
            // included, as the result file holds them): every event of a small model, a
            // bounded file for a large one, however fine the output grid.
            let per_row = rec.res.vars.len().max(rec.stored.len()).max(1);
            (10_000_000 / per_row).max(2_000)
        }
    }

    /// Parameter values (as evaluated now) into the result's parameter entries.
    fn param_values_into(&mut self, rec: &mut Recorder, order: &[usize]) {
        let m = self.rt.m;
        self.rt.run(&m.outputs);
        for (k, &i) in order.iter().enumerate() {
            if let ResultKind::Param(_) = rec.res.vars[k].kind {
                rec.res.vars[k].kind = ResultKind::Param(self.rt.r[i]);
            }
        }
    }

    /// Integrator state for stepping the model forward from its current (initialized,
    /// consistent) solution point.
    pub fn stepper(&mut self) -> Stepper {
        let mut st = self.make_stepper();
        self.restart(&mut st);
        st
    }

    fn make_stepper(&mut self) -> Stepper {
        let m = self.rt.m;
        let o = &self.opts;
        let n = m.states.len();
        let span = o.stop_time - o.start_time;
        let dt = if o.interval > 0.0 {
            o.interval
        } else {
            span / o.intervals.max(1) as f64
        };
        let mut max_step = if o.max_step > 0.0 {
            o.max_step
        } else {
            span.max(1e-12)
        };
        // Steps no longer than the shortest delay: delayed values come from the history,
        // never from the step being taken.
        for b in &self.rt.delays {
            if b.d > 0.0 && b.d < max_step {
                max_step = b.d;
            }
        }
        // Without states nothing limits the step: still look for crossings at least once
        // per output interval.
        if n == 0 && dt > 0.0 && dt < max_step {
            max_step = dt;
        }
        let rtol = o.tolerance;
        // Absolute tolerance per state from its nominal value, as Dymola and
        // OpenModelica do: a state of size 1e-4 (`nominal = 1e-4`) is integrated to
        // 1e-4 * tolerance, not to the tolerance itself.
        let atol: Vec<f64> = {
            self.rt.run(&m.nominals);
            m.state_nominals
                .iter()
                .map(|s| {
                    let v = s.map_or(1.0, |s| self.rt.r[s as usize].abs());
                    o.tolerance * if v.is_finite() && v > 0.0 { v } else { 1.0 }
                })
                .collect()
        };
        self.stats.method = o.method.name().to_string();
        // FMU components: the tape does not see through them (their derivatives and
        // outputs are set from outside), so the state Jacobian is dense and by
        // differences of the right-hand side.
        let full: Vec<Vec<u32>>;
        let (pattern, jacobian) = if m.externals.is_empty() {
            (&m.jac_pattern, o.jacobian)
        } else {
            full = vec![(0..n as u32).collect(); n];
            (&full, JacobianMode::Fd)
        };
        let solver: Box<dyn Integrator> = match o.method {
            Method::Bdf => {
                let b = Bdf::new(n, rtol, atol.clone(), max_step, pattern).with_jacobian(jacobian);
                self.stats.jacobian_colors = b.n_colors();
                Box::new(b)
            }
            Method::Radau => {
                let r = Radau5::new(n, rtol, atol.clone(), max_step, pattern, jacobian);
                self.stats.jacobian_colors = r.n_colors();
                Box::new(r)
            }
            Method::Esdirk => {
                let e = Esdirk::new(n, rtol, atol.clone(), max_step, pattern, jacobian);
                self.stats.jacobian_colors = e.n_colors();
                Box::new(e)
            }
            Method::Auto => {
                let b = Bdf::new(n, rtol, atol.clone(), max_step, pattern).with_jacobian(jacobian);
                self.stats.jacobian_colors = b.n_colors();
                Box::new(AutoSwitch::new(Dopri5::new(n, rtol, atol, max_step), b))
            }
            Method::Dopri5 => Box::new(Dopri5::new(n, rtol, atol, max_step)),
            Method::Rk4 => Box::new(FixedStep::new(
                n,
                FixedMethod::Rk4,
                if o.fixed_step > 0.0 { o.fixed_step } else { dt },
            )),
            Method::Euler => Box::new(FixedStep::new(
                n,
                FixedMethod::Euler,
                if o.fixed_step > 0.0 { o.fixed_step } else { dt },
            )),
        };
        Stepper {
            solver,
            x: vec![0.0; n],
            xi: vec![0.0; n],
            t: self.rt.time(),
            k_out: 1,
            dt,
            max_step,
            event_budget: usize::MAX,
            event_rows: 0,
            guard: 0,
            last_event: f64::NEG_INFINITY,
            last_event_rows: usize::MAX,
            same_instant: 0,
            wall0: crate::backend::web_time_now(),
            deadline: f64::INFINITY,
        }
    }

    /// Restart the integrator from the runtime's current states and time (after inputs,
    /// states or discrete values were changed from outside).
    pub fn restart(&mut self, st: &mut Stepper) {
        self.rt.get_states(&mut st.x);
        st.t = self.rt.time();
        st.solver.reset(&mut self.rt, st.t, &st.x);
    }

    /// Integrate (with event handling) from the stepper's time to `t_stop`, leaving the
    /// runtime with the consistent solution at the time reached (`t_stop`, or where
    /// `terminate()` stopped the run).
    pub fn advance_to(&mut self, st: &mut Stepper, t_stop: f64) -> Result<(), String> {
        self.advance(st, t_stop, None)?;
        let m = self.rt.m;
        let t = st.t;
        // A time event exactly at `t_stop` is handled here, as at the end of a full run:
        // the caller sees its right value, and the next call starts after it.
        if self.endpoint_time_event(st, t_stop)? {
            self.rt.get_states(&mut st.xi);
            st.solver.reset(&mut self.rt, t, &st.xi);
        }
        // The registers hold whatever the solver evaluated last: recompute the solution.
        if (st.solver.t() - t).abs() <= 1e-15 * t.abs().max(1.0) {
            st.x.copy_from_slice(st.solver.x());
        } else {
            st.solver.interpolate(t, &mut st.x);
        }
        let xs = st.x.clone();
        self.rt.rhs(t, &xs, &mut st.xi);
        self.rt.error = None;
        self.rt.run(&m.outputs);
        self.stats.solver = st.solver.stats();
        Ok(())
    }

    /// Integrate without stopping at intermediate times and call `sample(i, rt)` with the
    /// solution at each of `times` (increasing, after the stepper's time) from the
    /// integrator's dense output. For smooth runs only: when a relation changes (a state
    /// event) it stops and returns the time of the step where it changed. Stopping the
    /// integrator at many close times instead (as [`Simulation::advance_to`] does) can
    /// make step size control fail.
    pub fn advance_sampled(
        &mut self,
        st: &mut Stepper,
        times: &[f64],
        sample: &mut dyn FnMut(usize, &mut Runtime) -> Result<(), String>,
    ) -> Result<Option<f64>, String> {
        let m = self.rt.m;
        let Some(&t_stop) = times.last() else {
            return Ok(None);
        };
        let n_rel = m.layout.n_rel;
        let mut k = 0;
        let mut t = st.t;
        let mut emit = |sim: &mut Self, st: &mut Stepper, upto: f64, k: &mut usize| {
            while *k < times.len() && times[*k] <= upto * (1.0 + 1e-14) + 1e-300 {
                let tk = times[*k];
                if (st.solver.t() - tk).abs() <= 1e-15 * tk.abs().max(1.0) {
                    st.xi.copy_from_slice(st.solver.x());
                } else {
                    st.solver.interpolate(tk, &mut st.xi);
                }
                let xi = st.xi.clone();
                sim.rt.rhs(tk, &xi, &mut st.x);
                sim.rt.run(&m.outputs);
                sample(*k, &mut sim.rt)?;
                *k += 1;
            }
            Ok::<(), String>(())
        };
        emit(self, st, t, &mut k)?;
        while t < t_stop * (1.0 - 1e-15) - 1e-300 {
            st.guard += 1;
            if st.guard > 50_000_000 {
                return Err("too many steps".into());
            }
            let tn = st.solver.step(&mut self.rt, t_stop)?;
            self.log.extend(st.solver.take_log());
            self.rt.error = None;
            if n_rel > 0 {
                self.rt.rhs(tn, st.solver.x(), &mut st.xi);
                self.rt.eval_zc();
                if (0..n_rel)
                    .any(|i| self.rt.r[m.zc_raw[i] as usize] != self.rt.r[m.layout.rel0 + i])
                {
                    return Ok(Some(tn));
                }
            }
            emit(self, st, tn, &mut k)?;
            t = tn;
            st.t = tn;
        }
        emit(self, st, f64::INFINITY, &mut k)?;
        self.stats.solver = st.solver.stats();
        Ok(None)
    }

    /// The message of `terminate()`, once the model has called it.
    pub fn terminated(&self) -> Option<&str> {
        self.terminated.as_deref()
    }

    /// Process an event at the current time (after discrete inputs or parameters were
    /// changed from outside, or a crossing found by an external integrator): event
    /// iteration until the discrete values settle.
    pub fn handle_event(&mut self) -> Result<(), String> {
        self.stats.events += 1;
        self.rt.delay_right = true;
        self.event_iteration_ext(true)?;
        self.check_newton("event")?;
        self.record_delays();
        Ok(())
    }

    /// Record delayed values after an accepted step of an external integrator; returns
    /// the earliest pending time event (a delayed jump), if any.
    pub fn completed_step(&mut self) -> Option<f64> {
        self.record_delays();
        let t = self.rt.time();
        let k = self
            .delay_events
            .partition_point(|&x| x < t * (1.0 - 1e-14) - 1e-300);
        self.delay_events.drain(..k);
        self.delay_events.first().copied()
    }

    /// Drop pending time events up to the current time (they were just handled).
    pub fn consume_time_events(&mut self) -> Option<f64> {
        let t = self.rt.time();
        let k = self
            .delay_events
            .partition_point(|&x| x <= t * (1.0 + 1e-14) + 1e-300);
        self.delay_events.drain(..k);
        self.delay_events.first().copied()
    }

    /// The earliest pending `sample()` tick after `t`.
    fn next_sample_time(&self, t: f64) -> Option<f64> {
        let m = self.rt.m;
        m.sample_rels
            .iter()
            .zip(&m.sample_next)
            .filter(|&(&i, _)| self.rt.r[m.layout.rel0 + i] == 0.0)
            .map(|(_, &v)| self.rt.r[v as usize])
            // A tick only round-off ahead (a slice ended at 0.1 + 0.1 + .. just short of
            // 0.4) is no step limit: the next step crosses it and it fires at its time.
            .filter(|&ts| ts > t + 1e-12 * t.abs().max(1e-3))
            .min_by(f64::total_cmp)
            .map(|t0| {
                let ticks = m
                    .sample_rels
                    .iter()
                    .zip(&m.sample_next)
                    .filter(|&(&i, _)| self.rt.r[m.layout.rel0 + i] == 0.0)
                    .map(|(_, &v)| self.rt.r[v as usize]);
                sample_cluster_end(t0, ticks)
            })
    }

    fn advance(
        &mut self,
        st: &mut Stepper,
        t_stop: f64,
        mut rec: Option<&mut Recorder>,
    ) -> Result<(), String> {
        let m = self.rt.m;
        let o = self.opts.clone();
        let n = m.states.len();
        let dt = st.dt;
        let max_step = st.max_step;
        let next_out = |k: u64| o.start_time + k as f64 * dt;
        let event_budget = st.event_budget;
        let n_rel = m.layout.n_rel;
        let wall0 = st.wall0;
        let mut t = st.t;
        let mut steps_here = 0u32;
        while t < t_stop * (1.0 - 1e-15) - 1e-300 && self.terminated.is_none() {
            // A run in slices (`Incremental`) gives the caller control back once its wall
            // clock budget is spent; the loop state lives in `st`, so the next call
            // continues exactly where this one stopped.
            if st.deadline.is_finite()
                && steps_here > 0
                && steps_here % 4 == 0
                && crate::profile::now() > st.deadline
            {
                break;
            }
            steps_here += 1;
            st.guard += 1;
            if st.guard > 50_000_000 {
                return Err("too many steps".into());
            }
            if o.max_wall_time > 0.0
                && st.guard % 64 == 0
                && crate::backend::web_time_now() - wall0 > o.max_wall_time
            {
                return Err(format!(
                    "wall-clock limit of {} s reached at t = {}",
                    o.max_wall_time, t
                ));
            }
            let t_lim = match self.delay_events.first() {
                Some(&te) if te < t_stop => te,
                _ => t_stop,
            };
            let ext_te = self.ext_time_event().filter(|&te| te > t);
            let t_lim = match ext_te {
                Some(te) if te < t_lim => te,
                _ => t_lim,
            };
            // Steps end on the next sample() tick: the states at a time event are then
            // the step's own solution, not an interpolation across the tick.
            let t_lim = self.next_sample_time(t).map_or(t_lim, |ts| t_lim.min(ts));
            let t_lim = if n == 0 {
                t_lim.min(t + max_step)
            } else {
                t_lim
            };
            let s0 = self.prof.as_ref().map(|_| st.solver.stats());
            let tn = st.solver.step(&mut self.rt, t_lim)?;
            if let (Some(s0), Some(p)) = (s0, self.prof.as_deref_mut()) {
                p.on_step(&mut *st.solver, s0);
            }
            self.log.extend(st.solver.take_log());
            let tp = st.solver.t_prev();
            self.rt.error = None;
            // Event detection at the end of the step.
            let mut event_t: Option<f64> = None;
            if n_rel > 0 {
                self.rt.rhs(tn, st.solver.x(), &mut st.xi);
                self.rt.eval_zc();
                let mut crossing = Vec::new();
                for i in 0..n_rel {
                    let raw = self.rt.r[m.zc_raw[i] as usize];
                    let lat = self.rt.r[m.layout.rel0 + i];
                    let g = self.rt.r[m.zc_g[i] as usize].abs();
                    if g.is_finite() && g > self.zc_scale[i] {
                        self.zc_scale[i] = g;
                    }
                    // Hysteresis: a relation that sits on its switching point (a stuck
                    // friction element, a value from a Newton solve) must move clearly past
                    // it before it counts as crossing again; otherwise events chatter.
                    if raw != lat && !(g <= self.hysteresis(i)) {
                        crossing.push(i);
                    }
                }
                // A long step (a stuck friction element: the states do not move, the
                // driving torques do) can pass a relation on an algebraic variable twice,
                // there and back. Look at the output points inside the step as well.
                let mut t_hi = tn;
                if rec.is_some() {
                    let mut inner = Vec::new();
                    let mut k = st.k_out;
                    while next_out(k) < tn * (1.0 - 1e-14) && next_out(k) <= o.stop_time {
                        let to = next_out(k);
                        k += 1;
                        if to <= tp {
                            continue;
                        }
                        st.solver.interpolate(to, &mut st.xi);
                        self.rt.rhs(to, &st.xi, &mut st.x);
                        self.rt.eval_zc();
                        for i in 0..n_rel {
                            let raw = self.rt.r[m.zc_raw[i] as usize];
                            let lat = self.rt.r[m.layout.rel0 + i];
                            let g = self.rt.r[m.zc_g[i] as usize].abs();
                            if raw != lat && !(g <= self.hysteresis(i)) {
                                inner.push(i);
                            }
                        }
                        if !inner.is_empty() {
                            t_hi = to;
                            crossing = inner;
                            break;
                        }
                    }
                    if t_hi == tn && k > st.k_out {
                        self.rt.rhs(tn, st.solver.x(), &mut st.xi);
                    }
                }
                if !crossing.is_empty() {
                    let mut best = t_hi;
                    let after_event = tp - st.last_event <= 1e-10 * tp.abs().max(1.0);
                    let mut ticks = Vec::new();
                    for &i in &crossing {
                        // A sample() tick happens at its known time.
                        let te = match m.sample_rels.iter().position(|&r| r == i) {
                            Some(k) => {
                                let te = self.rt.r[m.sample_next[k] as usize].clamp(tp, tn);
                                ticks.push(te);
                                te
                            }
                            None => self.locate(&*st.solver, i, tp, best, after_event, &mut st.xi),
                        };
                        if te < best {
                            best = te;
                        }
                    }
                    // Ticks of different sample() calls that coincide up to round-off
                    // are one event, at the latest of them (where all are true).
                    if ticks.contains(&best) {
                        best = sample_cluster_end(best, ticks.into_iter());
                    }
                    event_t = Some(best);
                }
            }
            // Event indicators of the external components (imported FMUs).
            if !self.ext_z.is_empty() {
                if n_rel == 0 {
                    self.rt.rhs(tn, st.solver.x(), &mut st.xi);
                }
                let z = self.ext_indicators();
                let crossed = z
                    .iter()
                    .zip(&self.ext_z)
                    .any(|(&a, &b)| (b > 0.0 && a <= 0.0) || (b < 0.0 && a >= 0.0));
                if crossed {
                    let te = self.locate_ext(&*st.solver, tp, event_t.unwrap_or(tn), &mut st.xi);
                    event_t = Some(event_t.map_or(te, |e| e.min(te)));
                }
            }
            let mut time_event = false;
            if event_t.is_none()
                && let Some(&te) = self.delay_events.first()
                && tn >= te * (1.0 - 1e-14) - 1e-300
            {
                event_t = Some(tn);
                time_event = true;
            }
            if event_t.is_none()
                && let Some(te) = ext_te
                && tn >= te * (1.0 - 1e-14) - 1e-300
            {
                event_t = Some(tn);
                time_event = true;
            }
            if event_t.is_none() && self.rt.ext.is_some() {
                // FMI step events: the components may ask for an event after a step.
                self.rt.rhs(tn, st.solver.x(), &mut st.xi);
                if self
                    .with_ext(|h, rt| h.completed_step(rt))
                    .unwrap_or(Ok(false))?
                {
                    event_t = Some(tn);
                    time_event = true;
                } else {
                    self.ext_z = self.ext_indicators();
                }
                if let Some(Some(msg)) = self.rt.ext.as_ref().map(|h| h.terminated()) {
                    self.terminated.get_or_insert(msg);
                }
            }
            if let Some(te) = event_t {
                let k = self.delay_events.partition_point(|&x| x <= te);
                self.delay_events.drain(..k);
            }
            let t_end = event_t.unwrap_or(tn);
            // Output points inside the step (before the event).
            let t_out = self.prof.as_ref().map(|_| crate::profile::now());
            if let Some(rec) = rec.as_deref_mut() {
                while next_out(st.k_out) <= t_end * (1.0 + 1e-14) + 1e-300
                    && next_out(st.k_out) <= o.stop_time * (1.0 + 1e-14)
                {
                    let to = next_out(st.k_out).min(o.stop_time);
                    if event_t.is_some() && to >= t_end {
                        break;
                    }
                    st.solver.interpolate(to, &mut st.xi);
                    self.rt.rhs(to, &st.xi, &mut st.x);
                    rec.record(&mut self.rt);
                    self.record_delays();
                    self.check_asserts()?;
                    st.k_out += 1;
                }
            }
            if let (Some(t0), Some(p)) = (t_out, self.prof.as_deref_mut()) {
                p.output_secs += crate::profile::now() - t0;
            }
            if let Some(te) = event_t {
                let same = te - st.last_event <= 1e-10 * te.abs().max(1.0);
                if same {
                    st.same_instant += 1;
                    if st.same_instant > 500 {
                        let names: Vec<String> = (0..n_rel)
                            .filter(|&i| {
                                self.rt.r[m.zc_raw[i] as usize] != self.rt.r[m.layout.rel0 + i]
                            })
                            .take(4)
                            .map(|i| format!("`{}`", m.rel_text.get(i).map_or("?", |s| s.as_str())))
                            .collect();
                        return Err(format!(
                            "events chatter at t = {te}: {} keep switching back and forth",
                            if names.is_empty() {
                                "relations".to_string()
                            } else {
                                names.join(", ")
                            }
                        ));
                    }
                } else {
                    st.same_instant = 0;
                }
                st.last_event = te;
                self.stats.events += 1;
                if !time_event {
                    self.stats.state_events += 1;
                }
                // The row before the event sees the left values of delayed jumps.
                self.rt.delay_right = false;
                st.solver.interpolate(te, &mut st.xi);
                self.rt.rhs(te, &st.xi, &mut st.x);
                let snap = self.prof.as_deref_mut().map(|p| p.event_begin(&self.rt));
                // Replacing the right values of an event at the same instant adds no row,
                // so it is allowed (and not charged) once the budget is spent.
                let merge = same
                    && rec
                        .as_deref()
                        .is_some_and(|r| r.res.time.len() == st.last_event_rows);
                let store =
                    o.store_events && rec.is_some() && (merge || st.event_rows < event_budget);
                let mut merged_at = None;
                if store && let Some(rec) = rec.as_deref_mut() {
                    // An event right after another one at the same instant (a friction
                    // element starting to slide: `startForward` at `sa > tau0_max`, then
                    // `mode = Forward` as `w > 0` an instant later) continues it: the
                    // values in between hold for no time, so its rows replace the right
                    // values of the first one instead of adding a spike.
                    // They keep its time: both are the same event instant.
                    if merge {
                        merged_at = rec.pop();
                    } else {
                        rec.record(&mut self.rt);
                        st.event_rows += 2;
                    }
                    if !merge && st.event_rows >= event_budget {
                        self.log.push(format!(
                            "more than {} event points at t = {te}: later events are not stored \
                             (only output points)",
                            event_budget
                        ));
                    }
                }
                self.record_delays();
                self.rt.delay_right = true;
                self.event_iteration_ext(true)?;
                self.check_newton("event")?;
                self.record_delays();
                if let (Some(snap), Some(p)) = (snap, self.prof.as_deref_mut()) {
                    let trigger = if time_event {
                        crate::profile::Trigger::Time
                    } else {
                        crate::profile::Trigger::Crossing
                    };
                    p.event_end(snap, &self.rt, te, trigger);
                }
                if let Some(rec) = rec.as_deref_mut() {
                    if store {
                        rec.record(&mut self.rt);
                        if let (Some(t0), Some(t)) = (merged_at, rec.res.time.last_mut()) {
                            *t = t0;
                        }
                        st.last_event_rows = rec.res.time.len();
                    }
                    // Output point coinciding with the event.
                    if (next_out(st.k_out) - te).abs() <= 1e-12 * te.abs().max(1.0) {
                        if !store {
                            rec.record(&mut self.rt);
                        }
                        st.k_out += 1;
                    }
                }
                self.rt.get_states(&mut st.xi);
                t = te;
                st.solver.reset(&mut self.rt, t, &st.xi);
            } else {
                t = tn;
                if !m.delays.is_empty() {
                    let xs = st.solver.x().to_vec();
                    self.rt.rhs(tn, &xs, &mut st.xi);
                    self.record_delays();
                }
                if let Some(rec) = rec.as_deref_mut()
                    && (tn - o.stop_time).abs() <= 1e-12 * o.stop_time.abs().max(1.0)
                    && rec
                        .res
                        .time
                        .last()
                        .is_none_or(|&l| l < o.stop_time - 1e-12 * o.stop_time.abs().max(1.0))
                {
                    let xs = st.solver.x().to_vec();
                    self.rt.rhs(o.stop_time, &xs, &mut st.x);
                    rec.record(&mut self.rt);
                }
            }
            st.t = t;
            if let Some(e) = self.rt.error.take() {
                let b = &m.newton[e.block];
                self.newton_failure = Some(NewtonFailure {
                    block: e.block,
                    phase: "simulation".into(),
                    time: t,
                    residual: e.residual,
                });
                return Err(format!(
                    "nonlinear system for [{}] did not converge at t = {} (residual {:.3e})",
                    b.names.join(", "),
                    t,
                    e.residual
                ));
            }
        }
        st.t = t;
        Ok(())
    }

    /// A time event exactly at the stop time (`sample(0, T)` with StopTime a multiple of
    /// T, as in the sampled mean of a harmonic analysis) sits on its switching point
    /// when the last step ends, inside the hysteresis band of the crossing test. It is
    /// still an event of the run: process it and store its right value.
    fn stop_time_event(&mut self, st: &mut Stepper, rec: &mut Recorder) -> Result<(), String> {
        if self.endpoint_time_event(st, self.opts.stop_time)? {
            rec.record(&mut self.rt);
        }
        Ok(())
    }

    /// Process a time event (`sample()`, a relation on `time`) due exactly at `t_end`,
    /// where the last step of [`Simulation::advance`] ended; returns whether one fired.
    /// The relations are latched to their right values, so a later step from `t_end`
    /// does not see the event again.
    fn endpoint_time_event(&mut self, st: &mut Stepper, t_end: f64) -> Result<bool, String> {
        let m = self.rt.m;
        let n_rel = m.layout.n_rel;
        let t = st.t;
        if self.terminated.is_some()
            || (n_rel == 0 && self.rt.ext.is_none())
            || (t - t_end).abs() > 1e-12 * t_end.abs().max(1.0)
        {
            return Ok(false);
        }
        let xs = st.solver.x().to_vec();
        self.rt.rhs(t_end, &xs, &mut st.xi);
        self.rt.eval_zc();
        let due = (0..n_rel).any(|i| {
            (m.sample_rels.contains(&i) || mentions_time(&m.rel_text[i]))
                && self.rt.r[m.zc_raw[i] as usize] != self.rt.r[m.layout.rel0 + i]
        }) || self
            .ext_time_event()
            .is_some_and(|te| (te - t_end).abs() <= 1e-12 * t_end.abs().max(1.0));
        if due {
            self.stats.events += 1;
            st.last_event = t_end;
            self.rt.delay_right = true;
            let snap = self.prof.as_deref_mut().map(|p| p.event_begin(&self.rt));
            self.event_iteration_ext(true)?;
            self.check_newton("event")?;
            self.record_delays();
            if let (Some(snap), Some(p)) = (snap, self.prof.as_deref_mut()) {
                p.event_end(snap, &self.rt, t_end, crate::profile::Trigger::Time);
            }
        }
        Ok(due)
    }

    fn check_asserts(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        if m.asserts.is_empty() {
            return Ok(());
        }
        self.rt.run(&m.asserts_prog);
        for (s, msg) in &m.asserts {
            if self.rt.r[*s as usize] == 0.0 {
                return Err(format!("assertion failed at t = {}: {msg}", self.rt.time()));
            }
        }
        Ok(())
    }

    /// Locate the first time in `(ta, tb]` where relation `i` changes its raw value.
    /// Width of the hysteresis band of relation `i`.
    fn hysteresis(&self, i: usize) -> f64 {
        (1e-10 * self.zc_scale[i]).max(1e-14)
    }

    fn locate(
        &mut self,
        solver: &dyn Integrator,
        i: usize,
        ta: f64,
        tb: f64,
        after_event: bool,
        xi: &mut Vec<f64>,
    ) -> f64 {
        let m = self.rt.m;
        let lat = self.rt.r[m.layout.rel0 + i];
        let n = xi.len();
        let mut dx = vec![0.0; n];
        let mut eval = |this: &mut Self, t: f64| -> (f64, f64) {
            solver.interpolate(t, xi);
            this.rt.rhs(t, xi, &mut dx);
            this.rt.eval_zc();
            (
                this.rt.r[m.zc_g[i] as usize],
                this.rt.r[m.zc_raw[i] as usize],
            )
        };
        let (mut a, mut b) = (ta, tb);
        let (mut ga, ra) = eval(self, a);
        let (mut gb, rb) = eval(self, b);
        if rb == lat {
            return tb; // no crossing at tb any more (should not happen)
        }
        if ra != lat {
            if after_event && ga.abs() <= self.hysteresis(i) {
                return tb; // still inside the hysteresis band right after an event
            }
            if after_event {
                // The event just handled here kept the relation's value although it is
                // (barely) past its switching point: the event equations put the system
                // on that point (a valve opening at zero flow). Handling the same instant
                // again would change nothing; the next event is where the crossing has
                // clearly progressed, |g| ten times larger.
                let target = (10.0 * ga.abs()).max(1e3 * self.hysteresis(i));
                if !(gb.abs() > target) {
                    return tb;
                }
                for _ in 0..100 {
                    if b - a <= 1e-13 * b.abs().max(1.0) {
                        break;
                    }
                    let c = 0.5 * (a + b);
                    let (gc, _) = eval(self, c);
                    if gc.abs() >= target {
                        b = c;
                    } else {
                        a = c;
                    }
                }
                return b;
            }
            // Already crossed at the start: right after an event, or a step that ended
            // just past the switching point (a sample instant hit exactly).
            return ta;
        }
        let mut side = 0;
        for _ in 0..100 {
            if b - a <= 1e-13 * b.abs().max(1.0) {
                break;
            }
            // Illinois regula falsi, falling back to bisection.
            let mut c = if ga != gb && ga.is_finite() && gb.is_finite() {
                b - gb * (b - a) / (gb - ga)
            } else {
                0.5 * (a + b)
            };
            if !(c > a && c < b) {
                c = 0.5 * (a + b);
            }
            let (gc, rc) = eval(self, c);
            if rc == lat {
                a = c;
                ga = gc;
                if side == -1 {
                    gb *= 0.5;
                }
                side = -1;
            } else {
                b = c;
                gb = gc;
                if side == 1 {
                    ga *= 0.5;
                }
                side = 1;
            }
        }
        b
    }
}

/// Integrator state of a [`Simulation`] being stepped (see [`Simulation::stepper`]).
pub struct Stepper {
    solver: Box<dyn Integrator>,
    x: Vec<f64>,
    xi: Vec<f64>,
    /// Time reached.
    pub t: f64,
    k_out: u64,
    dt: f64,
    max_step: f64,
    event_budget: usize,
    event_rows: usize,
    guard: u64,
    last_event: f64,
    /// Rows stored when the last event's right values were recorded.
    last_event_rows: usize,
    same_instant: u32,
    wall0: f64,
    /// Wall-clock time (`profile::now`) at which `advance` returns early (slices).
    deadline: f64,
}

impl Stepper {
    pub fn solver_stats(&self) -> SolverStats {
        self.solver.stats()
    }
}

impl Simulation<'_> {
    pub fn log_messages(&self) -> &[String] {
        &self.log
    }
}

/// A simulation run in slices, for live plotting and interactive dashboards: the caller
/// advances it to a time or for a wall-clock budget, reads the result rows recorded so far
/// between slices, and may change free Real parameters on the way.
///
/// Slices that end only on the wall-clock budget (`advance(stop_time, budget)`) take the
/// very same steps as [`simulate`], so the result is identical to a full run. Slices that
/// end on an intermediate time make the integrator stop there (paced runs).
pub struct Incremental<'m> {
    sim: Simulation<'m>,
    st: Stepper,
    rec: Recorder,
    order: Vec<usize>,
    finished: bool,
    error: Option<String>,
}

impl<'m> Incremental<'m> {
    /// Initialize the model and record the initial point.
    pub fn new(m: &'m Model, opts: SimOptions) -> Result<Self, SimError> {
        let mut sim = Simulation::new(m, opts);
        let (mut rec, order) = Recorder::new(m);
        let t_init = crate::profile::now();
        if let Err(e) = sim.initialize() {
            return Err(SimError {
                message: e,
                partial: None,
            });
        }
        sim.param_values_into(&mut rec, &order);
        sim.stats.init_ms = (crate::profile::now() - t_init) * 1000.0;
        let mut st = sim.make_stepper();
        rec.record(&mut sim.rt);
        sim.restart(&mut st);
        st.event_budget = sim.event_budget(&rec);
        Ok(Incremental {
            sim,
            st,
            rec,
            order,
            finished: false,
            error: None,
        })
    }

    /// Simulated time reached.
    pub fn time(&self) -> f64 {
        self.st.t
    }

    pub fn options(&self) -> &SimOptions {
        &self.sim.opts
    }

    /// Whether the run reached its stop time, was terminated, or failed.
    pub fn finished(&self) -> bool {
        self.finished
    }

    /// The error that ended the run, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The rows recorded so far.
    pub fn results(&self) -> &Results {
        &self.rec.res
    }

    /// Integrate to `t` (at most the stop time), or until `wall_budget` seconds of wall
    /// clock time are spent (0: no budget). Returns whether the run is finished.
    pub fn advance(&mut self, t: f64, wall_budget: f64) -> Result<bool, String> {
        if self.finished {
            return Ok(true);
        }
        let stop = self.sim.opts.stop_time;
        let target = t.min(stop);
        self.st.deadline = if wall_budget > 0.0 {
            crate::profile::now() + wall_budget
        } else {
            f64::INFINITY
        };
        let r = self.sim.advance(&mut self.st, target, Some(&mut self.rec));
        self.st.deadline = f64::INFINITY;
        let r = r.and_then(|()| {
            let reached = |x: f64, to: f64| x >= to * (1.0 - 1e-15) - 1e-300;
            if self.sim.terminated.is_some() || reached(self.st.t, stop) {
                self.sim.stop_time_event(&mut self.st, &mut self.rec)?;
                self.finished = true;
            } else if reached(self.st.t, target) {
                // A time event due exactly where this slice ends: handle it here, as
                // `advance_to` does, and store its right value.
                if self.sim.endpoint_time_event(&mut self.st, target)? {
                    if self.sim.opts.store_events && self.st.event_rows < self.st.event_budget {
                        self.rec.record(&mut self.sim.rt);
                        self.st.event_rows += 1;
                    }
                    self.sim.rt.get_states(&mut self.st.xi);
                    self.st.solver.reset(&mut self.sim.rt, target, &self.st.xi);
                }
            }
            Ok(())
        });
        if let Err(e) = r {
            self.finished = true;
            self.error = Some(e.clone());
            return Err(e);
        }
        Ok(self.finished)
    }

    /// Change free Real parameters (see [`crate::sweep::classify`]) at the current time,
    /// without restarting: parameters bound to them are evaluated again, the discrete
    /// values settle in an event iteration and the integrator restarts from the current
    /// states. Start values and `fixed = false` parameters keep what initialization gave
    /// them. Fails, changing nothing, when a name is not such a parameter.
    pub fn set_params(&mut self, params: &[(String, f64)]) -> Result<(), String> {
        if self.finished {
            return Err("the run is finished".into());
        }
        let m = self.sim.rt.m;
        for (n, _) in params {
            if crate::sweep::classify(m, n) != crate::sweep::ParamClass::Runtime {
                return Err(format!(
                    "'{n}' is not a free Real parameter: it can only change with a new run"
                ));
            }
        }
        for (n, v) in params {
            match self.sim.opts.params.iter_mut().find(|(k, _)| k == n) {
                Some(p) => p.1 = *v,
                None => self.sim.opts.params.push((n.clone(), *v)),
            }
        }
        // The consistent solution at the time reached (the registers hold whatever the
        // solver evaluated last).
        let t = self.st.t;
        if (self.st.solver.t() - t).abs() <= 1e-15 * t.abs().max(1.0) {
            self.st.x.copy_from_slice(self.st.solver.x());
        } else {
            self.st.solver.interpolate(t, &mut self.st.x);
        }
        self.sim.eval_params()?;
        let xs = self.st.x.clone();
        self.sim.rt.rhs(t, &xs, &mut self.st.xi);
        self.sim.rt.error = None;
        self.sim.param_values_into(&mut self.rec, &self.order);
        self.sim.handle_event()?;
        self.sim.restart(&mut self.st);
        Ok(())
    }

    /// End the run where it is: the result so far, the statistics and the error, if any.
    pub fn finish(mut self) -> (Results, SimStats, Option<String>) {
        let m = self.sim.rt.m;
        let s = &mut self.sim.stats;
        s.solver = self.st.solver.stats();
        s.newton_solves = self.sim.rt.newton_stats.solves;
        s.newton_iterations = self.sim.rt.newton_stats.iterations;
        s.output_points = self.rec.res.time.len();
        s.states = m.states.len();
        let stats = s.clone();
        self.rec.res.log.append(&mut self.sim.log);
        if let Some(msg) = &self.sim.terminated {
            self.rec.res.log.push(format!("terminated: {msg}"));
        }
        (self.rec.res, stats, self.error)
    }
}

/// Convenience: simulate a compiled model with options.
/// The latest of the tick times `ticks` that coincide with `t0` up to round-off.
fn sample_cluster_end(t0: f64, ticks: impl Iterator<Item = f64>) -> f64 {
    let tol = 1e-12 * t0.abs().max(1e-3);
    ticks
        .filter(|&ts| ts >= t0 && ts - t0 <= tol)
        .fold(t0, f64::max)
}

pub fn simulate(m: &Model, opts: SimOptions) -> Result<(Results, SimStats), SimError> {
    Simulation::new(m, opts).run()
}

/// Simulate and, with `opts.profile`, return the performance profile and event log.
pub fn simulate_profiled(
    m: &Model,
    opts: SimOptions,
) -> (
    Result<(Results, SimStats), SimError>,
    Option<crate::profile::Profile>,
) {
    Simulation::new(m, opts).run_profiled()
}

/// Whether a relation's text reads `time` as a whole word, on either side (`time >= T`,
/// `1 <= time`), as opposed to a name that only contains it (`timer.y`, `t.time0`).
pub(crate) fn mentions_time(text: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.';
    text.match_indices("time").any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(ident)
            && !text[i + 4..].chars().next().is_some_and(ident)
    })
}

/// Right-hand side `der(x) = f(t, x, u)` of a model evaluated at given points (for
/// hybrid and data-driven modelling: the known physics at measured states). Parameters
/// take their values (with `params` overrides), discrete variables their start values;
/// relations are settled at each point. `inputs` holds one column per named top-level
/// input (or parameter) with one value per point. Returns, per point, the state
/// derivatives (in [`Model::states`] order) and the values of `read`.
pub fn eval_rhs(
    m: &Model,
    params: &[(String, f64)],
    t: &[f64],
    states: &[Vec<f64>],
    inputs: &[(String, Vec<f64>)],
    read: &[String],
) -> Result<(Vec<Vec<f64>>, Vec<Vec<f64>>), String> {
    let n = t.len();
    if states.len() != n {
        return Err(format!("{} time points but {} state rows", n, states.len()));
    }
    let find = |name: &str| -> Result<usize, String> {
        m.vars
            .iter()
            .position(|v| v.name == name)
            .ok_or_else(|| format!("unknown variable '{name}'"))
    };
    let opts = SimOptions {
        params: params.to_vec(),
        ..SimOptions::from_model(m)
    };
    let mut sim = Simulation::new(m, opts);
    sim.eval_params()?;
    sim.rt.set_time(t.first().copied().unwrap_or(0.0));
    sim.rt.run(&m.starts);
    sim.rt.save_pre();
    let input_ids: Vec<(usize, &Vec<f64>)> = inputs
        .iter()
        .map(|(name, col)| {
            let v = find(name)?;
            if col.len() != n {
                return Err(format!(
                    "input '{name}' has {} values for {n} points",
                    col.len()
                ));
            }
            Ok((v, col))
        })
        .collect::<Result<_, String>>()?;
    let read_ids: Vec<usize> = read
        .iter()
        .map(|r| find(r))
        .collect::<Result<_, String>>()?;
    let nx = m.states.len();
    let mut der = Vec::with_capacity(n);
    let mut vals = Vec::with_capacity(n);
    for k in 0..n {
        if states[k].len() != nx {
            return Err(format!(
                "point {k}: {} states given, the model has {nx}",
                states[k].len()
            ));
        }
        for (v, col) in &input_ids {
            sim.rt.r[*v] = col[k];
        }
        sim.rt.set_time(t[k]);
        sim.rt.set_states(&states[k]);
        sim.rt.run(&m.ode);
        for _ in 0..5 {
            if !sim.rt.latch_relations() {
                break;
            }
            sim.rt.run(&m.ode);
        }
        if let Some(e) = sim.rt.error.take() {
            return Err(format!(
                "point {k} (t = {}): nonlinear system for [{}] did not converge",
                t[k],
                m.newton[e.block].names.join(", ")
            ));
        }
        sim.rt.run(&m.outputs);
        let mut dx = vec![0.0; nx];
        sim.rt.get_derivs(&mut dx);
        der.push(dx);
        vals.push(read_ids.iter().map(|&v| sim.rt.r[v]).collect());
    }
    Ok((der, vals))
}
