//! Simulation performance analyzer and event log (Dymola's "Simulation Analysis",
//! Amesim's Performance Analyzer).
//!
//! Profiling is opt-in ([`crate::sim::SimOptions::profile`]); without it none of the
//! hooks below run and a simulation takes exactly the path it always did. With it:
//!
//! * **Step diagnostics.** Per accepted step the time, step size, order and the scaled
//!   local error estimate per state; per rejected attempt its time, size and reason. An
//!   integrator supplies the error estimate through [`StepProbe`]
//!   ([`crate::solver::Integrator::probe`]); one without a probe still gets its steps
//!   and rejection counts recorded from its statistics. From the estimates: which state
//!   was the largest contributor to the error norm, per step and in total ("which
//!   states limit the step size").
//! * **Time per block.** Every Newton block (algebraic loop) and loop-program call of the
//!   tape is timed with a cheap clock ([`now`]), with its calls, iterations and
//!   failures; the rest of a derivative evaluation is reported as the explicit
//!   equations, and what is left of the integration time as solver overhead.
//! * **Event log.** Each event with its time, trigger, the relations that switched (with
//!   their source text and component), the when-clauses that fired and the values that
//!   changed; per relation its count, first and last time and shortest interval, and
//!   chattering: at least [`CHATTER_COUNT`] switches of one relation each within
//!   [`CHATTER_WINDOW`] of the run divided by that count of the previous one.
//!
//! [`Profiler::finish`] turns the record into a serializable [`Profile`] with a summary
//! and plain-language hints.

use crate::codegen::{Action, Model, NewtonStats, Runtime};
use crate::expr::Cmp;
use crate::sim::SimStats;
use crate::solver::{Integrator, SolverStats};
use serde::Serialize;
use std::sync::OnceLock;

/// Chattering: at least this many switches of one relation ...
pub const CHATTER_COUNT: usize = 20;
/// ... with consecutive switches closer than this fraction of the simulated interval
/// divided by [`CHATTER_COUNT`].
pub const CHATTER_WINDOW: f64 = 0.01;
/// Most step records kept (later steps still count in the totals).
const MAX_STEPS: usize = 2_000_000;
/// Most per-state error values kept (steps x states).
const MAX_ERRS: usize = 4_000_000;
/// Most events kept in the log (later events still count per relation).
const MAX_EVENTS: usize = 100_000;
/// Most switching times kept per relation for the chattering analysis.
const MAX_REL_TIMES: usize = 200_000;
/// Most changed values listed per event.
const MAX_CHANGES: usize = 12;

// ----------------------------------------------------------------------------
// Clock
// ----------------------------------------------------------------------------

static CLOCK: OnceLock<fn() -> f64> = OnceLock::new();

/// Use `f` (seconds, any origin) as the profiling clock: the WebAssembly build passes
/// `performance.now() / 1000`. Natively the default is `std::time::Instant`.
pub fn set_clock(f: fn() -> f64) {
    let _ = CLOCK.set(f);
}

/// Profiling clock in seconds since an arbitrary origin.
#[inline]
pub fn now() -> f64 {
    if let Some(f) = CLOCK.get() {
        return f();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        EPOCH
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

// ----------------------------------------------------------------------------
// Integrator probe
// ----------------------------------------------------------------------------

/// Per-step diagnostics an integrator records while profiling is on (see
/// [`crate::solver::Integrator::probe`]). The integrator only writes into it when
/// `enabled`.
#[derive(Clone, Debug, Default)]
pub struct StepProbe {
    pub enabled: bool,
    /// Scaled local error estimate per state of the last accepted step,
    /// `err_i / (atol + rtol |x_i|)`, the terms of the error norm.
    pub err: Vec<f64>,
    /// `err` holds the estimate of the step just accepted.
    pub has_err: bool,
    /// Order of the method in the last accepted step.
    pub order: u32,
    /// Rejected attempts since the profiler last looked: `(t, h, Newton failure)`.
    pub rejected: Vec<(f64, f64, bool)>,
}

impl StepProbe {
    pub fn enable(&mut self, n_states: usize) {
        self.enabled = true;
        self.err = vec![0.0; n_states];
        self.has_err = false;
        self.rejected.clear();
    }

    /// A rejected attempt of size `h` from `t` (Newton failure or error test).
    #[inline]
    pub fn reject(&mut self, t: f64, h: f64, newton: bool) {
        if self.enabled {
            self.rejected.push((t, h, newton));
        }
    }

    /// The step just accepted had order `order`, with its error terms in `err`.
    #[inline]
    pub fn accept(&mut self, order: u32) {
        if self.enabled {
            self.has_err = true;
            self.order = order;
        }
    }
}

// ----------------------------------------------------------------------------
// Block timing (kept in the runtime)
// ----------------------------------------------------------------------------

/// Calls, Newton iterations, failures and time of one block of the tape.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct BlockAgg {
    pub calls: u64,
    pub iterations: u64,
    pub failures: u64,
    pub secs: f64,
}

/// Timing of the tape's blocks, filled by [`Runtime`] while `rt.prof` is set.
#[derive(Clone, Debug, Default)]
pub struct BlockProf {
    /// Per Newton block.
    pub newton: Vec<BlockAgg>,
    /// Per loop-program call site.
    pub sites: Vec<BlockAgg>,
    /// Derivative evaluations (`Runtime::rhs`) and their time, blocks included.
    pub rhs: BlockAgg,
    /// Time of the outermost blocks run inside derivative evaluations.
    pub nested_in_rhs: f64,
    depth: u32,
    in_rhs: u32,
}

impl BlockProf {
    pub fn new(m: &Model) -> Self {
        BlockProf {
            newton: vec![BlockAgg::default(); m.newton.len()],
            sites: vec![BlockAgg::default(); m.sites.len()],
            ..Default::default()
        }
    }

    /// Entering a timed block; returns its start time.
    #[inline]
    pub fn enter(&mut self) -> f64 {
        self.depth += 1;
        now()
    }

    #[inline]
    fn leave(&mut self, t0: f64) -> f64 {
        let dt = now() - t0;
        self.depth -= 1;
        if self.depth == 0 && self.in_rhs > 0 {
            self.nested_in_rhs += dt;
        }
        dt
    }

    /// Newton block `bi` done: `before` and `after` are the runtime's Newton counters.
    pub fn leave_newton(&mut self, bi: usize, t0: f64, before: NewtonStats, after: NewtonStats) {
        let dt = self.leave(t0);
        if let Some(b) = self.newton.get_mut(bi) {
            b.calls += 1;
            b.iterations += after.iterations.saturating_sub(before.iterations);
            b.failures += after.failures.saturating_sub(before.failures);
            b.secs += dt;
        }
    }

    /// Call of loop program site `site` done.
    pub fn leave_call(&mut self, site: usize, t0: f64) {
        let dt = self.leave(t0);
        if let Some(b) = self.sites.get_mut(site) {
            b.calls += 1;
            b.secs += dt;
        }
    }

    #[inline]
    pub fn enter_rhs(&mut self) -> f64 {
        self.in_rhs += 1;
        now()
    }

    #[inline]
    pub fn leave_rhs(&mut self, t0: f64) {
        self.in_rhs -= 1;
        if self.in_rhs == 0 {
            self.rhs.calls += 1;
            self.rhs.secs += now() - t0;
        }
    }
}

// ----------------------------------------------------------------------------
// Collector
// ----------------------------------------------------------------------------

/// What started an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// A zero crossing found on the step (a state event, or a time event when only
    /// relations on `time` / `sample()` switched).
    Crossing,
    /// A pending time event (a delayed jump), or a time event due at the end time.
    Time,
}

