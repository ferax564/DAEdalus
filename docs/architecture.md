# Architecture

DAEdalus goes from Modelica source text to results in five stages:

```
.mo sources ──► frontend ──► flat model ──► backend ──► causal system ──► codegen ──► tape ──► solvers ──► results
               (Rumoca     (scalar DAE)    (symbolic)                    (register             (BDF, Dopri5,   (.mat / CSV /
                parser +                                                  bytecode)             RK4, events)    in memory)
                own inst.)
```

All of it lives in `daedalus-core`. The CLI (`daedalus-cli`) and the browser
workbench (`daedalus-wasm` plus `web/`) are thin layers on top. The design goal
is that compile time and run time both stay close to linear in model size.

## What we took from whom

| Idea | Seen in | Here |
| --- | --- | --- |
| Aggressive symbolic preprocessing before any numerics (alias elimination, index reduction, BLT, tearing) | Dymola | `backend/` |
| Dummy-derivative state selection honouring `stateSelect` | Dymola, OpenModelica | `backend/index.rs` |
| A separate structurally analysed initialization problem | Dymola, OpenModelica | `backend/sort.rs` |
| `dsres.mat` result format, so existing scripts keep working | Dymola | `results.rs` |
| Package browser, icon, diagram and text views, variable browser, plot window | Dymola, OMEdit | `web/` |
| Browser-first workbench and server-side compile | Modelon Impact | `daedalus-wasm`, Web Worker |
| Display units in plots, parametric plots, crosshair readout | Wolfram SystemModeler, Dymola | `web/src/plot.ts` |
| Library test sweep with one process per model | OpenModelica library testing | `daedalus test-library` |

## Frontend (`src/frontend`)

**Parsing.** Rumoca's Modelica parser (Apache-2.0, pinned commit) produces the
AST. Everything after parsing is DAEdalus code.

**Class tree (`tree.rs`).** This holds all loaded files and implements Modelica
name lookup: enclosing scopes, imports, inherited members and redeclared
classes.

**Instantiation (`inst.rs`)** runs in two passes.
1. The first pass builds the instance tree: components, array dimensions,
   modifiers, `each` and `final`, conditional components, and replaceable
   redeclarations.
2. The second pass translates the bindings and equations of every instance into
   the expression arena. Along the way it:
   - unrolls `for` equations and array equations to scalars;
   - inlines functions by symbolic execution;
   - resolves `connect` sets (potential equality, flow sum to zero), including
     loops over component arrays;
   - handles expandable connectors (MLS 9.1.3): bus members that no equation
     uses become hidden constants instead of unknowns;
   - maps `ExternalObject` classes to integer handles and replaces the
     `ModelicaStandardTables` C functions with native tables. A lookup becomes
     one interpolation node in the arena, with linear, constant, Akima and
     Fritsch–Butland cubic smoothness, hold, linear and periodic
     extrapolation, and time events at breakpoints.

**Synchronous models and state machines.** After flattening:
1. `statemachines.rs` replaces the state machines of MLS chapter 17 by the
   clocked equations of MLS 17.3.4: `selectedState`, `activeState`,
   `nextState` and their resets per machine, plus the merged definitions of
   the variables the states define.
2. `clocks.rs` then:
   - solves the clock equations;
   - splits the clocked equations into partitions and infers their clocks;
   - lowers each base clock to one `when` clause that runs the solved
     equations of its partitions in dependency order.

**Inspection (`inspect.rs`)** serves the workbench. For any class it returns:
- the class tree;
- components with their placements;
- connections with their line annotations;
- the icon and diagram graphics layers;
- documentation and the experiment annotation.

`inspect.rs` parses annotations with a small recursive-descent parser over the
source text. This keeps annotations out of the simulation path.

## Expression arena (`src/expr.rs`)

Every expression is a hash-consed node in one arena. This gives three things:
- structurally equal subexpressions share an id, so CSE comes for free;
- symbolic differentiation and substitution are memoised by id;
- the code generator can share work across blocks.

## Backend (`src/backend`)

1. **Parameter ordering.** Parameters are sorted topologically by their binding
   dependencies and evaluated.
2. **Alias elimination (`alias.rs`).** Equations of the forms `a = b`,
   `a = -b` and `a = f(parameters)` are removed. Connector-heavy models lose
   about 40–60 % of their variables here; the 1000-mass chain loses 8,005 of 21,013.
