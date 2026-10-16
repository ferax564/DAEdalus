//! Bound-constrained quasi-Newton minimization: projected L-BFGS with an active set
//! (a simple relative of L-BFGS-B). Variables at a bound whose gradient pushes outward
//! are held; the two-loop recursion acts on the free ones, and a backtracking Armijo
//! search runs along the projected path `P(x + a d)`.
//!
//! Each iteration costs one gradient (an adjoint solve where it applies) and one or a
//! few objective evaluations, so [`iterate`] is driven step by step by the optimizers'
//! jobs.

use std::collections::VecDeque;

/// Limited memory of the curvature pairs `(s, y)`.
#[derive(Clone, Debug, Default)]
pub struct Memory {
    pairs: VecDeque<(Vec<f64>, Vec<f64>)>,
    size: usize,
}

impl Memory {
    pub fn new(size: usize) -> Memory {
        Memory {
            pairs: VecDeque::new(),
            size: size.max(1),
        }
    }

    pub fn clear(&mut self) {
        self.pairs.clear();
    }

    fn push(&mut self, s: Vec<f64>, y: Vec<f64>) {
        let sy: f64 = s.iter().zip(&y).map(|(a, b)| a * b).sum();
        let ns = s.iter().map(|a| a * a).sum::<f64>().sqrt();
        let ny = y.iter().map(|a| a * a).sum::<f64>().sqrt();
        if sy > 1e-12 * ns * ny && sy.is_finite() {
            if self.pairs.len() == self.size {
                self.pairs.pop_front();
            }
            self.pairs.push_back((s, y));
        }
    }

    /// `-H g` restricted to the free variables (two-loop recursion).
    fn direction(&self, g: &[f64], free: &[bool]) -> Vec<f64> {
        let mask = |v: &[f64]| -> Vec<f64> {
            v.iter()
                .zip(free)
                .map(|(&a, &f)| if f { a } else { 0.0 })
                .collect()
        };
        let dot = |a: &[f64], b: &[f64]| -> f64 { a.iter().zip(b).map(|(x, y)| x * y).sum() };
        let mut q = mask(g);
        let mut alpha = Vec::with_capacity(self.pairs.len());
        let pairs: Vec<(Vec<f64>, Vec<f64>)> = self
            .pairs
            .iter()
            .map(|(s, y)| (mask(s), mask(y)))
            .filter(|(s, y)| dot(s, y) > 0.0)
            .collect();
        for (s, y) in pairs.iter().rev() {
            let rho = 1.0 / dot(y, s);
            let a = rho * dot(s, &q);
            for (qi, yi) in q.iter_mut().zip(y) {
                *qi -= a * yi;
            }
            alpha.push((a, rho));
        }
        if let Some((s, y)) = pairs.last() {
            let gamma = dot(s, y) / dot(y, y);
            q.iter_mut().for_each(|x| *x *= gamma);
        }
        for ((s, y), (a, rho)) in pairs.iter().zip(alpha.iter().rev()) {
            let b = rho * dot(y, &q);
            for (qi, si) in q.iter_mut().zip(s) {
                *qi += (a - b) * si;
            }
        }
        q.iter().map(|x| -x).collect()
    }
}

fn project(x: &mut [f64], lo: &[f64], hi: &[f64]) {
    for i in 0..x.len() {
        x[i] = x[i].max(lo[i]).min(hi[i]);
    }
}

/// Variables not held at a bound by the gradient.
pub fn free_set(x: &[f64], g: &[f64], lo: &[f64], hi: &[f64]) -> Vec<bool> {
    (0..x.len())
        .map(|i| !((x[i] <= lo[i] && g[i] > 0.0) || (x[i] >= hi[i] && g[i] < 0.0)))
        .collect()
}

/// Largest component of the projected gradient.
pub fn projected_gradient(x: &[f64], g: &[f64], lo: &[f64], hi: &[f64]) -> f64 {
    let free = free_set(x, g, lo, hi);
    (0..x.len())
        .filter(|&i| free[i])
        .map(|i| g[i].abs())
        .fold(0.0, f64::max)
}

/// What an iteration did.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Moved to a lower value: the relative decrease and the largest step component.
    Moved { decrease: f64, step: f64 },
    /// No decrease along the quasi-Newton direction: the memory was dropped, and the
    /// next iteration tries steepest descent.
    Restart,
    /// The projected gradient vanishes.
    Stationary,
    /// No step along the search direction lowers the value.
    NoDecrease,
}