/// State of the model just before an event, to see what the event changed.
pub struct EventSnap {
    t0: f64,
    rels: Vec<f64>,
    vals: Vec<f64>,
}

struct EventRec {
    t: f64,
    trigger: Trigger,
    rels: Vec<(u32, bool)>,
    whens: Vec<(u32, u32)>,
    changes: Vec<(u32, f64, f64)>,
    n_changes: usize,
}

#[derive(Clone, Default)]
struct RelAgg {
    count: u64,
    rising: u64,
    first: f64,
    last: f64,
    min_interval: f64,
    times: Vec<f64>,
}

/// Records a profiled run (owned by the simulation while it runs).
pub struct Profiler {
    n: usize,
    steps_t: Vec<f64>,
    steps_h: Vec<f64>,
    steps_order: Vec<u32>,
    steps_err: Vec<f64>,
    steps_dom: Vec<i32>,
    errs: Vec<f32>,
    errs_full: bool,
    n_steps: u64,
    n_err_steps: u64,
    rejected: Vec<(f64, f64, bool)>,
    n_rejected: u64,
    dom_count: Vec<u64>,
    err_sq: Vec<f64>,
    err_max: Vec<f64>,
    events: Vec<EventRec>,
    n_events: u64,
    rels: Vec<RelAgg>,
    pending_whens: Vec<(u32, u32)>,
    /// Slots of the values compared before and after events: states, then discretes.
    watched: Vec<u32>,
    /// Seconds spent in event handling and in storing output points.
    pub events_secs: f64,
    pub output_secs: f64,
}

impl Profiler {
    pub fn new(m: &Model) -> Self {
        let n = m.states.len();
        let mut watched: Vec<u32> = m.states.clone();
        watched.extend(m.discretes.iter().copied());
        Profiler {
            n,
            steps_t: Vec::new(),
            steps_h: Vec::new(),
            steps_order: Vec::new(),
            steps_err: Vec::new(),
            steps_dom: Vec::new(),
            errs: Vec::new(),
            errs_full: true,
            n_steps: 0,
            n_err_steps: 0,
            rejected: Vec::new(),
            n_rejected: 0,
            dom_count: vec![0; n],
            err_sq: vec![0.0; n],
            err_max: vec![0.0; n],
            events: Vec::new(),
            n_events: 0,
            rels: vec![
                RelAgg {
                    first: f64::NAN,
                    last: f64::NAN,
                    min_interval: f64::INFINITY,
                    ..Default::default()
                };
                m.layout.n_rel
            ],
            pending_whens: Vec::new(),
            watched,
            events_secs: 0.0,
            output_secs: 0.0,
        }
    }

    fn push_reject(&mut self, t: f64, h: f64, newton: bool) {
        self.n_rejected += 1;
        if self.rejected.len() < MAX_STEPS {
            self.rejected.push((t, h, newton));
        }
    }

