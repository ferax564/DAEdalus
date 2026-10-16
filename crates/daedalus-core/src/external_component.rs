//! Extension point for model components whose equations are evaluated outside the
//! compiled tape: imported FMUs (`daedalus import-fmu`).
//!
//! A class carrying the annotation `__Daedalus_FMU(fileName = "X.fmu", fmiType = "...")`
//! is an *external component*. The frontend records each instance of it in
//! [`crate::flat::FlatModel::externals`] and turns the values the component supplies (its
//! outputs, and for Model Exchange the derivatives `fmu_dx` of its continuous states
//! `fmu_x`) into externally set variables ([`crate::flat::VarKind::Input`]), so the
//! rest of the model compiles as usual.
//!
//! At simulation time a [`Hook`] owns the external components: the runtime calls
//! [`Hook::ode`] wherever it would evaluate the model equations, and the simulator
//! asks the hook for event indicators, time events and event handling. The hook is
//! created by the native FMU importer (`crate::fmu_import`, feature `fmu`); where that
//! is not available (WebAssembly, the FMU runtime) simulating such a model fails with
//! a clear error.

use crate::codegen::Runtime;
use serde::{Deserialize, Serialize};

/// Name of the class annotation that marks an FMU wrapper block.
pub const FMU_ANNOTATION: &str = "__Daedalus_FMU";
/// Continuous states of a Model Exchange FMU in its wrapper block.
pub const STATES: &str = "fmu_x";
/// Their derivatives (supplied by the FMU).
pub const DERIVATIVES: &str = "fmu_dx";
/// Communication step size parameter of a Co-Simulation wrapper.
pub const STEP: &str = "communicationStepSize";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FmiKind {
    ModelExchange,
    CoSimulation,
}

impl FmiKind {
    pub fn parse(s: &str) -> Option<FmiKind> {
        match s {
            "ModelExchange" | "ME" | "me" => Some(FmiKind::ModelExchange),
            "CoSimulation" | "CS" | "cs" => Some(FmiKind::CoSimulation),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FmiKind::ModelExchange => "ModelExchange",
            FmiKind::CoSimulation => "CoSimulation",
        }
    }
}

/// One instance of an FMU wrapper block in a flattened model.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExternalComponent {
    /// Instance name (`plant`, `a.b[2]`; empty for the top-level model).
    pub prefix: String,
    /// The FMU file, resolved against the directory of the wrapper's source file.
    pub file: String,
    pub kind: FmiKind,
    /// Wrapper class name (for messages).
    pub class: String,
}

impl ExternalComponent {
    /// Full name of a wrapper member.
    pub fn member(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}.{name}", self.prefix)
        }
    }
}

/// Evaluation of external components inside a simulation (see the module docs).
///
/// Event handling follows the FMI pattern: [`Hook::begin_event`], then model event
/// iterations alternating with [`Hook::update_event`] until it reports no change, then
/// [`Hook::end_event`].
pub trait Hook: Send {
    /// Evaluate the model equations (`m.ode`) at the time and states in `rt`,
    /// supplying the external values (outputs, state derivatives).
    fn ode(&mut self, rt: &mut Runtime);
    /// Before the model's initialization: parameters are evaluated and start values
    /// set. Put the components into initialization mode with those inputs and write
    /// their outputs into `rt`.
    fn init(&mut self, rt: &mut Runtime, start: f64, stop: f64, tol: f64) -> Result<(), String>;
    /// After the model's initial equations were solved, components still in
    /// initialization mode: pass the solved inputs and write the outputs into `rt`;
    /// returns whether any output changed (the model's initialization must be solved
    /// again).
    fn init_update(&mut self, rt: &mut Runtime) -> Result<bool, String>;
    /// End the components' initialization with the solved inputs; write their outputs
    /// and initial states into `rt`. Returns whether any of them changed.
    fn exit_init(&mut self, rt: &mut Runtime) -> Result<bool, String>;
    /// Number of event indicators.
    fn n_indicators(&self) -> usize;
    /// Event indicators at the point of the last [`Hook::ode`].
    fn indicators(&mut self, rt: &mut Runtime, z: &mut [f64]);
    /// Earliest time event after the current time, if any (Model Exchange time events,
    /// Co-Simulation communication points).
    fn next_time_event(&self) -> Option<f64>;
    /// An event at the current time of `rt` begins (any event of the model).
    fn begin_event(&mut self, rt: &mut Runtime) -> Result<(), String>;
    /// After a model event iteration: pass inputs that changed and update the
    /// components' discrete states; returns whether any external value changed (the
    /// model must iterate again).
    fn update_event(&mut self, rt: &mut Runtime) -> Result<bool, String>;
    /// The event ends: back to continuous time, new states written into `rt`.
    fn end_event(&mut self, rt: &mut Runtime) -> Result<(), String>;
    /// After an accepted integrator step (no event): whether an event must be
    /// handled at this point (FMI step events).
    fn completed_step(&mut self, rt: &mut Runtime) -> Result<bool, String>;
    /// A component asked to stop the simulation.
    fn terminated(&self) -> Option<String> {
        None
    }
}

/// Creates the hook for a model with external components; installed by the native
/// FMU importer.
pub type HookFactory = fn(&crate::codegen::Model) -> Result<Box<dyn Hook>, String>;

/// The hook factory available in this build, if any.
pub fn factory() -> Option<HookFactory> {
    #[cfg(all(feature = "fmu", not(target_arch = "wasm32")))]
    {
        Some(crate::fmu_import::hook::create)
    }
    #[cfg(not(all(feature = "fmu", not(target_arch = "wasm32"))))]
    {
        None
    }
}

/// Error for a model with FMU components where they cannot be simulated.
pub fn unsupported(m: &crate::codegen::Model) -> String {
    let list: Vec<String> = m
        .externals
        .iter()
        .map(|e| {
            format!(
                "'{}' ({})",
                if e.prefix.is_empty() {
                    &m.name
                } else {
                    &e.prefix
                },
                e.file
            )
        })
        .collect();
    format!(
        "the model contains FMU components ({}): imported FMUs can only be simulated \
         with the native `daedalus` command line tool or the Python package, not in \
         the browser",
        list.join(", ")
    )
}
