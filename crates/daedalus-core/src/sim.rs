//! Simulation driver: initialization, integration, event detection and
//! iteration, and result recording.

use crate::backend::AliasKind;
use crate::codegen::{Action, Model, Runtime};
use crate::expr::VarId;
use crate::flat::VarKind;
use crate::results::{ResultKind, ResultVar, Results};
use crate::solver::{Bdf, Dopri5, FixedMethod, FixedStep, Integrator, SolverStats};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    /// Variable-order BDF (stiff; default, like Dassl in Dymola).
    Bdf,
    /// Dormand–Prince 5(4).
    Dopri5,
    Rk4,
    Euler,
}

impl Method {
    pub fn parse(s: &str) -> Option<Method> {
        Some(match s.to_ascii_lowercase().as_str() {
            "bdf" | "dassl" | "ida" | "cvode" | "lsodar" | "radau" | "esdirk45a" => Method::Bdf,
            "dopri5" | "dopri45" | "dopri" | "rkfix" | "rk45" => Method::Dopri5,
            "rk4" | "rkfix4" => Method::Rk4,
            "euler" => Method::Euler,
            _ => return None,
        })
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
    /// Parameter overrides by name.
    pub params: Vec<(String, f64)>,
    /// Abort after this many seconds of wall-clock time (0 = no limit).
    #[serde(default)]
    pub max_wall_time: f64,
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
            params: Vec::new(),
            max_wall_time: 0.0,
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
                p[v as usize] = true;
            }
            p
        };
        let hidden = |name: &str| name.starts_with('$');
        for (i, v) in m.vars.iter().enumerate() {
            let id = i as VarId;
            if is_param[i] || hidden(&v.name) {
                continue;
            }
            if let Some(AliasKind::Var { .. }) = alias_of.get(&id) {
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
        for (wi, w) in m.whens.iter().enumerate() {
            for (bi, acts) in w.actions.iter().enumerate() {
                if conds[wi][bi] && !prev[wi][bi] {
                    any = true;
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

    /// Event iteration at the current time until discrete values settle.
    fn event_iteration(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        for _ in 0..100 {
            self.rt.save_pre();
            let rel_changed = self.rt.latch_relations();
            self.rt.run(&m.ode);
            let conds = self.when_conds();
            let prev = std::mem::take(&mut self.cond_prev);
            let fired = self.fire(&conds, &prev);
            self.cond_prev = conds;
            // Recompute algebraics with the new discrete values.
            self.rt.run(&m.ode);
            let disc_changed = m.discretes.iter().any(|&d| {
                let a = self.rt.r[d as usize];
                let b = self.rt.r[m.layout.pre0 + d as usize];
                a != b && !(a.is_nan() && b.is_nan())
            });
            if !rel_changed && !fired && !disc_changed {
                self.rt.save_pre();
                return Ok(());
            }
        }
        Err(format!(
            "event iteration did not converge at t = {}",
            self.rt.time()
        ))
    }

    fn check_newton(&mut self, phase: &str) -> Result<(), String> {
        if let Some(e) = self.rt.error.take() {
            let b = &self.rt.m.newton[e.block];
            return Err(format!(
                "{phase}: nonlinear system for [{}] did not converge at t = {} (residual {:.3e})",
                b.names.join(", "),
                self.rt.time(),
                e.residual
            ));
        }
        Ok(())
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        let m = self.rt.m;
        let t0 = self.opts.start_time;
        self.eval_params()?;
        self.rt.set_time(t0);
        self.rt.r[m.layout.initial] = 1.0;
        self.rt.run(&m.starts);
        self.rt.save_pre();
        self.rt.run(&m.init);
        self.check_newton("initialization")?;
        self.rt.latch_relations();
        for &r in &m.sample_rels {
            self.rt.r[m.layout.rel0 + r] = 0.0;
        }
        self.rt.run(&m.ode);
        // when-clauses activated by initial()
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
        self.cond_prev = c0;
        self.rt.r[m.layout.initial] = 0.0;
        let _ = fired;
        // Event iteration at the start time (handles sample() at t0 and initial() clauses).
        self.event_iteration()?;
        self.check_newton("initialization")?;
        Ok(())
    }

    /// Run the whole simulation.
    pub fn run(mut self) -> Result<(Results, SimStats), SimError> {
        let m = self.rt.m;
        let (mut rec, order) = Recorder::new(m);
        let t_init = crate::backend::web_time_now();
        if let Err(e) = self.initialize() {
            return Err(SimError {
                message: e,
                partial: None,
            });
        }
        // Parameter values into the result.
        for (k, &i) in order.iter().enumerate() {
            if let ResultKind::Param(_) = rec.res.vars[k].kind {
                rec.res.vars[k].kind = ResultKind::Param(self.rt.r[i]);
            }
        }
        let t1 = crate::backend::web_time_now();
        self.stats.init_ms = (t1 - t_init) * 1000.0;
        let r = self.integrate(&mut rec);
        self.stats.sim_ms = (crate::backend::web_time_now() - t1) * 1000.0;
        self.stats.newton_solves = self.rt.newton_stats.solves;
        self.stats.newton_iterations = self.rt.newton_stats.iterations;
        self.stats.output_points = rec.res.time.len();
        self.stats.states = m.states.len();
        rec.res.log.append(&mut self.log);
        if let Some(msg) = &self.terminated {
            rec.res.log.push(format!("terminated: {msg}"));
        }
        match r {
            Ok(()) => Ok((rec.res, self.stats)),
            Err(e) => Err(SimError {
                message: e,
                partial: Some(rec.res),
            }),
        }
    }

    fn integrate(&mut self, rec: &mut Recorder) -> Result<(), String> {
        let m = self.rt.m;
        let o = self.opts.clone();
        let n = m.states.len();
        let span = o.stop_time - o.start_time;
        let dt = if o.interval > 0.0 {
            o.interval
        } else {
            span / o.intervals.max(1) as f64
        };
        let max_step = if o.max_step > 0.0 {
            o.max_step
        } else {
            span.max(1e-12)
        };
        let rtol = o.tolerance;
        let atol = o.tolerance;
        let mut solver: Box<dyn Integrator> = match o.method {
            Method::Bdf => {
                let b = Bdf::new(n, rtol, atol, max_step, &m.jac_pattern);
                self.stats.jacobian_colors = b.n_colors();
                Box::new(b)
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
        let mut x = vec![0.0; n];
        self.rt.get_states(&mut x);
        let mut t = o.start_time;
        rec.record(&mut self.rt);
        let mut k_out: u64 = 1;
        let next_out = |k: u64| o.start_time + k as f64 * dt;
        solver.reset(&mut self.rt, t, &x);
        let n_rel = m.layout.n_rel;
        let mut xi = vec![0.0; n];
        let mut guard = 0u64;
        // Events piling up at one instant (Zeno behaviour, chattering modes).
        let (mut last_event, mut same_instant) = (f64::NEG_INFINITY, 0u32);
        let wall0 = crate::backend::web_time_now();
        while t < o.stop_time * (1.0 - 1e-15) - 1e-300 && self.terminated.is_none() {
            guard += 1;
            if guard > 50_000_000 {
                return Err("too many steps".into());
            }
            if o.max_wall_time > 0.0
                && guard % 64 == 0
                && crate::backend::web_time_now() - wall0 > o.max_wall_time
            {
                return Err(format!(
                    "wall-clock limit of {} s reached at t = {}",
                    o.max_wall_time, t
                ));
            }
            let tn = solver.step(&mut self.rt, o.stop_time)?;
            let tp = solver.t_prev();
            self.rt.error = None;
            // Event detection at the end of the step.
            let mut event_t: Option<f64> = None;
            if n_rel > 0 {
                self.rt.rhs(tn, solver.x(), &mut xi);
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
                if !crossing.is_empty() {
                    let mut best = tn;
                    for &i in &crossing {
                        let te = self.locate(&*solver, i, tp, best, &mut xi);
                        if te < best {
                            best = te;
                        }
                    }
                    event_t = Some(best);
                }
            }
            let t_end = event_t.unwrap_or(tn);
            // Output points inside the step (before the event).
            while next_out(k_out) <= t_end * (1.0 + 1e-14) + 1e-300
                && next_out(k_out) <= o.stop_time * (1.0 + 1e-14)
            {
                let to = next_out(k_out).min(o.stop_time);
                if event_t.is_some() && to >= t_end {
                    break;
                }
                solver.interpolate(to, &mut xi);
                self.rt.rhs(to, &xi, &mut x);
                rec.record(&mut self.rt);
                self.check_asserts()?;
                k_out += 1;
            }
            if let Some(te) = event_t {
                if te - last_event <= 1e-10 * te.abs().max(1.0) {
                    same_instant += 1;
                    if same_instant > 500 {
                        let names: Vec<String> = (0..n_rel)
                            .filter(|&i| {
                                self.rt.r[m.zc_raw[i] as usize] != self.rt.r[m.layout.rel0 + i]
                            })
                            .take(4)
                            .map(|i| format!("relation {i}"))
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
                    same_instant = 0;
                }
                last_event = te;
                self.stats.events += 1;
                self.stats.state_events += 1;
                solver.interpolate(te, &mut xi);
                self.rt.rhs(te, &xi, &mut x);
                if o.store_events {
                    rec.record(&mut self.rt);
                }
                self.event_iteration()?;
                self.check_newton("event")?;
                if o.store_events {
                    rec.record(&mut self.rt);
                }
                // Output point coinciding with the event.
                if (next_out(k_out) - te).abs() <= 1e-12 * te.abs().max(1.0) {
                    if !o.store_events {
                        rec.record(&mut self.rt);
                    }
                    k_out += 1;
                }
                self.rt.get_states(&mut xi);
                t = te;
                solver.reset(&mut self.rt, t, &xi);
            } else {
                t = tn;
                if (tn - o.stop_time).abs() <= 1e-12 * o.stop_time.abs().max(1.0)
                    && rec
                        .res
                        .time
                        .last()
                        .is_none_or(|&l| l < o.stop_time - 1e-12 * o.stop_time.abs().max(1.0))
                {
                    let xs = solver.x().to_vec();
                    self.rt.rhs(o.stop_time, &xs, &mut x);
                    rec.record(&mut self.rt);
                }
            }
            if let Some(e) = self.rt.error.take() {
                let b = &m.newton[e.block];
                return Err(format!(
                    "nonlinear system for [{}] did not converge at t = {} (residual {:.3e})",
                    b.names.join(", "),
                    t,
                    e.residual
                ));
            }
        }
        self.stats.solver = solver.stats();
        Ok(())
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
            if ga.abs() <= self.hysteresis(i) {
                return tb; // still inside the hysteresis band at the start
            }
            return ta; // already crossed at the start (e.g. right after an event)
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

impl Simulation<'_> {
    pub fn log_messages(&self) -> &[String] {
        &self.log
    }
}

/// Convenience: simulate a compiled model with options.
pub fn simulate(m: &Model, opts: SimOptions) -> Result<(Results, SimStats), SimError> {
    Simulation::new(m, opts).run()
}