    /// After `solver.step()`; `s0` are the solver's statistics before it.
    pub fn on_step(&mut self, solver: &mut dyn Integrator, s0: SolverStats) {
        let (tp, tn) = (solver.t_prev(), solver.t());
        let s1 = solver.stats();
        let accepted = s1.steps > s0.steps;
        let mut order = 0;
        let mut err: Option<(f64, i32)> = None;
        let probed = match solver.probe() {
            Some(p) if p.enabled => {
                for (t, h, newton) in std::mem::take(&mut p.rejected) {
                    self.push_reject(t, h, newton);
                }
                if accepted {
                    order = p.order;
                    if p.has_err && p.err.len() == self.n && self.n > 0 {
                        err = Some(self.analyze(&p.err));
                    }
                }
                p.has_err = false;
                true
            }
            _ => false,
        };
        if !probed {
            let rj = s1.rejected.saturating_sub(s0.rejected);
            let nf = s1.newton_failures.saturating_sub(s0.newton_failures);
            for k in 0..rj {
                self.push_reject(tp, f64::NAN, k < nf);
            }
        }
        if accepted {
            self.n_steps += 1;
            if self.steps_t.len() < MAX_STEPS {
                self.steps_t.push(tn);
                self.steps_h.push(tn - tp);
                self.steps_order.push(order);
                let (e, d) = err.unwrap_or((f64::NAN, -1));
                self.steps_err.push(e);
                self.steps_dom.push(d);
            }
        }
    }

    /// Error norm and largest contributor of one step's error terms.
    fn analyze(&mut self, e: &[f64]) -> (f64, i32) {
        self.n_err_steps += 1;
        let mut sum = 0.0;
        let mut dom = 0;
        let mut big = -1.0;
        for (i, &v) in e.iter().enumerate() {
            let a = v.abs();
            if a.is_finite() {
                sum += a * a;
                self.err_sq[i] += a * a;
                if a > self.err_max[i] {
                    self.err_max[i] = a;
                }
                if a > big {
                    big = a;
                    dom = i;
                }
            }
        }
        if big >= 0.0 {
            self.dom_count[dom] += 1;
        }
        if self.errs_full {
            if self.errs.len() + e.len() <= MAX_ERRS && self.steps_t.len() < MAX_STEPS {
                self.errs.extend(e.iter().map(|&v| v.abs() as f32));
            } else {
                self.errs_full = false;
            }
        }
        (
            (sum / e.len() as f64).sqrt(),
            if big >= 0.0 { dom as i32 } else { -1 },
        )
    }

    /// A when-clause branch fired (called from the event iteration).
    pub fn when_fired(&mut self, clause: usize, branch: usize) {
        if self.pending_whens.len() < 64 {
            self.pending_whens.push((clause as u32, branch as u32));
        }
    }

    /// Snapshot before an event is handled.
    pub fn event_begin(&mut self, rt: &Runtime) -> EventSnap {
        let m = rt.m;
        self.pending_whens.clear();
        EventSnap {
            t0: now(),
            rels: rt.r[m.layout.rel0..m.layout.rel0 + m.layout.n_rel].to_vec(),
            vals: self.watched.iter().map(|&s| rt.r[s as usize]).collect(),
        }
    }

    /// The event at `t` was handled: record what it changed.
    pub fn event_end(&mut self, snap: EventSnap, rt: &Runtime, t: f64, trigger: Trigger) {
        let m = rt.m;
        let mut rels = Vec::new();
        for (i, &before) in snap.rels.iter().enumerate() {
            let after = rt.r[m.layout.rel0 + i];
            if after != before {
                rels.push((i as u32, after != 0.0));
                let a = &mut self.rels[i];
                if a.count > 0 {
                    a.min_interval = a.min_interval.min(t - a.last);
                } else {
                    a.first = t;
                }
                a.count += 1;
                if after != 0.0 {
                    a.rising += 1;
                }
                a.last = t;
                if a.times.len() < MAX_REL_TIMES {
                    a.times.push(t);
                }
            }
        }
        let mut changes = Vec::new();
        let mut n_changes = 0;
        for (k, &s) in self.watched.iter().enumerate() {
            let (a, b) = (snap.vals[k], rt.r[s as usize]);
            if a != b && !(a.is_nan() && b.is_nan()) {
                n_changes += 1;
                if changes.len() < MAX_CHANGES {
                    changes.push((s, a, b));
                }
            }
        }
        self.n_events += 1;
        let whens = std::mem::take(&mut self.pending_whens);
        if self.events.len() < MAX_EVENTS {
            self.events.push(EventRec {
                t,
                trigger,
                rels,
                whens,
                changes,
                n_changes,
            });
        }
        self.events_secs += now() - snap.t0;
    }

    /// The when-clauses fired by the initialization (`initial()`, `sample()` at the
    /// start time) form the first entry of the log.
    pub fn initial_event(&mut self, t: f64) {
        let whens = std::mem::take(&mut self.pending_whens);
        if !whens.is_empty() {
            self.n_events += 1;
            self.events.push(EventRec {
                t,
                trigger: Trigger::Time,
                rels: Vec::new(),
                whens,
                changes: Vec::new(),
                n_changes: 0,
            });
        }
    }