3. **Index reduction (`index.rs`).**
   - `der(x)` becomes an ordinary variable.
   - Unknowns fixed by equations over parameters only are marked
     time-invariant. Their derivative is zero, so Pantelides never
     differentiates through them.
   - Pantelides' algorithm differentiates the minimal set of equations. It reads
     the visited set of a failed augmenting path from a trail, so its cost
     stays proportional to the region searched.
   - Dummy derivatives (Mattsson–Söderlind) are selected by `stateSelect` tier.
     The selection checks numerically at start values that the Jacobian of the
     differentiated equations with respect to the chosen dummies is non-singular.
   - Small blocks use Gram–Schmidt with column pivoting, for the best
     conditioning.
   - Large blocks use sparse greedy elimination over the columns in preference
     order, so selection stays linear for long chains.
4. **Matching and BLT (`graph.rs`, `sort.rs`).** Hopcroft–Karp matching and
   Tarjan's strongly connected components, both iterative, give the block
   lower-triangular order.
5. **Solving and tearing.**
   - A scalar block is solved symbolically when the unknown appears linearly.
     Otherwise it becomes a Newton block.
   - Algebraic loops are torn with a greedy heuristic. Linear loops are flagged
     so the runtime solves them in one factorisation.
6. **Initialization.** A separate system is built from the start values,
   `fixed=true` and the initial equations, and sorted the same way.

## Code generation (`src/codegen.rs`)

The causal system is compiled to a flat register bytecode, the "tape":
- it runs over one contiguous `f64` register file;
- common subexpressions are shared across blocks;
- Newton blocks carry exact symbolic Jacobians;
- the hot loop does no allocation, so the same tape runs natively and in
  WebAssembly.

The tape for the 10,000-mass chain has 360k instructions.

## Solvers (`src/solver.rs`, `src/sparse.rs`, `src/linalg.rs`, `src/sim.rs`)

**BDF.** Variable order 1–5 and variable step, the stiff default and the role
DASSL plays in Dymola.
- The Jacobian is built by colored finite differences, so it costs a few
  function evaluations, not n.
- Up to 48 states, Newton matrices use dense LU. Its singularity test is
  relative to each column, which avoids false singular reports when states have
  very different scales.
- Above 48 states it uses a left-looking Gilbert–Peierls sparse LU with a
  minimum-degree ordering. Threshold pivoting prefers the diagonal (threshold
  0.001), which keeps fill near zero for mechanical chains.

**Explicit methods.** Dopri5 is Dormand–Prince 5(4) with dense output. RK4 and
Euler run at a fixed step for real-time use.

**Events.**
- Relations are monitored as zero crossings and located on the dense output.
  Each relation has a hysteresis band relative to its own running magnitude,
  so a relation that hovers at zero does not fire twice for one crossing.
- If events keep switching the same relations back and forth at one instant
  (more than 500 events within a relative time of 1e-10), the run stops with
  an error naming those relations, instead of stalling.
- Event iteration re-evaluates `when` clauses, `reinit` and `pre()` until the
  discrete state is stable.
- `terminate()` ends the run cleanly.

## Results (`src/results.rs`)

Results are held in memory, exported as CSV, or written as a Dymola `dsres.mat`
(MATLAB v4 "binTrans"). Aliases are stored as signed references, as Dymola
does, so files stay small.

## Workbench (`web/`, `crates/daedalus-wasm`)

**Worker.** The `Workspace` (documents, class tree, compile, simulate, results)
is compiled to WebAssembly and runs in a Web Worker behind a small RPC layer.
The UI thread never blocks during translation or simulation.

**Diagram.** Annotations are drawn as SVG with explicit affine matrices. The
Modelica coordinate system is y-up, and text is kept upright.
- Components without a placement get a layered auto-layout.
- Connections without a line get orthogonal routing.

**Plot.** The plot is a canvas. It decimates each trace to a min and max per pixel
column, so drawing cost depends on the plot width, not on the number of
points. It supports:
- display units (rpm, bar, deg, degC, l/min, kJ);
- parametric x-axes;
- a crosshair readout;
- drag-to-zoom.

## Scaling notes

| Phase | 1,000 masses | 10,000 masses |
| --- | ---: | ---: |
| Parse (library + model) | 25 ms | 25 ms |
| Flatten | 110 ms | 1.4 s |
| Backend | 70 ms | 1.0 s |
| Simulate 1 s (BDF, 1e-6) | 0.42 s | 3.9 s |

The flattener is now the largest translation cost, and most of that is
allocation. Interning instance names and modifiers is the next step; see the
roadmap.
