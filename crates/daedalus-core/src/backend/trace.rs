//! A record of the backend's structural decisions, kept only when
//! [`super::Options::trace`] is set: the equations after index reduction, which
//! equation computes which unknown in each BLT block, and the candidate sets of the
//! dummy-derivative state selection. [`crate::structure`] turns it into the report of
//! the Structure view; nothing in the simulation path reads it.

use super::state_select::AppliedOverride;
use crate::expr::{ExprId, VarId};

/// One equation of the simulation system (index into [`Trace::eqs`] = equation id).
#[derive(Clone, Copy, Debug)]
pub struct TraceEq {
    pub lhs: ExprId,
    pub rhs: ExprId,
    pub origin: u32,
    /// The equation this one was differentiated from (index reduction).
    pub parent: Option<usize>,
}

/// One runtime block, in the order of [`super::System::blocks`].
#[derive(Clone, Debug, Default)]
pub struct TraceBlock {
    /// `(equation, unknown)` in evaluation order: an assignment, or the inner
    /// (explicitly solved) equations of a torn loop.
    pub solved: Vec<(usize, VarId)>,
    /// Residual equations of a loop, solved by Newton together for the tear variables.
    pub residual_eqs: Vec<usize>,
    /// The tear (iteration) variables.
    pub tears: Vec<VarId>,
}

/// One block of the dummy-derivative selection: `eqs` (differentiated equations) must be
/// solved for `dummies.len()` of the `candidates`; the others stay derivatives of states.
#[derive(Clone, Debug)]
pub struct StateSet {
    /// 1 for the highest derivatives, 2 for the next order down, ...
    pub level: usize,
    pub eqs: Vec<usize>,
    /// Derivative variables, most willing to become a dummy first.
    pub candidates: Vec<VarId>,
    /// Coarse tier of each candidate (lower is more wanted as a state's derivative).
    pub tiers: Vec<i64>,
    pub dummies: Vec<VarId>,
}

#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub eqs: Vec<TraceEq>,
    /// Unknowns (states excluded) of each equation.
    pub incidence: Vec<Vec<VarId>>,
    pub blocks: Vec<TraceBlock>,
    pub state_sets: Vec<StateSet>,
    /// Equations differentiated so that `stateSelect = prefer/always` variables can
    /// become states (Dymola's state change).
    pub state_change_eqs: usize,
    pub overrides: Vec<AppliedOverride>,
}