    /// Build the report. `blocks` is the runtime's block timing, `span` the simulated
    /// interval.
    pub fn finish(
        self,
        m: &Model,
        blocks: Option<Box<BlockProf>>,
        stats: &SimStats,
        method: &str,
        span: f64,
    ) -> Profile {
        let state_names: Vec<String> = m
            .states
            .iter()
            .map(|&s| m.vars[s as usize].name.clone())
            .collect();
        let var_name = |s: u32| {
            m.vars
                .get(s as usize)
                .map_or_else(|| format!("${s}"), |v| v.name.clone())
        };
        // Relations.
        let rel_text: Vec<String> = (0..m.layout.n_rel)
            .map(|i| pretty_relation(m.rel_text.get(i).map_or("?", |s| s.as_str())))
            .collect();
        let gap = CHATTER_WINDOW * span.abs().max(1e-300) / CHATTER_COUNT as f64;
        let mut relations: Vec<RelationSummary> = self
            .rels
            .iter()
            .enumerate()
            .filter(|(_, a)| a.count > 0)
            .map(|(i, a)| {
                let timed = m.sample_rels.contains(&i)
                    || crate::sim::mentions_time(m.rel_text.get(i).map_or("", |s| s))
                    || rel_text[i].contains("$sample");
                let chattering = if timed {
                    None
                } else {
                    chatter_cluster(&a.times, gap)
                };
                RelationSummary {
                    index: i,
                    text: rel_text[i].clone(),
                    origin: origin_of(&rel_text[i]),
                    time_event: timed,
                    count: a.count,
                    rising: a.rising,
                    first: a.first,
                    last: a.last,
                    min_interval: if a.count > 1 {
                        a.min_interval
                    } else {
                        f64::NAN
                    },
                    chattering,
                }
            })
            .collect();
        relations.sort_by(|a, b| b.count.cmp(&a.count).then(a.index.cmp(&b.index)));
        // Events.
        let when_label = |wi: u32, bi: u32| -> (String, Vec<&'static str>) {
            let mut tags = Vec::new();
            let mut assigned = Vec::new();
            if let Some(acts) = m
                .whens
                .get(wi as usize)
                .and_then(|w| w.actions.get(bi as usize))
            {
                for a in acts {
                    match a {
                        Action::Assign(v, _) => assigned.push(var_name(*v)),
                        Action::Reinit(v, _) => {
                            tags.push("reinit");
                            assigned.push(format!("reinit({})", var_name(*v)));
                        }
                        Action::Terminate(_) => tags.push("terminate"),
                        Action::Assert(..) => {}
                    }
                }
            }
            let what = if assigned.is_empty() {
                format!("when #{wi}")
            } else {
                format!("when #{wi}: {}", assigned.join(", "))
            };
            (what, tags)
        };
        let events: Vec<EventEntry> = self
            .events
            .iter()
            .map(|e| {
                let mut tags: Vec<String> = Vec::new();
                let mut whens = Vec::new();
                for &(wi, bi) in &e.whens {
                    let (w, t) = when_label(wi, bi);
                    whens.push(w);
                    if !tags.iter().any(|x| x == "when") {
                        tags.push("when".into());
                    }
                    for x in t {
                        if !tags.iter().any(|y| y == x) {
                            tags.push(x.into());
                        }
                    }
                }
                let state_rel = e.rels.iter().any(|&(i, _)| {
                    relations
                        .iter()
                        .find(|r| r.index == i as usize)
                        .is_some_and(|r| !r.time_event)
                });
                let kind = if e.trigger == Trigger::Crossing && state_rel {
                    "state"
                } else {
                    "time"
                };
                let (text, origin) = match e.rels.first() {
                    Some(&(i, _)) => (
                        rel_text[i as usize].clone(),
                        origin_of(&rel_text[i as usize]),
                    ),
                    None => (String::new(), String::new()),
                };
                EventEntry {
                    t: e.t,
                    kind,
                    tags,
                    relations: e
                        .rels
                        .iter()
                        .map(|&(i, v)| RelSwitch {
                            index: i as usize,
                            value: v,
                        })
                        .collect(),
                    text,
                    origin,
                    whens,
                    changes: e
                        .changes
                        .iter()
                        .map(|&(s, a, b)| Change {
                            name: var_name(s),
                            before: a,
                            after: b,
                        })
                        .collect(),
                    n_changes: e.n_changes,
                }
            })
            .collect();
        // Limiting states.
        let mut limiting: Vec<StateLimit> = (0..self.n)
            .filter(|&i| self.dom_count[i] > 0)
            .map(|i| StateLimit {
                index: i,
                name: state_names[i].clone(),
                steps: self.dom_count[i],
                share: self.dom_count[i] as f64 / self.n_err_steps.max(1) as f64,
                rms_error: (self.err_sq[i] / self.n_err_steps.max(1) as f64).sqrt(),
                max_error: self.err_max[i],
            })
            .collect();
        limiting.sort_by(|a, b| b.steps.cmp(&a.steps).then(a.index.cmp(&b.index)));
        // Time split.
        let total_ms = stats.init_ms + stats.sim_ms;
        let events_ms = self.events_secs * 1e3;
        let output_ms = self.output_secs * 1e3;
        let integration_ms = (stats.sim_ms - events_ms - output_ms).max(0.0);
        // Blocks.
        let mut block_costs = Vec::new();
        let mut newton_stats = Vec::new();
        if let Some(bp) = &blocks {
            for (i, b) in bp.newton.iter().enumerate() {
                if b.calls == 0 {
                    continue;
                }
                let nb = &m.newton[i];
                let names: Vec<String> = nb.names.iter().take(40).cloned().collect();
                let kind = if nb.linear { "linear" } else { "nonlinear" };
                let label = format!(
                    "{kind} block {i} ({} tear{}, {} unknown{})",
                    nb.tears.len(),
                    if nb.tears.len() == 1 { "" } else { "s" },
                    nb.names.len(),
                    if nb.names.len() == 1 { "" } else { "s" }
                );
                newton_stats.push(NewtonBlockStats {
                    index: i,
                    label: label.clone(),
                    calls: b.calls,
                    iterations: b.iterations,
                    failures: b.failures,
                });
                block_costs.push(BlockCost {
                    kind: kind.into(),
                    index: i as i64,
                    label,
                    origin: common_prefix(&nb.names),
                    vars: names,
                    eqs: nb.eqs.iter().take(40).cloned().collect(),
                    calls: b.calls,
                    iterations: b.iterations,
                    failures: b.failures,
                    time_ms: b.secs * 1e3,
                    share: 0.0,
                });
            }
            let mut per_prog: Vec<BlockAgg> = vec![BlockAgg::default(); m.progs.len()];
            for (s, b) in bp.sites.iter().enumerate() {
                if let Some(p) = m
                    .sites
                    .get(s)
                    .and_then(|x| per_prog.get_mut(x.prog as usize))
                {
                    p.calls += b.calls;
                    p.secs += b.secs;
                }
            }
            for (p, b) in per_prog.iter().enumerate() {
                if b.calls == 0 {
                    continue;
                }
                block_costs.push(BlockCost {
                    kind: "function".into(),
                    index: p as i64,
                    label: format!(
                        "function loop {p} (while loop, {} state values)",
                        m.progs[p].n
                    ),
                    origin: String::new(),
                    vars: Vec::new(),
                    eqs: Vec::new(),
                    calls: b.calls,
                    iterations: 0,
                    failures: 0,
                    time_ms: b.secs * 1e3,
                    share: 0.0,
                });
            }
            let explicit_ms = ((bp.rhs.secs - bp.nested_in_rhs) * 1e3).max(0.0);
            block_costs.push(BlockCost {
                kind: "explicit".into(),
                index: -1,
                label: "explicit equations (derivative evaluations outside the blocks above)"
                    .into(),
                origin: String::new(),
                vars: Vec::new(),
                eqs: Vec::new(),
                calls: bp.rhs.calls,
                iterations: 0,
                failures: 0,
                time_ms: explicit_ms,
                share: 0.0,
            });
            let solver_ms = (integration_ms - bp.rhs.secs * 1e3).max(0.0);
            block_costs.push(BlockCost {
                kind: "solver".into(),
                index: -1,
                label: "integrator (linear algebra, step and error control, event search)".into(),
                origin: String::new(),
                vars: Vec::new(),
                eqs: Vec::new(),
                calls: stats.solver.steps,
                iterations: 0,
                failures: 0,
                time_ms: solver_ms,
                share: 0.0,
            });
        }
        let denom = total_ms.max(1e-12);
        for b in &mut block_costs {
            b.share = b.time_ms / denom;
        }
        block_costs.sort_by(|a, b| b.time_ms.total_cmp(&a.time_ms));
        let mut p = Profile {
            model: m.name.clone(),
            method: method.to_string(),
            states: state_names,
            timing: Timing {
                total_ms,
                init_ms: stats.init_ms,
                integration_ms,
                events_ms,
                output_ms,
            },
            solver: stats.solver,
            events_total: self.n_events,
            state_events: stats.state_events,
            newton_solves: stats.newton_solves,
            newton_iterations: stats.newton_iterations,
            steps: StepSeries {
                t: self.steps_t,
                h: self.steps_h,
                order: self.steps_order,
                err: self.steps_err,
                dominant: self.steps_dom,
            },
            steps_total: self.n_steps,
            step_errors: self.errs,
            step_errors_complete: self.errs_full,
            rejected: self
                .rejected
                .iter()
                .map(|&(t, h, newton)| Rejected {
                    t,
                    h,
                    reason: if newton { "newton" } else { "error" },
                })
                .collect(),
            rejected_total: self.n_rejected,
            limiting,
            blocks: block_costs,
            newton_blocks: newton_stats,
            events,
            relations,
            chatter_rule: ChatterRule {
                count: CHATTER_COUNT,
                max_gap: gap,
            },
            hints: Vec::new(),
        };
        p.hints = hints(&p);
        p
    }
}