/// One iteration from `(x, f, g)`, updating them in place. `eval_f` returns the value
/// at a point (infinite where it cannot be evaluated), `eval_g` the gradient.
#[allow(clippy::too_many_arguments)]
pub fn iterate(
    x: &mut Vec<f64>,
    f: &mut f64,
    g: &mut Vec<f64>,
    lo: &[f64],
    hi: &[f64],
    mem: &mut Memory,
    gtol: f64,
    eval_f: &mut dyn FnMut(&[f64]) -> f64,
    eval_g: &mut dyn FnMut(&[f64]) -> Result<Vec<f64>, String>,
) -> Result<Outcome, String> {
    let n = x.len();
    let free = free_set(x, g, lo, hi);
    let pg = (0..n)
        .filter(|&i| free[i])
        .map(|i| g[i].abs())
        .fold(0.0, f64::max);
    if pg <= gtol {
        return Ok(Outcome::Stationary);
    }
    let mut d = mem.direction(g, &free);
    let slope: f64 = d.iter().zip(g.iter()).map(|(a, b)| a * b).sum();
    if !(slope < 0.0) || d.iter().any(|v| !v.is_finite()) {
        mem.clear();
        d = (0..n).map(|i| if free[i] { -g[i] } else { 0.0 }).collect();
    }
    // First step of a steepest descent: at most 1/4 of the range (or of |x|) per variable.
    let mut a = 1.0;
    if mem.pairs.is_empty() {
        let dmax = (0..n)
            .map(|i| {
                let span = if (hi[i] - lo[i]).is_finite() {
                    hi[i] - lo[i]
                } else {
                    x[i].abs().max(1.0)
                };
                d[i].abs() / span
            })
            .fold(0.0, f64::max);
        if dmax > 0.25 {
            a = 0.25 / dmax;
        }
    }
    for _ in 0..30 {
        let mut xn: Vec<f64> = (0..n).map(|i| x[i] + a * d[i]).collect();
        project(&mut xn, lo, hi);
        let decrease_lin: f64 = (0..n).map(|i| g[i] * (xn[i] - x[i])).sum();
        let step = (0..n).map(|i| (xn[i] - x[i]).abs()).fold(0.0, f64::max);
        if step == 0.0 {
            break;
        }
        let fnew = eval_f(&xn);
        if fnew.is_finite() && fnew <= *f + 1e-4 * decrease_lin {
            let gn = eval_g(&xn)?;
            let s: Vec<f64> = (0..n).map(|i| xn[i] - x[i]).collect();
            let y: Vec<f64> = (0..n).map(|i| gn[i] - g[i]).collect();
            mem.push(s, y);
            let decrease = (*f - fnew) / f.abs().max(fnew.abs()).max(1e-300);
            *x = xn;
            *f = fnew;
            *g = gn;
            return Ok(Outcome::Moved { decrease, step });
        }
        a *= if fnew.is_finite() { 0.5 } else { 0.25 };
    }
    if !mem.pairs.is_empty() {
        // retry from steepest descent next time
        mem.clear();
        return Ok(Outcome::Restart);
    }
    Ok(Outcome::NoDecrease)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_rosenbrock() {
        let fv = |x: &[f64]| (1.0 - x[0]).powi(2) + 100.0 * (x[1] - x[0] * x[0]).powi(2);
        let gv = |x: &[f64]| {
            vec![
                -2.0 * (1.0 - x[0]) - 400.0 * x[0] * (x[1] - x[0] * x[0]),
                200.0 * (x[1] - x[0] * x[0]),
            ]
        };
        // unconstrained minimum (1, 1) inside the box
        let (lo, hi) = (vec![-2.0, -2.0], vec![2.0, 2.0]);
        let mut x = vec![-1.2, 1.0];
        let mut f = fv(&x);
        let mut g = gv(&x);
        let mut mem = Memory::new(8);
        for _ in 0..200 {
            let o = iterate(
                &mut x,
                &mut f,
                &mut g,
                &lo,
                &hi,
                &mut mem,
                1e-10,
                &mut |p| fv(p),
                &mut |p| Ok(gv(p)),
            )
            .unwrap();
            if matches!(o, Outcome::Stationary | Outcome::NoDecrease) {
                break;
            }
        }
        assert!(
            (x[0] - 1.0).abs() < 1e-5 && (x[1] - 1.0).abs() < 1e-5,
            "{x:?}"
        );
        // the bound x1 <= 0.5 is active at the solution
        let hi = vec![2.0, 0.5];
        let mut x = vec![-1.2, 0.4];
        let mut f = fv(&x);
        let mut g = gv(&x);
        let mut mem = Memory::new(8);
        for _ in 0..300 {
            let o = iterate(
                &mut x,
                &mut f,
                &mut g,
                &lo,
                &hi,
                &mut mem,
                1e-10,
                &mut |p| fv(p),
                &mut |p| Ok(gv(p)),
            )
            .unwrap();
            if matches!(o, Outcome::Stationary | Outcome::NoDecrease) {
                break;
            }
        }
        // min of (1 - a)^2 + 100 (0.5 - a^2)^2 near a = 0.7071
        assert!((x[1] - 0.5).abs() < 1e-12, "{x:?}");
        assert!((x[0] - 0.70786).abs() < 1e-3, "{x:?}");
    }
}
