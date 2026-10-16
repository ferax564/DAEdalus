//! A record of the backend's structural decisions, kept only when
//! [`super::Options::trace`] is set: the equations after index reduction, which
//! equation computes which unknown in each BLT block, and the candidate sets of the
//! dummy-derivative state selection. [`crate::structure`] turns it into the report of
//! the Structure view and [`crate::debug`] into transformation chains and structural
//! diagnostics; nothing in the simulation path reads it.

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
    /// The flat (source) equation it descends from: index into [`Trace::sources`], or
    /// [`super::NO_SRC`] for a generated one (start value of initialization).
    pub src: u32,
}

/// An equation as the frontend produced it, before any symbolic transformation.
#[derive(Clone, Copy, Debug)]
pub struct SourceEq {
    pub lhs: ExprId,
    pub rhs: ExprId,
    pub origin: u32,
    /// An initial equation.
    pub initial: bool,
}

/// An equation alias elimination consumed: `a = b`, `a = -b` or `a = f(parameters)`.
#[derive(Clone, Copy, Debug)]
pub struct AliasEq {
    pub src: u32,
    /// The equation as alias elimination saw it (after earlier substitutions).
    pub lhs: ExprId,
    pub rhs: ExprId,
    /// The variables it relates (the second is `None` for `a = f(parameters)`).
    pub a: VarId,
    pub b: Option<VarId>,
}

/// One equation of a structurally singular system, rendered when the error occurred.
#[derive(Clone, Debug)]
pub struct SingularEq {
    pub text: String,
    pub origin: String,
    pub origin_id: u32,
    pub src: u32,
    /// Indices into [`Singular::vars`].
    pub unknowns: Vec<u32>,
    /// The equation as written, when alias elimination or index reduction changed it.
    pub source_text: Option<String>,
    /// Equations alias elimination consumed for variables of this one, `(text, origin)`:
    /// they take part in the conflict although they are no longer in the system.
    pub related: Vec<(String, u32)>,
}

/// A system whose matching failed: kept so that [`crate::debug`] can locate the over-
/// and underdetermined parts. The compile error itself names a few equations; this
/// holds the whole system.
#[derive(Clone, Debug, Default)]
pub struct Singular {
    /// `index reduction`, `simulation` or `initialization`.
    pub problem: String,
    pub eqs: Vec<SingularEq>,
    /// Names of the unknowns.
    pub vars: Vec<String>,
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
    /// The flat equations (`initial` ones after the others), before alias elimination.
    pub sources: Vec<SourceEq>,
    /// Equations removed by alias elimination, in the order they were found.
    pub alias_eqs: Vec<AliasEq>,
    /// `(representative, member)`: the representative of an alias class took its
    /// `start` (and `fixed`) from `member`.
    pub start_from: Vec<(VarId, VarId)>,
    /// Source equations dropped as trivially satisfied after substitution.
    pub redundant: Vec<u32>,
    /// The initialization system and its blocks (as `eqs` / `blocks` for simulation).
    pub init_eqs: Vec<TraceEq>,
    pub init_incidence: Vec<Vec<VarId>>,
    pub init_blocks: Vec<TraceBlock>,
    /// Unknowns whose `fixed = true` start value is an initial equation.
    pub init_fixed: Vec<VarId>,
    /// Unknowns whose start value initialization added to make the system square
    /// (states without `fixed`, then other variables).
    pub init_defaulted: Vec<VarId>,
    /// Initial / `fixed` equations dropped as redundant.
    pub init_redundant: Vec<String>,
    /// Set when a matching failed (the compile returns an error then).
    pub singular: Option<Singular>,
}