/// The densest run of switching times with consecutive gaps at most `gap`, if it holds
/// at least [`CHATTER_COUNT`] switches.
fn chatter_cluster(times: &[f64], gap: f64) -> Option<Chatter> {
    let mut best: Option<(usize, usize)> = None;
    let mut start = 0;
    for k in 1..=times.len() {
        if k == times.len() || times[k] - times[k - 1] > gap {
            if k - start >= CHATTER_COUNT && best.is_none_or(|(a, b)| k - start > b - a) {
                best = Some((start, k));
            }
            start = k;
        }
    }
    best.map(|(a, b)| Chatter {
        count: (b - a) as u64,
        from: times[a],
        to: times[b - 1],
    })
}

/// Relation text with readable operators (`x Gt 0` -> `x > 0`).
pub fn pretty_relation(s: &str) -> String {
    let mut out = s.to_string();
    for op in [Cmp::Lt, Cmp::Le, Cmp::Gt, Cmp::Ge, Cmp::Eq, Cmp::Ne] {
        out = out.replace(&format!(" {op:?} "), &format!(" {} ", op.symbol()));
    }
    out
}

/// The component a relation belongs to: the enclosing instance of the first dotted name
/// it reads (`clutch.v > 0` -> `clutch`); empty at the top level.
pub fn origin_of(text: &str) -> String {
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '[' || c == ']';
    let mut best = String::new();
    for tok in text.split(|c: char| !ident(c)) {
        if let Some(k) = tok.rfind('.')
            && tok
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            best = tok[..k].to_string();
            break;
        }
    }
    best
}

/// Longest common instance prefix of dotted names (`a.b.x`, `a.b.y` -> `a.b`).
fn common_prefix(names: &[String]) -> String {
    let mut it = names.iter();
    let Some(first) = it.next() else {
        return String::new();
    };
    let mut parts: Vec<&str> = first.split('.').collect();
    parts.pop();
    for n in it {
        let q: Vec<&str> = n.split('.').collect();
        let k = parts
            .iter()
            .zip(&q[..q.len().saturating_sub(1)])
            .take_while(|(a, b)| a == b)
            .count();
        parts.truncate(k);
    }
    parts.join(".")
}

fn pct(x: f64) -> String {
    format!("{:.0}%", 100.0 * x)
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Two times with as many digits as it takes to tell them apart.
fn fmt_range(a: f64, b: f64) -> (String, String) {
    let scale = a.abs().max(b.abs()).max(1e-300);
    let rel = ((b - a).abs() / scale).max(1e-15);
    let digits = ((-rel.log10()).ceil() as usize + 2).clamp(6, 15);
    let f = |x: f64| {
        format!(
            "{}",
            format!("{x:.*e}", digits - 1).parse::<f64>().unwrap_or(x)
        )
    };
    (f(a), f(b))
}

/// Plain-language hints from a profile.
fn hints(p: &Profile) -> Vec<String> {
    let mut h = Vec::new();
    let explicit = matches!(p.method.as_str(), "Dopri5" | "Rk4" | "Euler");
    for s in p.limiting.iter().take(3) {
        // With one state the answer is trivially that state.
        if s.share < 0.25 || p.states.len() < 2 {
            break;
        }
        let mut line = format!(
            "state `{}` limited the step in {} of steps (rms scaled error {:.2})",
            s.name,
            pct(s.share),
            s.rms_error
        );
        if s.share >= 0.5 && explicit && p.solver.steps > 1000 {
            line.push_str(
                "; if it is a fast, well-damped mode (a stiff state), an implicit method \
                 (BDF) takes much longer steps",
            );
        } else if s.share >= 0.5 {
            line.push_str(
                "; check its nominal scale and the tolerance, or whether a fast mode it \
                 belongs to matters for the result",
            );
        }
        h.push(line);
    }
    let attempts = p.solver.steps + p.rejected_total;
    if attempts > 20 && p.rejected_total as f64 > 0.2 * attempts as f64 {
        let newton = p.rejected.iter().filter(|r| r.reason == "newton").count();
        h.push(format!(
            "{} of {} step attempts were rejected ({} by the error test, {} by Newton \
             failures): the step size keeps overshooting; look for discontinuities \
             without events (noEvent, abs, min/max in derivatives) or a too loose \
             max step",
            group(p.rejected_total),
            group(attempts),
            group(p.rejected_total - newton as u64),
            group(newton as u64)
        ));
    }
    for r in &p.relations {
        if let Some(c) = &r.chattering {
            let place = if r.origin.is_empty() {
                String::new()
            } else {
                format!(" in `{}`", r.origin)
            };
            let (a, b) = fmt_range(c.from, c.to);
            h.push(format!(
                "relation `{}`{place} fired {} times between t={a} and {b}: likely \
                 chattering; consider hysteresis, a stick/slip (mode) formulation that \
                 holds the switched state, or noEvent() if the expression is continuous",
                r.text,
                group(c.count),
            ));
        }
    }
    for b in p.blocks.iter().take(5) {
        if (b.kind == "nonlinear" || b.kind == "linear") && b.share >= 0.2 {
            let ipc = b.iterations as f64 / b.calls.max(1) as f64;
            let vars = b
                .vars
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            h.push(format!(
                "{} over [{}{}] takes {} of the run time ({} calls, {:.1} iterations per \
                 call){}",
                b.label,
                vars,
                if b.vars.len() > 4 { ", …" } else { "" },
                pct(b.share),
                group(b.calls),
                ipc,
                if b.kind == "nonlinear" && ipc > 3.0 {
                    "; better start values or a different tearing (choice of iteration \
                     variables) may cut the iterations"
                } else {
                    ""
                }
            ));
        }
        if b.failures > 0 {
            h.push(format!(
                "{} failed to converge {} times (the integrator then retries with a smaller step)",
                b.label,
                group(b.failures)
            ));
        }
    }
    if p.timing.total_ms > 20.0 && p.timing.events_ms > 0.3 * p.timing.total_ms {
        h.push(format!(
            "event handling takes {} of the run time ({} events)",
            pct(p.timing.events_ms / p.timing.total_ms),
            group(p.events_total)
        ));
    }
    if p.timing.total_ms > 20.0 && p.timing.output_ms > 0.3 * p.timing.total_ms {
        h.push(format!(
            "storing output points takes {} of the run time: fewer output intervals \
             would speed it up",
            pct(p.timing.output_ms / p.timing.total_ms)
        ));
    }
    h
}

// ----------------------------------------------------------------------------
// Report
// ----------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct Timing {
    pub total_ms: f64,
    pub init_ms: f64,
    /// Integration without event handling and output.
    pub integration_ms: f64,
    pub events_ms: f64,
    pub output_ms: f64,
}

/// Accepted steps (parallel arrays).
#[derive(Clone, Debug, Default, Serialize)]
pub struct StepSeries {
    /// End time of the step.
    pub t: Vec<f64>,
    pub h: Vec<f64>,
    /// Order of the method (0: not reported by the integrator).
    pub order: Vec<u32>,
    /// Error norm (RMS of the scaled error terms; null without an estimate).
    pub err: Vec<f64>,
    /// State with the largest error term (-1: none).
    pub dominant: Vec<i32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Rejected {
    pub t: f64,
    /// Size of the rejected attempt (null when the integrator does not report it).
    pub h: f64,
    /// "error" (error test) | "newton" (Newton failure).
    pub reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateLimit {
    pub index: usize,
    pub name: String,
    /// Steps in which this state had the largest error term.
    pub steps: u64,
    pub share: f64,
    pub rms_error: f64,
    pub max_error: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockCost {
    /// "nonlinear" | "linear" | "function" | "explicit" | "solver"
    pub kind: String,
    /// Newton block or loop program index (-1 for the aggregate rows).
    pub index: i64,
    pub label: String,
    /// Component the block's unknowns belong to.
    pub origin: String,
    pub vars: Vec<String>,
    /// Residual equations `0 = ...`.
    pub eqs: Vec<String>,
    pub calls: u64,
    pub iterations: u64,
    pub failures: u64,
    /// Time over the whole run (integration, events and output points), including nested
    /// blocks (calls inside a Newton block's equations).
    pub time_ms: f64,
    /// Fraction of the total run time.
    pub share: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct NewtonBlockStats {
    pub index: usize,
    pub label: String,
    pub calls: u64,
    pub iterations: u64,
    pub failures: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelSwitch {
    pub index: usize,
    /// Value after the event.
    pub value: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    pub name: String,
    pub before: f64,
    pub after: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct EventEntry {
    pub t: f64,
    /// "state" | "time"
    pub kind: &'static str,
    /// What the event did: "when", "reinit", "terminate".
    pub tags: Vec<String>,
    /// Relations that switched.
    pub relations: Vec<RelSwitch>,
    /// Text and component of the first relation that switched.
    pub text: String,
    pub origin: String,
    /// When-clauses that fired.
    pub whens: Vec<String>,
    /// States and discrete variables changed by the event (the first few).
    pub changes: Vec<Change>,
    pub n_changes: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Chatter {
    /// Switches in the densest run.
    pub count: u64,
    pub from: f64,
    pub to: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct RelationSummary {
    pub index: usize,
    pub text: String,
    pub origin: String,
    /// A relation on `time` or `sample()` (never reported as chattering).
    pub time_event: bool,
    /// Switches (both directions).
    pub count: u64,
    /// Switches to true.
    pub rising: u64,
    pub first: f64,
    pub last: f64,
    pub min_interval: f64,
    pub chattering: Option<Chatter>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatterRule {
    pub count: usize,
    pub max_gap: f64,
}

/// The performance report of one simulation.
#[derive(Clone, Debug, Serialize)]
pub struct Profile {
    pub model: String,
    pub method: String,
    pub states: Vec<String>,
    pub timing: Timing,
    pub solver: SolverStats,
    pub events_total: u64,
    pub state_events: u64,
    pub newton_solves: u64,
    pub newton_iterations: u64,
    pub steps: StepSeries,
    pub steps_total: u64,
    /// Scaled error term per state and recorded step (row-major, `states.len()` per
    /// step, for the first steps with an estimate).
    pub step_errors: Vec<f32>,
    /// `step_errors` covers every step with an estimate.
    pub step_errors_complete: bool,
    pub rejected: Vec<Rejected>,
    pub rejected_total: u64,
    /// States by the number of steps they limited.
    pub limiting: Vec<StateLimit>,
    /// Most expensive first.
    pub blocks: Vec<BlockCost>,
    pub newton_blocks: Vec<NewtonBlockStats>,
    pub events: Vec<EventEntry>,
    /// Relations that switched, most often first.
    pub relations: Vec<RelationSummary>,
    pub chatter_rule: ChatterRule,
    pub hints: Vec<String>,
}

impl Profile {
    /// The summary report as text (what `daedalus simulate --profile` prints).
    pub fn summary_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let t = &self.timing;
        let share = |x: f64| {
            if t.total_ms > 0.0 {
                pct(x / t.total_ms)
            } else {
                "-".into()
            }
        };
        let _ = writeln!(s, "Performance profile of {} ({})", self.model, self.method);
        let _ = writeln!(
            s,
            "  time: {:.2} ms total = initialization {:.2} ms ({}), integration {:.2} ms ({}), \
             events {:.2} ms ({}), output {:.2} ms ({})",
            t.total_ms,
            t.init_ms,
            share(t.init_ms),
            t.integration_ms,
            share(t.integration_ms),
            t.events_ms,
            share(t.events_ms),
            t.output_ms,
            share(t.output_ms)
        );
        let st = &self.solver;
        let _ = writeln!(
            s,
            "  steps: {} accepted, {} rejected ({} Newton); {} f-evals, {} Jacobians, {} LU; \
             {} events ({} state events); {} nonlinear solves, {} iterations",
            group(st.steps),
            group(self.rejected_total),
            group(
                self.rejected
                    .iter()
                    .filter(|r| r.reason == "newton")
                    .count() as u64
            ),
            group(st.rhs_evals),
            group(st.jac_evals),
            group(st.lu_factorizations),
            group(self.events_total),
            group(self.state_events),
            group(self.newton_solves),
            group(self.newton_iterations)
        );
        if !self.steps.h.is_empty() {
            let (mut lo, mut hi) = (f64::INFINITY, 0.0f64);
            for &h in &self.steps.h {
                if h > 0.0 {
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
            if lo <= hi {
                let _ = writeln!(s, "  step size: {lo:.3e} .. {hi:.3e}");
            }
        }
        if !self.limiting.is_empty() {
            let _ = writeln!(s, "  states limiting the step (largest error term):");
            for l in self.limiting.iter().take(5) {
                let _ = writeln!(
                    s,
                    "    {:>5}  {:<40} {} steps, rms scaled error {:.3}",
                    pct(l.share),
                    l.name,
                    group(l.steps),
                    l.rms_error
                );
            }
        }
        if !self.blocks.is_empty() {
            let _ = writeln!(s, "  costliest blocks:");
            for b in self.blocks.iter().take(5) {
                let iters = if b.kind == "linear" || b.kind == "nonlinear" {
                    format!(
                        ", {:.1} it/call{}",
                        b.iterations as f64 / b.calls.max(1) as f64,
                        if b.failures > 0 {
                            format!(", {} failures", group(b.failures))
                        } else {
                            String::new()
                        }
                    )
                } else {
                    String::new()
                };
                let vars = if b.vars.is_empty() {
                    String::new()
                } else {
                    format!(
                        " [{}{}]",
                        b.vars
                            .iter()
                            .take(3)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                        if b.vars.len() > 3 { ", …" } else { "" }
                    )
                };
                let _ = writeln!(
                    s,
                    "    {:>5}  {:>9.2} ms  {}{} ({} calls{})",
                    pct(b.share),
                    b.time_ms,
                    b.label,
                    vars,
                    group(b.calls),
                    iters
                );
            }
        }
        let chat: Vec<&RelationSummary> = self
            .relations
            .iter()
            .filter(|r| r.chattering.is_some())
            .collect();
        if !self.relations.is_empty() {
            let _ = writeln!(
                s,
                "  relations: {} switched, {} chattering",
                self.relations.len(),
                chat.len()
            );
            for r in self.relations.iter().take(5) {
                let (a, b) = fmt_range(r.first, r.last);
                let _ = writeln!(
                    s,
                    "    {:>7}x  {}{}  t = {a} .. {b}{}",
                    group(r.count),
                    r.text,
                    if r.origin.is_empty() {
                        String::new()
                    } else {
                        format!("  ({})", r.origin)
                    },
                    if r.chattering.is_some() {
                        "  CHATTERING"
                    } else {
                        ""
                    }
                );
            }
        }
        if !self.hints.is_empty() {
            let _ = writeln!(s, "  hints:");
            for h in &self.hints {
                let _ = writeln!(s, "    - {h}");
            }
        }
        s
    }

    /// The event log as text, one event per line.
    pub fn event_log_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "{:>14}  {:<6} {:<18} what", "time", "kind", "action");
        for e in &self.events {
            let mut what = Vec::new();
            if !e.text.is_empty() {
                let more = e.relations.len().saturating_sub(1);
                what.push(format!(
                    "{} -> {}{}{}",
                    e.text,
                    e.relations[0].value,
                    if e.origin.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", e.origin)
                    },
                    if more > 0 {
                        format!(" +{more} relations")
                    } else {
                        String::new()
                    }
                ));
            }
            what.extend(e.whens.iter().cloned());
            let ch: Vec<String> = e
                .changes
                .iter()
                .map(|c| format!("{}: {} -> {}", c.name, c.before, c.after))
                .collect();
            if !ch.is_empty() {
                what.push(format!(
                    "{}{}",
                    ch.join(", "),
                    if e.n_changes > e.changes.len() {
                        format!(" (+{})", e.n_changes - e.changes.len())
                    } else {
                        String::new()
                    }
                ));
            }
            let _ = writeln!(
                s,
                "{:>14.9}  {:<6} {:<18} {}",
                e.t,
                e.kind,
                e.tags.join(","),
                what.join("; ")
            );
        }
        if self.events_total as usize > self.events.len() {
            let _ = writeln!(
                s,
                "... {} more events not listed",
                self.events_total as usize - self.events.len()
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chatter_cluster_finds_dense_run() {
        let mut t: Vec<f64> = vec![0.1, 0.5];
        t.extend((0..30).map(|k| 1.0 + k as f64 * 1e-5));
        t.push(2.0);
        let c = chatter_cluster(&t, 1e-4).unwrap();
        assert_eq!(c.count, 30);
        assert_eq!(c.from, 1.0);
        assert!(chatter_cluster(&t[..10], 1e-4).is_none());
    }

    #[test]
    fn names() {
        assert_eq!(pretty_relation("clutch.v Gt 0"), "clutch.v > 0");
        assert_eq!(origin_of("clutch.v > 0"), "clutch");
        assert_eq!(origin_of("a.b[2].x <= time"), "a.b[2]");
        assert_eq!(origin_of("x > 0"), "");
        assert_eq!(
            common_prefix(&["a.b.x".into(), "a.b.y".into(), "a.c".into()]),
            "a"
        );
    }
}
