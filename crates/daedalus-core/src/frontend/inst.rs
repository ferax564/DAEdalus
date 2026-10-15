//! Instantiation and flattening: class tree + top model name -> [`FlatModel`].
//!
//! Pass 1 builds the instance tree (components, array dimensions, conditional
//! components, modifiers). Pass 2 translates bindings and equations of every
//! instance into the flat expression arena, inlining functions by symbolic
//! execution. Connection sets are resolved last.

use super::overconstrained::{OcAnswers, OcLog};
use super::tree::{ClassId, ClassTree, ROOT, builtin_name};
use crate::expr::{Arena, Cmp, ExprId, Fn1, Fn2, LoopProg, Node, Table, VarId};
use crate::flat::*;
use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use rumoca_core::{ArrayConstructor, Causality, ClassType, OpBinary, OpUnary, Variability};
use rumoca_ir_ast as ast;
use rumoca_ir_ast::{Expression as E, TerminalType};
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

#[path = "clocks.rs"]
mod clocks;

pub type InstId = u32;

/// Lexical environment for class lookups, carrying redeclared class overrides.
pub struct Env {
    pub class: ClassId,
    pub overrides: Rc<FxHashMap<String, ClassId>>,
    pub parent: Option<Rc<Env>>,
}

#[derive(Clone)]
pub struct Scope {
    pub inst: Option<InstId>,
    pub env: Rc<Env>,
}

#[derive(Clone)]
pub struct Scoped<'a> {
    pub expr: &'a E,
    pub scope: Scope,
    /// Element selection for modifiers distributed over arrays of components
    /// (`c[n](p = {..})` gives element `k` the `k`-th slice): (k, n) pairs, outermost first.
    pub sel: Vec<(usize, usize)>,
}

/// Modification tree (MLS §7.2) with expressions bound to the scope they were written in.
#[derive(Clone, Default)]
pub struct Mod<'a> {
    pub value: Option<Scoped<'a>>,
    pub subs: Vec<(String, Mod<'a>)>,
    pub each: bool,
    pub final_: bool,
    /// `redeclare model/package X = Y`: class override (name, class expression, scope).
    pub redeclares: Vec<(String, &'a ast::ComponentReference, Scope)>,
    /// `redeclare T comp(...)`: new type of the modified component.
    pub retype: Option<(&'a ast::ComponentReference, Scope)>,
}

impl<'a> Mod<'a> {
    pub fn sub(&self, name: &str) -> Option<&Mod<'a>> {
        self.subs.iter().find(|(n, _)| n == name).map(|(_, m)| m)
    }

    fn set_path(&mut self, path: &[String], m: Mod<'a>) {
        if path.is_empty() {
            *self = merge(&m, self);
            return;
        }
        let head = &path[0];
        if let Some(pos) = self.subs.iter().position(|(n, _)| n == head) {
            self.subs[pos].1.set_path(&path[1..], m);
        } else {
            let mut child = Mod::default();
            child.set_path(&path[1..], m);
            self.subs.push((head.clone(), child));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_none() && self.subs.is_empty() && self.redeclares.is_empty()
    }
}

/// Merge two modifications: `outer` overrides `inner`.
pub fn merge<'a>(outer: &Mod<'a>, inner: &Mod<'a>) -> Mod<'a> {
    let mut r = inner.clone();
    if inner.final_ && inner.value.is_some() {
        // final modifiers cannot be overridden
    } else if outer.value.is_some() {
        r.value = outer.value.clone();
        r.each = outer.each;
    }
    for (n, om) in &outer.subs {
        if let Some(pos) = r.subs.iter().position(|(rn, _)| rn == n) {
            let merged = merge(om, &r.subs[pos].1);
            r.subs[pos].1 = merged;
        } else {
            r.subs.push((n.clone(), om.clone()));
        }
    }
    for rd in &outer.redeclares {
        r.redeclares.retain(|x| x.0 != rd.0);
        r.redeclares.push(rd.clone());
    }
    if outer.retype.is_some() {
        r.retype = outer.retype.clone();
    }
    r
}

#[derive(Clone, Debug)]
pub enum Comp {
    Var(VarId),
    Inst(InstId),
    /// Array of components (shared: cloning a component reference is O(1)).
    Arr(Vec<usize>, std::rc::Rc<Vec<Comp>>),
    /// String parameter or constant, evaluated at instantiation (strings never reach the
    /// flat model, but they select conditional components and replaceable choices).
    Str(std::rc::Rc<str>),
    Removed,
}

pub struct Instance<'a> {
    pub prefix: String,
    pub class: ClassId,
    pub env: Rc<Env>,
    pub comps: IndexMap<String, Comp>,
    /// Components declared `inner` (by name) for outer resolution.
    pub inners: FxHashSet<String>,
    /// `inner outer` components: the local name refers to the outer instance, and
    /// components below see this inner instance.
    pub inner_of: FxHashMap<String, Comp>,
    pub parent: Option<InstId>,
    pub is_connector: bool,
    /// Declared `final` (or inside a final component): its parameters cannot change.
    pub is_final: bool,
    /// Connector components (by name) of this instance.
    pub connector_comps: FxHashSet<String>,
    pub equations: Vec<(&'a ast::Equation, Rc<Env>)>,
    pub initial_equations: Vec<(&'a ast::Equation, Rc<Env>)>,
    pub algorithms: Vec<(&'a [ast::Statement], Rc<Env>)>,
    pub initial_algorithms: Vec<(&'a [ast::Statement], Rc<Env>)>,
}

/// Translation value: scalar expression, array of scalars, instance (array) or string.
#[derive(Clone, Debug)]
pub enum Val {
    S(ExprId),
    A(Vec<usize>, Vec<ExprId>),
    Inst(InstId),
    IA(Vec<usize>, Vec<InstId>),
    Str(String),
    /// Array of strings (`substanceNames`).
    SA(Vec<usize>, Vec<String>),
    /// Record value: ordered fields.
    Rec(Vec<(String, Val)>),
    /// Several function outputs (tuple).
    Tuple(Vec<Val>),
    /// A function value: `function f(a = 1)` passed to a functional input.
    Func(Rc<FuncVal>),
}

/// A function with some inputs bound (MLS 12.4.2, partial application).
pub struct FuncVal {
    cls: ClassId,
    overrides: Rc<FxHashMap<String, ClassId>>,
    /// Scope of the package the function was reached through (`Medium.f`).
    penv: Option<Rc<Env>>,
    bound: Vec<(String, Val)>,
}

impl std::fmt::Debug for FuncVal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "function #{} bound {:?}", self.cls, self.bound)
    }
}

impl Val {
    fn scalar(&self) -> Result<ExprId> {
        match self {
            Val::S(e) => Ok(*e),
            Val::A(d, v) if v.len() == 1 && d.iter().all(|&x| x == 1) => Ok(v[0]),
            Val::Tuple(v) if !v.is_empty() => v[0].scalar(),
            _ => bail!("expected a scalar expression, found {}", self.kind()),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Val::S(_) => "scalar",
            Val::A(..) => "array",
            Val::Inst(_) => "component",
            Val::IA(..) => "component array",
            Val::Str(_) => "string",
            Val::SA(..) => "string array",
            Val::Rec(_) => "record",
            Val::Tuple(_) => "tuple",
            Val::Func(_) => "function",
        }
    }

    /// Flatten to (dims, elements) treating scalars as 0-d arrays.
    fn elems(&self) -> Result<(Vec<usize>, Vec<ExprId>)> {
        match self {
            Val::S(e) => Ok((vec![], vec![*e])),
            Val::A(d, v) => Ok((d.clone(), v.clone())),
            Val::Tuple(v) if !v.is_empty() => v[0].elems(),
            _ => bail!("expected a numeric value, found {}", self.kind()),
        }
    }
}

fn mk_arr(dims: Vec<usize>, v: Vec<ExprId>) -> Val {
    if dims.is_empty() {
        Val::S(v[0])
    } else {
        Val::A(dims, v)
    }
}

struct PendingVar<'a> {
    var: VarId,
    m: Mod<'a>,
    /// Flat element index within the declared array (row-major) and total count.
    index: usize,
    count: usize,
    /// Declaration equation for a non-parameter variable goes into the equation list.
    is_param: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ConnPoint {
    var: VarId,
    inside: bool,
}

#[derive(Default, Clone)]
pub struct Ctx {
    locals: Vec<FxHashMap<String, Val>>,
    no_event: bool,
    end_value: Option<usize>,
    /// Inside a function body (no instance components visible).
    in_function: bool,
    /// Condition under which the statements being executed run (inside the branches of
    /// a time-varying `if`); `None` means always.
    path: Option<ExprId>,
    /// A `break` was executed unconditionally in the innermost loop.
    broke: bool,
    /// Run-time condition under which the function has already returned (a `return`
    /// under a time-varying `if`): later assignments only take effect when it is false.
    sret: Option<ExprId>,
    /// The same for a `break` in the innermost loop.
    sbrk: Option<ExprId>,
}

pub struct Flattener<'a, 't> {
    pub tree: &'t ClassTree<'a>,
    pub model: FlatModel,
    pub insts: Vec<Instance<'a>>,
    /// Modifiers of a redeclared class `redeclare function f = g(a = ..)` per instance.
    class_mods: FxHashMap<(InstId, String), Vec<(String, Scoped<'a>)>>,
    /// Number of connect equations per connector (for `cardinality`), keyed by its
    /// first leaf variable.
    conn_count: FxHashMap<VarId, usize>,
    counting_connects: bool,
    /// Stream variables of connectors, and the connector instance owning each
    /// variable declared directly in a connector.
    stream_vars: FxHashSet<VarId>,
    leaf_owner: FxHashMap<VarId, InstId>,
    /// `inStream(s)` requests: stream variable -> variable standing for the mixed value.
    in_stream: IndexMap<VarId, VarId>,
    pending: Vec<PendingVar<'a>>,
    env_cache: FxHashMap<ClassId, Rc<Env>>,
    const_cache: FxHashMap<(ClassId, String), Val>,
    const_in_progress: FxHashSet<(ClassId, String)>,
    /// Connection points union-find.
    conn_parent: FxHashMap<ConnPoint, ConnPoint>,
    conn_flow: FxHashMap<VarId, bool>,
    /// All flow variables of connectors that are inside connectors of some instance.
    connector_flows: Vec<VarId>,
    /// `semiLinear(x, s, h)` calls on plain variables: (result, x, s, h).
    semi_linear: Vec<(ExprId, VarId, VarId, ExprId)>,
    homotopy_var: Option<VarId>,
    /// Translated parameter bindings (lazy; for structural evaluation).
    param_values: FxHashMap<VarId, f64>,
    binding_done: FxHashSet<VarId>,
    binding_mods: FxHashMap<VarId, usize>,
    sample_count: usize,
    /// Native stand-ins for the MSL external table objects: (arena table, `columns`).
    ext_tables: Vec<(u32, Vec<usize>)>,
    /// Instances of expandable connectors (and connectors nested in them).
    expandable_insts: FxHashSet<InstId>,
    /// Variables declared inside expandable connectors.
    expandable_vars: Vec<VarId>,
    when_assigned: FxHashSet<VarId>,
    /// Variables of type `Clock`.
    clock_vars: FxHashSet<VarId>,
    /// Equations of `when <clock> then` clauses, with their clock.
    clocked_whens: Vec<(ExprId, Vec<Equation>)>,
    /// A synchronous operator was translated.
    uses_clocks: bool,
    depth: usize,
    /// Next id of a [`Node::Local`] standing for a loop variable while a loop body is
    /// translated (unique, so nested loops never mix their variables up).
    next_local: u32,
    /// Functions being compiled as callables (their body is inlined once), and the
    /// compiled ones by function and argument shape: (program, inputs, output template).
    outlining: FxHashSet<ClassId>,
    outlined: FxHashMap<String, (u32, usize, Vec<Val>)>,
    /// Functions that could not be compiled on their own.
    outline_failed: FxHashSet<ClassId>,
    /// Sizes of `[:]` array fields of record components bound as a whole
    /// (`cellData = data`): the bound record's own field sizes, by full name.
    dim_hints: FxHashMap<String, Vec<usize>>,
    /// Set while instantiating the inner part of an `inner outer` component.
    instantiating_inner: bool,
    pub var_by_name: FxHashMap<String, VarId>,
    /// Declared but not yet instantiated components, instantiated on first reference so
    /// that declaration order does not matter (e.g. conditions on later parameters).
    deferred: FxHashMap<(InstId, String), (&'a ast::Component, Rc<Env>, Mod<'a>)>,
    /// Overconstrained connection graph recorded during this run.
    pub oc_log: OcLog,
    /// Spanning-tree answers from a previous run (second run of an overconstrained model).
    pub oc_answers: Option<Rc<OcAnswers>>,
    oc_class: FxHashMap<ClassId, Option<ClassId>>,
    /// Structured components bound to a value as a whole (`Orientation R = nullRotation()`,
    /// `Complex c = a*b`): the value is distributed over the fields in pass 2.
    record_bindings: Vec<(InstId, Scoped<'a>)>,
}

const MAX_DEPTH: usize = 200;

impl<'a, 't> Flattener<'a, 't> {
    pub fn new(tree: &'t ClassTree<'a>) -> Self {
        Flattener {
            tree,
            model: FlatModel {
                arena: Arena::new(),
                ..Default::default()
            },
            insts: Vec::new(),
            class_mods: FxHashMap::default(),
            conn_count: FxHashMap::default(),
            counting_connects: false,
            stream_vars: FxHashSet::default(),
            leaf_owner: FxHashMap::default(),
            in_stream: IndexMap::new(),
            pending: Vec::new(),
            env_cache: FxHashMap::default(),
            const_cache: FxHashMap::default(),
            const_in_progress: FxHashSet::default(),
            conn_parent: FxHashMap::default(),
            conn_flow: FxHashMap::default(),
            connector_flows: Vec::new(),
            semi_linear: Vec::new(),
            homotopy_var: None,
            param_values: FxHashMap::default(),
            binding_done: FxHashSet::default(),
            binding_mods: FxHashMap::default(),
            sample_count: 0,
            ext_tables: Vec::new(),
            expandable_insts: FxHashSet::default(),
            expandable_vars: Vec::new(),
            when_assigned: FxHashSet::default(),
            clock_vars: FxHashSet::default(),
            clocked_whens: Vec::new(),
            uses_clocks: false,
            depth: 0,
            next_local: 1 << 20,
            outlining: FxHashSet::default(),
            outlined: FxHashMap::default(),
            outline_failed: FxHashSet::default(),
            dim_hints: FxHashMap::default(),
            instantiating_inner: false,
            var_by_name: FxHashMap::default(),
            deferred: FxHashMap::default(),
            oc_log: OcLog::default(),
            oc_answers: None,
            oc_class: FxHashMap::default(),
            record_bindings: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Environments and class lookup
    // ------------------------------------------------------------------

    pub fn env_of(&mut self, class: ClassId) -> Rc<Env> {
        if let Some(e) = self.env_cache.get(&class) {
            return e.clone();
        }
        let parent = self.tree.node(class).parent.map(|p| self.env_of(p));
        let e = Rc::new(Env {
            class,
            overrides: Rc::new(FxHashMap::default()),
            parent,
        });
        self.env_cache.insert(class, e.clone());
        e
    }

    fn env_with(&mut self, class: ClassId, overrides: Rc<FxHashMap<String, ClassId>>) -> Rc<Env> {
        if overrides.is_empty() {
            return self.env_of(class);
        }
        let parent = self.tree.node(class).parent.map(|p| self.env_of(p));
        Rc::new(Env {
            class,
            overrides,
            parent,
        })
    }

    fn lookup_class(&self, env: &Env, parts: &[&str]) -> Option<ClassId> {
        let first = *parts.first()?;
        let mut cur: Option<&Env> = Some(env);
        let mut found = None;
        while let Some(e) = cur {
            if let Some(&c) = e.overrides.get(first) {
                found = Some(c);
                break;
            }
            if let Some(c) = self.tree.lookup(e.class, &[first], None) {
                // tree.lookup walks lexical parents itself; but we still want overrides
                // of enclosing envs to win, so only accept direct members here.
                if self.tree.member_class(e.class, first) == Some(c) {
                    found = Some(c);
                    break;
                }
            }
            if let Some(c) = self.tree.lookup_import(e.class, first) {
                found = Some(c);
                break;
            }
            if e.parent.is_none() {
                found = self.tree.lookup(e.class, &[first], None);
                break;
            }
            cur = e.parent.as_deref();
        }
        let mut c = match found {
            Some(c) => c,
            None => *self.tree.node(ROOT).children.get(first)?,
        };
        for p in &parts[1..] {
            c = self.tree.member_class(c, p)?;
        }
        Some(c)
    }

    /// The class `name` that the enclosing class of `class` inherits (the base of
    /// `redeclare model extends name`), skipping `class` itself.
    fn inherited_class(&self, class: ClassId, name: &str) -> Option<ClassId> {
        let parent = self.tree.node(class).parent?;
        let mut stack = self.tree.bases(parent);
        let mut seen = FxHashSet::default();
        while let Some(b) = stack.pop() {
            if !seen.insert(b) {
                continue;
            }
            if let Some(&c) = self.tree.node(b).children.get(name)
                && c != class
            {
                return Some(c);
            }
            let mut more = self.tree.bases(b);
            more.reverse();
            stack.extend(more);
        }
        None
    }

    /// The package through which class `cls`, named `parts` in `env`, is reached when
    /// that package only inherits it (`Medium.BaseProperties` with `Medium` extending the
    /// package that declares it): the class then sees that package's constants and
    /// redeclarations before its own lexical scope.
    fn class_via(&self, env: &Env, parts: &[&str], cls: ClassId) -> Option<ClassId> {
        let via = if parts.len() > 1 {
            self.lookup_class(env, &parts[..parts.len() - 1])
        } else {
            let mut e: Option<&Env> = Some(env);
            let mut found = None;
            while let Some(en) = e {
                if en.overrides.contains_key(parts[0]) {
                    break;
                }
                if self.tree.member_class(en.class, parts[0]) == Some(cls) {
                    found = Some(en.class);
                    break;
                }
                e = en.parent.as_deref();
            }
            found
        }?;
        let lexical = self.tree.node(cls).parent;
        (Some(via) != lexical && self.tree.class_type(via) == ClassType::Package).then_some(via)
    }

    /// Whether class `c` inherits (transitively) from class `base`.
    fn inherits_from(&self, c: ClassId, base: ClassId) -> bool {
        let mut stack = self.tree.bases(c);
        let mut seen = FxHashSet::default();
        while let Some(b) = stack.pop() {
            if b == base {
                return true;
            }
            if seen.insert(b) {
                stack.extend(self.tree.bases(b));
            }
        }
        false
    }

    // ------------------------------------------------------------------
    // Modifications
    // ------------------------------------------------------------------

    fn mods_from_exprs(
        &self,
        exprs: &'a [E],
        each: &[bool],
        final_: &[bool],
        redeclare: &[bool],
        scope: &Scope,
    ) -> Mod<'a> {
        let mut m = Mod::default();
        for (i, e) in exprs.iter().enumerate() {
            let ea = each.get(i).copied().unwrap_or(false);
            let fi = final_.get(i).copied().unwrap_or(false);
            let rd = redeclare.get(i).copied().unwrap_or(false);
            self.add_mod_expr(&mut m, e, ea, fi, rd, scope);
        }
        m
    }

    fn add_mod_expr(
        &self,
        m: &mut Mod<'a>,
        e: &'a E,
        each: bool,
        final_: bool,
        redeclare: bool,
        scope: &Scope,
    ) {
        match e {
            E::Modification { target, value, .. } => {
                let path: Vec<String> = target
                    .parts
                    .iter()
                    .map(|p| p.ident.text.to_string())
                    .collect();
                if let E::ClassModification {
                    target: cls,
                    modifications,
                    each_flags,
                    final_flags,
                    redeclare_flags,
                    ..
                } = &**value
                    && (redeclare
                        || (modifications.is_empty() && self.looks_like_class(cls, scope)))
                {
                    if path.len() == 1 {
                        m.redeclares.push((path[0].clone(), cls, scope.clone()));
                    }
                    // Component redeclaration: new type plus its modifications.
                    let mut sub = self.mods_from_exprs(
                        modifications,
                        each_flags,
                        final_flags,
                        redeclare_flags,
                        scope,
                    );
                    sub.retype = Some((cls, scope.clone()));
                    sub.each = each;
                    m.set_path(&path, sub);
                    return;
                }
                let sub = Mod {
                    value: Some(Scoped {
                        expr: value,
                        scope: scope.clone(),
                        sel: Vec::new(),
                    }),
                    each,
                    final_,
                    ..Default::default()
                };
                m.set_path(&path, sub);
            }
            E::ClassModification {
                target,
                modifications,
                each_flags,
                final_flags,
                redeclare_flags,
                ..
            } => {
                let path: Vec<String> = target
                    .parts
                    .iter()
                    .map(|p| p.ident.text.to_string())
                    .collect();
                let mut sub = self.mods_from_exprs(
                    modifications,
                    each_flags,
                    final_flags,
                    redeclare_flags,
                    scope,
                );
                sub.each = each;
                sub.final_ = final_;
                m.set_path(&path, sub);
            }
            // `field(attr = ..) = value`
            E::Binary {
                op: rumoca_core::OpBinary::Assign,
                lhs,
                rhs,
                ..
            } if matches!(lhs.as_ref(), E::ClassModification { .. }) => {
                if let E::ClassModification {
                    target,
                    modifications,
                    each_flags,
                    final_flags,
                    redeclare_flags,
                    ..
                } = lhs.as_ref()
                {
                    let path: Vec<String> = target
                        .parts
                        .iter()
                        .map(|p| p.ident.text.to_string())
                        .collect();
                    let mut sub = self.mods_from_exprs(
                        modifications,
                        each_flags,
                        final_flags,
                        redeclare_flags,
                        scope,
                    );
                    sub.value = Some(Scoped {
                        expr: rhs,
                        scope: scope.clone(),
                        sel: Vec::new(),
                    });
                    sub.each = each;
                    sub.final_ = final_;
                    m.set_path(&path, sub);
                }
            }
            E::NamedArgument { name, value, .. } => {
                let sub = Mod {
                    value: Some(Scoped {
                        expr: value,
                        scope: scope.clone(),
                        sel: Vec::new(),
                    }),
                    each,
                    final_,
                    ..Default::default()
                };
                m.set_path(&[name.text.to_string()], sub);
            }
            _ => {}
        }
    }

    fn looks_like_class(&self, cref: &ast::ComponentReference, scope: &Scope) -> bool {
        let parts: Vec<&str> = cref.parts.iter().map(|p| &*p.ident.text).collect();
        self.lookup_class(&scope.env, &parts)
            .is_some_and(|c| !matches!(self.tree.class_type(c), ClassType::Function))
    }

    fn component_mod(&self, comp: &'a ast::Component, scope: &Scope) -> Mod<'a> {
        let mut m = self.mods_from_exprs(
            &comp.source_modifications,
            &comp.source_modification_each_flags,
            &comp.source_modification_final_flags,
            &comp.source_modification_redeclare_flags,
            scope,
        );
        if let Some(b) = &comp.binding {
            m.value = Some(Scoped {
                expr: b,
                scope: scope.clone(),
                sel: Vec::new(),
            });
        }
        if comp.is_final {
            m.final_ = true;
        }
        m
    }

    /// Attribute modifications contributed by a chain of short type definitions
    /// (`type Length = Real(unit="m")`).
    fn type_attr_mods(&mut self, class: ClassId) -> Mod<'a> {
        let mut out = Mod::default();
        let mut cur = Some(class);
        let mut guard = 0;
        while let Some(c) = cur {
            guard += 1;
            if guard > 32 {
                break;
            }
            let Some(def) = self.tree.def(c) else { break };
            let env = self.env_of(c);
            let scope = Scope { inst: None, env };
            let mut next = None;
            for ext in &def.extends {
                let exprs: Vec<&'a E> = ext.modifications.iter().map(|m| &m.expr).collect();
                let mut m = Mod::default();
                for (i, e) in exprs.iter().enumerate() {
                    let em = &ext.modifications[i];
                    self.add_mod_expr(&mut m, e, em.each, em.final_, em.redeclare, &scope);
                }
                out = merge(&out, &m);
                let parts: Vec<&str> = ext.base_name.name.iter().map(|t| &*t.text).collect();
                if parts.len() == 1 && builtin_name(parts[0]).is_some() {
                    next = None;
                } else {
                    let penv = self.env_of(self.tree.node(c).parent.unwrap_or(ROOT));
                    next = self.lookup_class(&penv, &parts);
                }
            }
            cur = next;
        }
        out
    }

    // ------------------------------------------------------------------
    // Pass 1: instance tree
    // ------------------------------------------------------------------

    pub fn flatten(&mut self, top: ClassId) -> Result<()> {
        self.model.name = self.tree.node(top).path.clone();
        let env = self.env_of(top);
        let m = Mod::default();
        let root = self.instantiate(top, String::new(), &m, env, None)?;
        // Experiment annotation
        if let Some(def) = self.tree.def(top) {
            self.read_experiment(&def.annotation);
        }
        // Pass 2: attribute/binding translation and equations
        self.translate_pending()?;
        self.count_connects()?;
        let n = self.insts.len();
        for i in 0..n {
            self.translate_instance(i as InstId)
                .with_context(|| format!("in component '{}'", self.inst_name(i as InstId)))?;
        }
        self.finish_streams();
        self.finish_connections();
        self.lower_clocks()?;
        self.finish_discrete();
        self.drop_unused_expandable();
        let _ = root;
        Ok(())
    }

    /// Members of expandable connectors that no connection or equation uses are not
    /// part of the model (MLS 9.1.3). They become hidden constants so ids stay stable.
    fn drop_unused_expandable(&mut self) {
        if self.expandable_vars.is_empty() {
            return;
        }
        let used = self.referenced_vars();
        for &v in &self.expandable_vars {
            if used.contains(&v) {
                continue;
            }
            let var = &mut self.model.vars[v as usize];
            var.kind = VarKind::Constant;
            var.binding = Some(Arena::ZERO);
            var.start = Some(Arena::ZERO);
            var.is_protected = true;
            var.generated = true;
        }
    }

    /// Variables referenced by any equation, when-clause, assertion or attribute.
    fn referenced_vars(&self) -> FxHashSet<VarId> {
        let m = &self.model;
        let mut roots: Vec<ExprId> = Vec::new();
        for e in m.equations.iter().chain(&m.initial_equations) {
            roots.push(e.lhs);
            roots.push(e.rhs);
        }
        for w in &m.whens {
            for (c, acts) in &w.branches {
                roots.push(*c);
                for a in acts {
                    match a {
                        WhenAction::Assign(v, e) | WhenAction::Reinit(v, e) => {
                            roots.push(m.arena.var_ref(*v));
                            roots.push(*e);
                        }
                        WhenAction::Assert(e, _) => roots.push(*e),
                        WhenAction::Terminate(_) => {}
                    }
                }
            }
        }
        roots.extend(m.asserts.iter().map(|(c, _)| *c));
        for v in &m.vars {
            roots.extend(
                [v.binding, v.start, v.nominal, v.min, v.max]
                    .into_iter()
                    .flatten(),
            );
        }
        m.arena.vars_in(&roots)
    }

    fn inst_name(&self, i: InstId) -> String {
        let p = &self.insts[i as usize].prefix;
        if p.is_empty() {
            self.model.name.clone()
        } else {
            p.clone()
        }
    }

    fn read_experiment(&mut self, ann: &[E]) {
        for a in ann {
            if let E::ClassModification {
                target,
                modifications,
                ..
            } = a
                && target.parts.len() == 1
                && &*target.parts[0].ident.text == "experiment"
            {
                for m in modifications {
                    if let E::Modification { target, value, .. } = m {
                        let key = target
                            .parts
                            .iter()
                            .map(|p| p.ident.text.to_string())
                            .collect::<Vec<_>>()
                            .join(".");
                        let num = literal_number(value);
                        match key.as_str() {
                            "StartTime" => self.model.experiment.start_time = num,
                            "StopTime" => self.model.experiment.stop_time = num,
                            "Tolerance" => self.model.experiment.tolerance = num,
                            "Interval" => self.model.experiment.interval = num,
                            "__Dymola_Algorithm" | "__Daedalus_Algorithm" => {
                                if let E::Terminal { token, .. } = &**value {
                                    self.model.experiment.algorithm =
                                        Some(token.text.trim_matches('"').to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    fn instantiate(
        &mut self,
        class: ClassId,
        prefix: String,
        m: &Mod<'a>,
        env: Rc<Env>,
        parent: Option<InstId>,
    ) -> Result<InstId> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            bail!("instantiation depth exceeded at '{prefix}' (recursive model?)");
        }
        // Class overrides from redeclare modifiers.
        let mut overrides: FxHashMap<String, ClassId> = (*env.overrides).clone();
        for (name, cls, scope) in &m.redeclares {
            let parts: Vec<&str> = cls.parts.iter().map(|p| &*p.ident.text).collect();
            if let Some(c) = self.lookup_class(&scope.env, &parts) {
                overrides.insert(name.clone(), c);
            }
        }
        let env = if overrides.len() == env.overrides.len() && m.redeclares.is_empty() {
            env
        } else {
            Rc::new(Env {
                class: env.class,
                overrides: Rc::new(overrides),
                parent: env.parent.clone(),
            })
        };
        let is_connector = matches!(self.tree.class_type(class), ClassType::Connector);
        let expandable = is_connector
            && (self.tree.def(class).is_some_and(|d| d.expandable)
                || parent.is_some_and(|p| self.expandable_insts.contains(&p)));
        if expandable {
            self.expandable_insts.insert(self.insts.len() as InstId);
        }
        let id = self.insts.len() as InstId;
        for (name, _, _) in &m.redeclares {
            if let Some(sub) = m.sub(name) {
                let vals: Vec<(String, Scoped<'a>)> = sub
                    .subs
                    .iter()
                    .filter_map(|(n, sm)| sm.value.clone().map(|v| (n.clone(), v)))
                    .collect();
                if !vals.is_empty() {
                    self.class_mods.insert((id, name.clone()), vals);
                }
            }
        }
        self.insts.push(Instance {
            prefix: prefix.clone(),
            class,
            env: env.clone(),
            comps: IndexMap::new(),
            inners: FxHashSet::default(),
            inner_of: FxHashMap::default(),
            parent,
            is_connector,
            is_final: m.final_ || parent.is_some_and(|p| self.insts[p as usize].is_final),
            connector_comps: FxHashSet::default(),
            equations: Vec::new(),
            initial_equations: Vec::new(),
            algorithms: Vec::new(),
            initial_algorithms: Vec::new(),
        });
        let mut elems: Vec<(&'a ast::Component, Rc<Env>, Mod<'a>)> = Vec::new();
        self.collect(class, env.clone(), m, id, &mut elems, 0)?;
        let names: Vec<&'a str> = elems.iter().map(|(c, _, _)| c.name.as_str()).collect();
        for (comp, cenv, cm) in elems.into_iter().rev() {
            // first declaration wins (later duplicates come from inherited classes)
            self.deferred
                .insert((id, comp.name.clone()), (comp, cenv, cm));
        }
        for name in names {
            self.ensure_component(id, name)?;
        }
        self.depth -= 1;
        Ok(id)
    }

    /// Instantiate a deferred component of `inst` now, if it has not been yet.
    fn ensure_component(&mut self, inst: InstId, name: &str) -> Result<()> {
        if let Some((comp, cenv, cm)) = self.deferred.remove(&(inst, name.to_string())) {
            if self.insts[inst as usize].comps.contains_key(name) {
                return Ok(());
            }
            let prefix = self.insts[inst as usize].prefix.clone();
            self.instantiate_component(inst, comp, cenv, cm)
                .with_context(|| format!("instantiating '{}{}'", dot(&prefix), comp.name))?;
        }
        Ok(())
    }

    /// Collect components (own + inherited) with their effective modifiers.
    fn collect(
        &mut self,
        class: ClassId,
        env: Rc<Env>,
        m: &Mod<'a>,
        inst: InstId,
        out: &mut Vec<(&'a ast::Component, Rc<Env>, Mod<'a>)>,
        depth: usize,
    ) -> Result<()> {
        if depth > 32 {
            bail!("extends depth exceeded");
        }
        let Some(def) = self.tree.def(class) else {
            return Ok(());
        };
        let scope = Scope {
            inst: Some(inst),
            env: env.clone(),
        };
        for ext in &def.extends {
            let parts: Vec<&str> = ext.base_name.name.iter().map(|t| &*t.text).collect();
            if parts.len() == 1 && builtin_name(parts[0]).is_some() {
                continue;
            }
            // Extends names are resolved in the enclosing scope (with overrides).
            let lookup_env = match &env.parent {
                Some(p) => Rc::new(Env {
                    class: p.class,
                    overrides: env.overrides.clone(),
                    parent: p.parent.clone(),
                }),
                None => env.clone(),
            };
            // `redeclare model extends X` inside X: the base is the inherited X.
            let class_extends = parts.len() == 1 && self.tree.node(class).name == parts[0];
            let mut base = if class_extends {
                class
            } else {
                self.lookup_class(&lookup_env, &parts)
                    .or_else(|| self.lookup_class(&env, &parts))
                    .ok_or_else(|| anyhow!("base class '{}' not found", ext.base_name))?
            };
            if base == class {
                // `redeclare model extends X`: the base is the inherited X, not this class.
                base = self
                    .inherited_class(class, parts[0])
                    .ok_or_else(|| anyhow!("base class '{}' not found", ext.base_name))?;
            }
            let mut em = Mod::default();
            for x in &ext.modifications {
                self.add_mod_expr(&mut em, &x.expr, x.each, x.final_, x.redeclare, &scope);
            }
            let merged = merge(m, &em);
            // Modifiers of `redeclare function f = g(a = ..)` in the extends clause.
            for (name, _, _) in &em.redeclares {
                if let Some(sub) = merged.sub(name) {
                    let vals: Vec<(String, Scoped<'a>)> = sub
                        .subs
                        .iter()
                        .filter_map(|(n, sm)| sm.value.clone().map(|v| (n.clone(), v)))
                        .collect();
                    if !vals.is_empty() {
                        self.class_mods.entry((inst, name.clone())).or_insert(vals);
                    }
                }
            }
            // Redeclares in extends apply to the instance environment for inherited elements.
            let mut ov: FxHashMap<String, ClassId> = (*env.overrides).clone();
            for (name, cls, sc) in &merged.redeclares {
                let p: Vec<&str> = cls.parts.iter().map(|p| &*p.ident.text).collect();
                if let Some(c) = self.lookup_class(&sc.env, &p) {
                    ov.insert(name.clone(), c);
                }
            }
            // A base inherited from the package that encloses it in a base of our own
            // enclosing package (`redeclare model extends BaseProperties`) resolves names
            // in our enclosing package first, so it sees its redeclared records.
            let blex = self.tree.node(base).parent;
            let benv = match (&env.parent, blex) {
                (Some(pe), Some(bl)) if pe.class != bl && self.inherits_from(pe.class, bl) => {
                    let ov = Rc::new(ov);
                    let p = self.inherited_env(pe.class, bl, ov.clone());
                    Rc::new(Env {
                        class: base,
                        overrides: ov,
                        parent: Some(p),
                    })
                }
                _ => self.env_with(base, Rc::new(ov)),
            };
            self.collect(base, benv.clone(), &merged, inst, out, depth + 1)?;
            // Make inherited overrides visible to the instance env as well.
            if !benv.overrides.is_empty() {
                let ienv = &self.insts[inst as usize].env;
                if ienv.overrides.len() < benv.overrides.len() {
                    let mut ov2 = (*ienv.overrides).clone();
                    for (k, v) in benv.overrides.iter() {
                        ov2.entry(k.clone()).or_insert(*v);
                    }
                    let ne = Rc::new(Env {
                        class: ienv.class,
                        overrides: Rc::new(ov2),
                        parent: ienv.parent.clone(),
                    });
                    self.insts[inst as usize].env = ne;
                }
            }
        }
        for (name, comp) in &def.components {
            let dm = self.component_mod(comp, &scope);
            let em = m.sub(name).cloned().unwrap_or_default();
            let cm = merge(&em, &dm);
            out.push((comp, env.clone(), cm));
        }
        let ins = &mut self.insts[inst as usize];
        for eq in &def.equations {
            ins.equations.push((eq, env.clone()));
        }
        for eq in &def.initial_equations {
            ins.initial_equations.push((eq, env.clone()));
        }
        for alg in &def.algorithms {
            ins.algorithms.push((alg.as_slice(), env.clone()));
        }
        for alg in &def.initial_algorithms {
            ins.initial_algorithms.push((alg.as_slice(), env.clone()));
        }
        Ok(())
    }

    fn instantiate_component(
        &mut self,
        inst: InstId,
        comp: &'a ast::Component,
        env: Rc<Env>,
        m: Mod<'a>,
    ) -> Result<()> {
        let name = comp.name.clone();
        let scope = Scope {
            inst: Some(inst),
            env: env.clone(),
        };
        if let Some(cond) = &comp.condition {
            let v = self.eval_structural(cond, &scope)?;
            if v == 0.0 {
                self.insts[inst as usize].comps.insert(name, Comp::Removed);
                return Ok(());
            }
        }
        if comp.outer && !std::mem::take(&mut self.instantiating_inner) {
            let mut p = self.insts[inst as usize].parent;
            while let Some(pi) = p {
                // The inner may not be instantiated yet.
                self.ensure_component(pi, &name)?;
                let pinst = &self.insts[pi as usize];
                if pinst.inners.contains(&name)
                    && let Some(c) = pinst
                        .inner_of
                        .get(&name)
                        .or_else(|| pinst.comps.get(&name))
                        .cloned()
                {
                    if comp.inner {
                        // `inner outer`: instantiate the inner part, then point the local
                        // name at the outer one.
                        self.instantiating_inner = true;
                        self.instantiate_component(inst, comp, env, m)?;
                        if let Some(own) = self.insts[inst as usize].comps.insert(name.clone(), c) {
                            self.insts[inst as usize].inner_of.insert(name, own);
                        }
                        return Ok(());
                    }
                    self.insts[inst as usize].comps.insert(name, c);
                    return Ok(());
                }
                p = self.insts[pi as usize].parent;
            }
            if !comp.inner {
                self.model.warnings.push(format!(
                    "no inner declaration for outer component '{}'; using a local instance",
                    name
                ));
            }
        }
        if comp.inner {
            self.insts[inst as usize].inners.insert(name.clone());
        }
        // Type resolution
        let tparts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
        let builtin = if tparts.len() == 1 {
            builtin_name(tparts[0])
        } else {
            None
        };
        let tclass = if builtin.is_some() {
            None
        } else {
            match &m.retype {
                Some((cls, sc)) => {
                    let parts: Vec<&str> = cls.parts.iter().map(|p| &*p.ident.text).collect();
                    Some(
                        self.lookup_class(&sc.env, &parts)
                            .ok_or_else(|| anyhow!("type '{}' not found", cls))?,
                    )
                }
                None => Some(
                    self.lookup_class(&env, &tparts)
                        .ok_or_else(|| anyhow!("type '{}' not found", comp.type_name))?,
                ),
            }
        };
        // Dimensions
        let mut dims = Vec::new();
        // A field of a record bound as a whole takes the bound field's size, also when its
        // declared size depends on a field the binding overrides (`rcData[nRC]`).
        let hint = self
            .dim_hints
            .get(&format!(
                "{}{}",
                dot(&self.insts[inst as usize].prefix),
                name
            ))
            .cloned();
        for s in &comp.shape_expr {
            match s {
                ast::Subscript::Expression(_)
                    if let Some(&d) = hint.as_ref().and_then(|h| h.get(dims.len())) =>
                {
                    dims.push(d);
                }
                ast::Subscript::Expression(e) => {
                    let v = self.eval_structural(e, &scope)?;
                    dims.push(v.round().max(0.0) as usize);
                }
                ast::Subscript::Range { .. } | ast::Subscript::Empty => {
                    // A field of a record bound as a whole takes the bound field's size.
                    let key = format!("{}{}", dot(&self.insts[inst as usize].prefix), name);
                    if let Some(h) = self.dim_hints.get(&key)
                        && let Some(&d) = h.get(dims.len())
                    {
                        dims.push(d);
                        continue;
                    }
                    // Unknown size `[:]`: derive from the binding value if possible.
                    let d = if let Some(v) = &m.value {
                        // `{a, b}`: the size without evaluating the elements (they may
                        // refer to components not instantiated yet).
                        if dims.is_empty()
                            && let E::Array {
                                elements,
                                kind: ArrayConstructor::Array,
                                ..
                            } = v.expr
                        {
                            dims.push(elements.len());
                            continue;
                        }
                        let val = self.translate_scoped(v, &mut Ctx::default())?;
                        match val {
                            Val::A(d, _) => d.get(dims.len()).copied().unwrap_or(0),
                            Val::IA(d, _) => d.get(dims.len()).copied().unwrap_or(0),
                            _ => 1,
                        }
                    } else {
                        0
                    };
                    dims.push(d);
                }
            }
        }
        // Dimensions of the type, along its chain of short definitions and extends
        // (`type Orientation extends TransformationMatrix`, `type TransformationMatrix = Real[3, 3]`).
        let mut tcur = tclass;
        let mut tdepth = 0;
        while let Some(tc) = tcur
            && tdepth < 16
        {
            tdepth += 1;
            let Some(td) = self.tree.def(tc) else { break };
            if !matches!(self.tree.class_type(tc), ClassType::Type) && tdepth > 1 {
                break;
            }
            let tenv = self.env_of(tc);
            for s in &td.array_subscripts {
                if let ast::Subscript::Expression(e) = s {
                    let v = self.eval_structural(
                        e,
                        &Scope {
                            inst: None,
                            env: tenv.clone(),
                        },
                    )?;
                    dims.push(v as usize);
                }
            }
            tcur = self.tree.bases(tc).into_iter().next();
        }
        let count: usize = dims.iter().product();
        let base = builtin.or_else(|| tclass.and_then(|c| self.tree.builtin_base(c)));
        let is_connector_type =
            tclass.is_some_and(|c| matches!(self.tree.class_type(c), ClassType::Connector));
        if is_connector_type {
            self.insts[inst as usize]
                .connector_comps
                .insert(name.clone());
        }
        let prefix = self.insts[inst as usize].prefix.clone();
        let full = format!("{}{}", dot(&prefix), name);
        if let Some(b) = base {
            // Scalar (or array of scalar) variables.
            let mut tm = match tclass {
                Some(tc) => self.type_attr_mods(tc),
                None => Mod::default(),
            };
            tm = merge(&m, &tm);
            let ty = match b {
                "Real" => VarType::Real,
                "Integer" => VarType::Integer,
                "Boolean" => VarType::Boolean,
                "String" => {
                    let mut c = Comp::Removed;
                    if dims.is_empty() {
                        let src = tm
                            .value
                            .clone()
                            .or_else(|| tm.sub("start").and_then(|s| s.value.clone()));
                        let v = match src {
                            Some(sc) => match self.translate_scoped(&sc, &mut Ctx::default()) {
                                Ok(Val::Str(x)) => Some(x),
                                _ => None,
                            },
                            None => Some(String::new()),
                        };
                        if let Some(x) = v {
                            c = Comp::Str(x.into());
                        }
                    } else if let Some(sc) = tm
                        .value
                        .clone()
                        .or_else(|| tm.sub("start").and_then(|s| s.value.clone()))
                        && let Ok(Val::SA(d, xs)) = self.translate_scoped(&sc, &mut Ctx::default())
                        && d.iter().product::<usize>() == count
                    {
                        c = Comp::Arr(
                            dims.clone(),
                            Rc::new(xs.into_iter().map(|x| Comp::Str(x.into())).collect()),
                        );
                    }
                    self.insts[inst as usize].comps.insert(name, c);
                    return Ok(());
                }
                "ExternalObject" => VarType::Integer,
                "Clock" => VarType::Real,
                _ => VarType::Enumeration,
            };
            let mut kind = match &comp.variability {
                // An external object (table handle) is fixed for the whole simulation.
                _ if b == "ExternalObject" => VarKind::Parameter,
                _ if b == "Clock" => VarKind::Discrete,
                Variability::Constant(_) => VarKind::Constant,
                Variability::Parameter(_) => VarKind::Parameter,
                Variability::Discrete(_) => VarKind::Discrete,
                _ => {
                    if ty == VarType::Real {
                        VarKind::Continuous
                    } else {
                        VarKind::Discrete
                    }
                }
            };
            // Causality from the declaration or the type (connector RealInput = input Real).
            let causality_input = matches!(comp.causality, Causality::Input(_))
                || tclass.is_some_and(|c| self.type_is_input(c));
            let causality_output = matches!(comp.causality, Causality::Output(_))
                || tclass.is_some_and(|c| self.type_is_output(c));
            let at_top = prefix.is_empty() || self.is_top_connector(inst);
            if causality_input && at_top && matches!(kind, VarKind::Continuous | VarKind::Discrete)
            {
                kind = VarKind::Input;
            }
            let is_flow = matches!(comp.connection, ast::Connection::Flow(_));
            let desc: String = comp
                .description
                .iter()
                .map(|t| t.text.trim_matches('"').to_string())
                .collect::<Vec<_>>()
                .join("");
            let mut elems = Vec::with_capacity(count.max(1));
            let n = if dims.is_empty() { 1 } else { count };
            for k in 0..n {
                let vname = if dims.is_empty() {
                    full.clone()
                } else {
                    format!("{}[{}]", full, index_str(&dims, k))
                };
                let mut v = Variable::new(vname.clone(), kind, ty);
                v.is_flow = is_flow;
                v.description = desc.clone();
                v.is_protected = comp.is_protected;
                v.is_final = comp.is_final || self.insts[inst as usize].is_final;
                v.is_output = causality_output && prefix.is_empty();
                let vid = self.model.add_var(v);
                if b == "Clock" {
                    self.clock_vars.insert(vid);
                }
                if self.expandable_insts.contains(&inst) {
                    self.expandable_vars.push(vid);
                }
                self.var_by_name.insert(vname, vid);
                if self.insts[inst as usize].is_connector {
                    self.leaf_owner.insert(vid, inst);
                    if matches!(comp.connection, ast::Connection::Stream(_)) {
                        self.stream_vars.insert(vid);
                    }
                }
                if is_flow && self.insts[inst as usize].is_connector {
                    self.connector_flows.push(vid);
                }
                if is_flow && self.insts[inst as usize].is_connector {
                    self.conn_flow.insert(vid, true);
                }
                let is_param = matches!(kind, VarKind::Parameter | VarKind::Constant);
                self.pending.push(PendingVar {
                    var: vid,
                    m: tm.clone(),
                    index: k,
                    count: n,
                    is_param,
                });
                if is_param {
                    self.binding_mods.insert(vid, self.pending.len() - 1);
                }
                elems.push(Comp::Var(vid));
            }
            // A scalar connector var itself (RealInput u) is a connector component.
            if is_connector_type && is_flow {
                for c in &elems {
                    if let Comp::Var(v) = c {
                        self.connector_flows.push(*v);
                    }
                }
            }
            let c = if dims.is_empty() {
                elems.pop().unwrap()
            } else {
                Comp::Arr(dims, std::rc::Rc::new(elems))
            };
            self.insts[inst as usize].comps.insert(name, c);
            return Ok(());
        }
        let tc = tclass.unwrap();
        if matches!(
            self.tree.class_type(tc),
            ClassType::Function | ClassType::Package
        ) {
            self.insts[inst as usize].comps.insert(name, Comp::Removed);
            return Ok(());
        }
        let via = if m.retype.is_none() {
            self.class_via(&env, &tparts, tc)
        } else {
            None
        };
        let tenv = if let Some(v) = via {
            let lexical = self.tree.node(tc).parent.unwrap_or(ROOT);
            let p = self.inherited_env(v, lexical, env.overrides.clone());
            Rc::new(Env {
                class: tc,
                overrides: env.overrides.clone(),
                parent: Some(p),
            })
        } else {
            let te = self.env_of(tc);
            let ov = env.overrides.clone();
            if ov.is_empty() {
                te
            } else {
                Rc::new(Env {
                    class: tc,
                    overrides: ov,
                    parent: te.parent.clone(),
                })
            }
        };
        // Record parameters (`parameter Rec r`) propagate variability to fields.
        let var_prefix = comp.variability.clone();
        let mut elems = Vec::new();
        let n = if dims.is_empty() { 1 } else { count };
        for k in 0..n {
            let cprefix = if dims.is_empty() {
                full.clone()
            } else {
                format!("{}[{}]", full, index_str(&dims, k))
            };
            let cm = if !dims.is_empty() {
                select_mod_element(&m, k, n)
            } else {
                m.clone()
            };
            let mut bound_src = None;
            if let Some(v) = &cm.value
                && matches!(v.expr, E::ComponentReference(_))
                && let Ok(Val::Inst(src)) = self.translate_scoped(v, &mut Ctx::default())
            {
                self.collect_dim_hints(src, &cprefix);
                bound_src = Some(src);
            } else if let Some(v) = &cm.value
                && matches!(
                    v.expr,
                    E::Array { .. } | E::ArrayComprehension { .. } | E::If { .. }
                )
            {
                // An element of a record array built from records (`{{if degraded then
                // dataDegraded else dataOriginal for kp in 1:Np} for ks in 1:Ns}`): its
                // fields take the sizes of the selected record, not of the declaration's
                // default (`OCV_SOC[:,2] = [SOCmin, ..; SOCmax, 1]`).
                match self.translate_scoped(v, &mut Ctx::default()) {
                    Ok(Val::Inst(src)) => self.collect_dim_hints(src, &cprefix),
                    Ok(Val::Rec(fields)) => self.rec_dim_hints(&fields, &cprefix),
                    _ => {}
                }
            }
            let child = self.instantiate(tc, cprefix, &cm, tenv.clone(), Some(inst))?;
            if let Some(src) = bound_src {
                self.alias_param_fields(child, src);
            }
            if !matches!(var_prefix, Variability::Empty) {
                self.propagate_variability(child, &var_prefix);
            }
            if let Some(v) = &cm.value {
                self.record_bindings.push((child, v.clone()));
            }
            // `flow Complex i`: the prefix applies to every field.
            if matches!(comp.connection, ast::Connection::Flow(_)) {
                let mut leaves = Vec::new();
                self.connector_leaves(&Comp::Inst(child), &mut leaves);
                let in_connector = self.insts[inst as usize].is_connector;
                for v in leaves {
                    self.model.vars[v as usize].is_flow = true;
                    if in_connector {
                        self.connector_flows.push(v);
                        self.conn_flow.insert(v, true);
                    }
                }
            }
            elems.push(Comp::Inst(child));
        }
        let c = if dims.is_empty() {
            elems.pop().unwrap()
        } else {
            Comp::Arr(dims, std::rc::Rc::new(elems))
        };
        self.insts[inst as usize].comps.insert(name, c);
        Ok(())
    }

    fn propagate_variability(&mut self, inst: InstId, v: &Variability) {
        let comps: Vec<Comp> = self.insts[inst as usize].comps.values().cloned().collect();
        for c in comps {
            self.propagate_comp(&c, v);
        }
    }

    fn propagate_comp(&mut self, c: &Comp, v: &Variability) {
        match c {
            Comp::Var(id) => {
                let var = &mut self.model.vars[*id as usize];
                match v {
                    Variability::Parameter(_) if var.kind != VarKind::Constant => {
                        var.kind = VarKind::Parameter;
                        if let Some(pos) = self.pending.iter().position(|p| p.var == *id) {
                            self.pending[pos].is_param = true;
                            self.binding_mods.insert(*id, pos);
                        }
                    }
                    Variability::Constant(_) => {
                        var.kind = VarKind::Constant;
                        if let Some(pos) = self.pending.iter().position(|p| p.var == *id) {
                            self.pending[pos].is_param = true;
                            self.binding_mods.insert(*id, pos);
                        }
                    }
                    Variability::Discrete(_) => var.kind = VarKind::Discrete,
                    _ => {}
                }
            }
            Comp::Inst(i) => self.propagate_variability(*i, v),
            Comp::Arr(_, v2) => {
                let v2 = v2.clone();
                for x in v2.iter() {
                    self.propagate_comp(x, v);
                }
            }
            Comp::Removed | Comp::Str(_) => {}
        }
    }

    fn type_is_input(&self, c: ClassId) -> bool {
        self.type_causality(c, 0) == 1
    }

    fn type_is_output(&self, c: ClassId) -> bool {
        self.type_causality(c, 0) == 2
    }

    fn type_causality(&self, c: ClassId, depth: usize) -> u8 {
        if depth > 16 {
            return 0;
        }
        let Some(d) = self.tree.def(c) else { return 0 };
        match d.causality {
            Causality::Input(_) => return 1,
            Causality::Output(_) => return 2,
            _ => {}
        }
        for b in self.tree.bases(c) {
            let r = self.type_causality(b, depth + 1);
            if r != 0 {
                return r;
            }
        }
        0
    }

    fn is_top_connector(&self, inst: InstId) -> bool {
        let i = &self.insts[inst as usize];
        i.is_connector
            && i.parent
                .is_some_and(|p| self.insts[p as usize].prefix.is_empty())
    }

    // ------------------------------------------------------------------
    // Structural evaluation
    // ------------------------------------------------------------------

    /// Evaluate an expression that must be known at translation time.
    pub fn eval_structural(&mut self, e: &'a E, scope: &Scope) -> Result<f64> {
        // `Real x[E]` / `x[Boolean]`: a dimension given by an enumeration or Boolean type.
        if let E::ComponentReference(cr) = e
            && cr.parts.iter().all(|p| p.subs.is_none())
        {
            let parts: Vec<&str> = cr.parts.iter().map(|p| &*p.ident.text).collect();
            if parts == ["Boolean"] {
                return Ok(2.0);
            }
            if let Some(c) = self.lookup_class(&scope.env, &parts)
                && let Some(n) = self.enum_size(c)
            {
                return Ok(n as f64);
            }
        }
        let v = self.translate(e, scope, &mut Ctx::default())?;
        let x = v.scalar()?;
        self.eval_param_expr(x).ok_or_else(|| {
            anyhow!(
                "expression '{}' must be a constant or parameter expression",
                self.model.show(x)
            )
        })
    }

    /// Numerically evaluate an expression over parameters/constants (following bindings).
    pub fn eval_param_expr(&mut self, e: ExprId) -> Option<f64> {
        if let Some(c) = self.model.arena.as_const(e) {
            return Some(c);
        }
        let mut leaves = Vec::new();
        self.model.arena.visit_leaves(e, &mut |n| leaves.push(n));
        let mut map = FxHashMap::default();
        for l in leaves {
            match l {
                Node::Var(v) => {
                    let val = self.param_value(v)?;
                    map.insert(v, val);
                }
                _ => return None,
            }
        }
        let arena = &mut self.model.arena;
        let consts: FxHashMap<VarId, ExprId> =
            map.iter().map(|(&k, &v)| (k, arena.constant(v))).collect();
        let r = arena.substitute(e, &|n| {
            if let Node::Var(v) = n {
                consts.get(&v).copied()
            } else {
                None
            }
        });
        arena.as_const(r)
    }

    fn param_value(&mut self, v: VarId) -> Option<f64> {
        if let Some(&x) = self.param_values.get(&v) {
            return Some(x);
        }
        let var = &self.model.vars[v as usize];
        if !matches!(var.kind, VarKind::Parameter | VarKind::Constant) {
            return None;
        }
        self.ensure_binding(v).ok()?;
        let var = &self.model.vars[v as usize];
        let b = var.binding.or(var.start)?;
        let r = self.eval_param_expr(b)?;
        self.param_values.insert(v, r);
        Some(r)
    }

    /// Translate the binding/start of a parameter on demand (needed for structural parameters).
    fn ensure_binding(&mut self, v: VarId) -> Result<()> {
        if self.binding_done.contains(&v) {
            return Ok(());
        }
        let Some(&pi) = self.binding_mods.get(&v) else {
            return Ok(());
        };
        self.binding_done.insert(v);
        self.translate_attrs(pi)
    }

    // ------------------------------------------------------------------
    // Pass 2: translation
    // ------------------------------------------------------------------

    /// Parameter fields of a record bound as a whole to `src` take `src`'s fields as their
    /// bindings right away, so sizes declared from them (`resistor[cellData.nRC]`) see the
    /// bound values rather than the record's defaults.
    fn alias_param_fields(&mut self, child: InstId, src: InstId) {
        let comps: Vec<(String, Comp)> = self.insts[child as usize]
            .comps
            .iter()
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        let o = self.model.origin("record binding");
        for (n, c) in comps {
            let Some(sc) = self.insts[src as usize].comps.get(&n).cloned() else {
                continue;
            };
            match (c, sc) {
                (Comp::Var(cv), Comp::Var(sv))
                    if matches!(
                        self.model.vars[cv as usize].kind,
                        VarKind::Parameter | VarKind::Constant
                    ) =>
                {
                    let e = self.model.arena.var(sv);
                    self.bind_var(cv, e, o);
                }
                (Comp::Inst(ci), Comp::Inst(si)) => self.alias_param_fields(ci, si),
                _ => {}
            }
        }
    }

    /// Record the array sizes of a record value's fields under `prefix` (see `dim_hints`).
    fn rec_dim_hints(&mut self, fields: &[(String, Val)], prefix: &str) {
        for (n, v) in fields {
            let key = format!("{prefix}.{n}");
            match v {
                Val::A(d, _) | Val::IA(d, _) => {
                    self.dim_hints.insert(key, d.clone());
                }
                Val::Rec(f) => self.rec_dim_hints(f, &key),
                Val::Inst(i) => self.collect_dim_hints(*i, &key),
                _ => {}
            }
        }
    }

    /// Record the array sizes of instance `src`'s fields under `prefix` (see `dim_hints`).
    fn collect_dim_hints(&mut self, src: InstId, prefix: &str) {
        let comps: Vec<(String, Comp)> = self.insts[src as usize]
            .comps
            .iter()
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        for (n, c) in comps {
            let key = format!("{prefix}.{n}");
            match c {
                Comp::Arr(d, elems) => {
                    // An array of records: each element's fields too (`cellData[Ns, Np]`).
                    for (k, e) in elems.iter().enumerate() {
                        if let Comp::Inst(i) = e {
                            self.collect_dim_hints(*i, &format!("{key}[{}]", index_str(&d, k)));
                        }
                    }
                    self.dim_hints.insert(key, d);
                }
                Comp::Inst(i) => self.collect_dim_hints(i, &key),
                _ => {}
            }
        }
    }

    /// Distribute whole-component bindings over the fields (see `record_bindings`).
    fn translate_record_bindings(&mut self) -> Result<()> {
        let rb = std::mem::take(&mut self.record_bindings);
        for (inst, sc) in rb {
            let v = self
                .translate_scoped(&sc, &mut Ctx::default())
                .with_context(|| {
                    format!("in declaration of '{}'", self.insts[inst as usize].prefix)
                })?;
            let o = self.model.origin(format!(
                "declaration of {}",
                self.insts[inst as usize].prefix
            ));
            self.bind_fields(inst, v, o).with_context(|| {
                format!("in declaration of '{}'", self.insts[inst as usize].prefix)
            })?;
        }
        Ok(())
    }

    fn bind_fields(&mut self, inst: InstId, v: Val, o: u32) -> Result<()> {
        let rec = self.as_record(&v)?;
        let Val::Rec(fields) = rec else {
            unreachable!()
        };
        let comps: Vec<(String, Comp)> = self.insts[inst as usize]
            .comps
            .iter()
            .map(|(k, c)| (k.clone(), c.clone()))
            .collect();
        for (name, c) in comps {
            let Some((_, fv)) = fields.iter().find(|(n, _)| *n == name) else {
                continue;
            };
            self.bind_comp(&c, fv.clone(), o)?;
        }
        Ok(())
    }

    fn bind_comp(&mut self, c: &Comp, v: Val, o: u32) -> Result<()> {
        match c {
            Comp::Var(var) => {
                let e = v.scalar()?;
                self.bind_var(*var, e, o);
            }
            Comp::Arr(_, el) if el.iter().all(|x| matches!(x, Comp::Var(_))) => {
                let (_, xs) = v.elems()?;
                for (k, x) in el.iter().enumerate() {
                    if let Comp::Var(var) = x {
                        let e = *xs
                            .get(k)
                            .or(xs.first())
                            .ok_or_else(|| anyhow!("empty array value"))?;
                        self.bind_var(*var, e, o);
                    }
                }
            }
            Comp::Inst(i) => self.bind_fields(*i, v, o)?,
            Comp::Arr(_, el) => {
                let parts: Vec<Val> = match v {
                    Val::IA(_, ids) => ids.into_iter().map(Val::Inst).collect(),
                    Val::Tuple(t) => t,
                    other => vec![other],
                };
                for (k, x) in el.iter().enumerate() {
                    if let Some(p) = parts.get(k).or(parts.first()) {
                        self.bind_comp(x, p.clone(), o)?;
                    }
                }
            }
            Comp::Removed | Comp::Str(_) => {}
        }
        Ok(())
    }

    fn bind_var(&mut self, var: VarId, e: ExprId, o: u32) {
        let kind = self.model.vars[var as usize].kind;
        if matches!(kind, VarKind::Parameter | VarKind::Constant) {
            self.model.vars[var as usize].binding = Some(e);
            self.binding_done.insert(var);
        } else {
            let lhs = self.model.arena.var(var);
            self.model.equations.push(Equation {
                lhs,
                rhs: e,
                origin: o,
            });
        }
    }

    fn translate_pending(&mut self) -> Result<()> {
        self.translate_record_bindings()?;
        for i in 0..self.pending.len() {
            let v = self.pending[i].var;
            if self.binding_done.contains(&v) {
                continue;
            }
            self.binding_done.insert(v);
            let name = self.model.vars[v as usize].name.clone();
            self.translate_attrs(i)
                .with_context(|| format!("in declaration of '{name}'"))?;
        }
        Ok(())
    }

    /// Translate a modifier value and apply its array-of-components element selection.
    fn translate_scoped(&mut self, s: &Scoped<'a>, ctx: &mut Ctx) -> Result<Val> {
        let mut v = self.translate(s.expr, &s.scope.clone(), ctx)?;
        for &(k, n) in &s.sel {
            v = slice_leading(v, k, n);
        }
        Ok(v)
    }

    fn element_of(&self, val: Val, index: usize, count: usize, each: bool) -> Result<ExprId> {
        match val {
            Val::S(e) => Ok(e),
            Val::A(_, v) => {
                if each || count == 1 {
                    if v.len() == 1 {
                        Ok(v[0])
                    } else {
                        bail!("array value for scalar attribute")
                    }
                } else if v.len() == count {
                    Ok(v[index])
                } else if v.len() == 1 {
                    Ok(v[0])
                } else {
                    bail!(
                        "array value of size {} does not match {} elements",
                        v.len(),
                        count
                    )
                }
            }
            Val::Tuple(v) if !v.is_empty() => self.element_of(v[0].clone(), index, count, each),
            other => bail!("unexpected {} value in modifier", other.kind()),
        }
    }

    fn translate_attrs(&mut self, pi: usize) -> Result<()> {
        let (var, m, index, count, is_param) = {
            let p = &self.pending[pi];
            (p.var, p.m.clone(), p.index, p.count, p.is_param)
        };
        let mut ctx = Ctx::default();
        // binding
        if let Some(s) = &m.value {
            let val = self.translate_scoped(s, &mut ctx)?;
            let e = self.element_of(val, index, count, m.each)?;
            if is_param {
                self.model.vars[var as usize].binding = Some(e);
            } else {
                let lhs = self.model.arena.var(var);
                let o = self.model.origin(format!(
                    "declaration of {}",
                    self.model.vars[var as usize].name
                ));
                self.model.equations.push(Equation {
                    lhs,
                    rhs: e,
                    origin: o,
                });
            }
        }
        for (attr, sm) in &m.subs {
            let Some(s) = &sm.value else { continue };
            match attr.as_str() {
                "start" | "nominal" | "min" | "max" => {
                    let val = self.translate_scoped(s, &mut ctx)?;
                    let e = self.element_of(val, index, count, sm.each)?;
                    let v = &mut self.model.vars[var as usize];
                    match attr.as_str() {
                        "start" => v.start = Some(e),
                        "nominal" => v.nominal = Some(e),
                        "min" => v.min = Some(e),
                        _ => v.max = Some(e),
                    }
                }
                "fixed" => {
                    let val = self.translate_scoped(s, &mut ctx)?;
                    let e = self.element_of(val, index, count, sm.each)?;
                    let f = self.eval_param_expr(e).unwrap_or(1.0);
                    self.model.vars[var as usize].fixed = Some(f != 0.0);
                }
                "unit" | "displayUnit" | "quantity" => {
                    if let E::Terminal {
                        token,
                        terminal_type: TerminalType::String,
                        ..
                    } = s.expr
                    {
                        let t = token.text.trim_matches('"').to_string();
                        let v = &mut self.model.vars[var as usize];
                        match attr.as_str() {
                            "unit" => v.unit = t,
                            "displayUnit" => v.display_unit = t,
                            _ => {}
                        }
                    }
                }
                "stateSelect" => {
                    let val = self.translate_scoped(s, &mut ctx)?;
                    let e = val.scalar()?;
                    if let Some(x) = self.eval_param_expr(e) {
                        self.model.vars[var as usize].state_select = match x as i64 {
                            1 => StateSelect::Never,
                            2 => StateSelect::Avoid,
                            4 => StateSelect::Prefer,
                            5 => StateSelect::Always,
                            _ => StateSelect::Default,
                        };
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Count the connect equations of every instance before equations are translated,
    /// so `cardinality(c)` is known wherever it is used.
    fn count_connects(&mut self) -> Result<()> {
        self.counting_connects = true;
        let mut i = 0;
        let r = loop {
            if i >= self.insts.len() {
                break Ok(());
            }
            let eqs = self.insts[i].equations.clone();
            for (eq, env) in &eqs {
                let scope = Scope {
                    inst: Some(i as InstId),
                    env: env.clone(),
                };
                // Connections this pass cannot resolve are left to the real translation.
                let _ = self.count_connect_eq(eq, &scope, &mut Ctx::default());
            }
            i += 1;
        };
        self.counting_connects = false;
        r
    }

    fn count_connect_eq(
        &mut self,
        eq: &'a ast::Equation,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<()> {
        match eq {
            ast::Equation::Connect { lhs, rhs } => self.translate_connect(lhs, rhs, scope, ctx),
            ast::Equation::For { indices, equations } => {
                if !equations.iter().any(has_connect) {
                    return Ok(());
                }
                self.for_loop(indices, scope, ctx, &mut |this, ctx| {
                    for e in equations {
                        let _ = this.count_connect_eq(e, scope, ctx);
                    }
                    Ok(())
                })
            }
            _ => Ok(()),
        }
    }

    /// The homotopy parameter shared by every `homotopy()` call.
    fn homotopy_lambda(&mut self) -> ExprId {
        let v = match self.homotopy_var {
            Some(v) => v,
            None => {
                let mut v = Variable::new(
                    crate::flat::HOMOTOPY_LAMBDA.to_string(),
                    VarKind::Parameter,
                    VarType::Real,
                );
                v.generated = true;
                v.is_protected = true;
                v.binding = Some(Arena::ONE);
                v.start = Some(Arena::ONE);
                let id = self.model.add_var(v);
                self.binding_done.insert(id);
                self.homotopy_var = Some(id);
                id
            }
        };
        self.model.arena.var(v)
    }

    /// The variable standing for `inStream(s)`; its equation is added with the connections.
    fn in_stream_var(&mut self, s: VarId) -> VarId {
        if let Some(&v) = self.in_stream.get(&s) {
            return v;
        }
        let name = format!("$inStream.{}", self.model.vars[s as usize].name);
        let mut v = Variable::new(name, VarKind::Continuous, VarType::Real);
        v.generated = true;
        v.is_protected = true;
        v.start = self.model.vars[s as usize].start;
        v.nominal = self.model.vars[s as usize].nominal;
        let id = self.model.add_var(v);
        self.in_stream.insert(s, id);
        id
    }

    /// The flow variable of the connector holding stream variable `s`.
    fn stream_flow(&self, s: VarId) -> Option<VarId> {
        let owner = *self.leaf_owner.get(&s)?;
        self.insts[owner as usize]
            .comps
            .values()
            .find_map(|c| match c {
                Comp::Var(v) if self.model.vars[*v as usize].is_flow => Some(*v),
                _ => None,
            })
    }

    /// The variable of connector `owner` with the same field path as stream variable `s`.
    fn stream_peer(&self, s: VarId, owner: InstId) -> Option<VarId> {
        let so = *self.leaf_owner.get(&s)?;
        let sname = &self.model.vars[s as usize].name;
        let suffix = sname.strip_prefix(&self.insts[so as usize].prefix)?;
        let name = format!("{}{}", self.insts[owner as usize].prefix, suffix);
        self.var_by_name.get(&name).copied()
    }

    /// Equations of the stream connectors (MLS 15.2): `inStream` values of inside
    /// connectors and the stream variables of outside connectors, both mixed from
    /// the other connectors of the connection set weighted by their outflow.
    fn finish_streams(&mut self) {
        let origin = self.model.origin("stream connection");
        // Connection sets of flow variables.
        let points: Vec<ConnPoint> = self.conn_parent.keys().copied().collect();
        let mut sets: FxHashMap<ConnPoint, Vec<ConnPoint>> = FxHashMap::default();
        for p in points {
            let r = self.find(p);
            sets.entry(r).or_default().push(p);
        }
        for set in sets.values_mut() {
            set.sort_by_key(|p| (p.var, p.inside));
        }
        let set_of = |this: &mut Self, p: ConnPoint| -> Vec<ConnPoint> {
            if this.conn_parent.contains_key(&p) {
                let r = this.find(p);
                sets.get(&r).cloned().unwrap_or_else(|| vec![p])
            } else {
                vec![p]
            }
        };
        let eps = 1e-10;
        // `stream var of connector at point q`, `outflow weight of q into the set`, value.
        let mut done: FxHashSet<VarId> = FxHashSet::default();
        let mut outside_done: FxHashSet<VarId> = FxHashSet::default();
        let mut k = 0;
        while k < self.in_stream.len() {
            let (s, ph) = self.in_stream.get_index(k).map(|(a, b)| (*a, *b)).unwrap();
            k += 1;
            if !done.insert(s) {
                continue;
            }
            let Some(f) = self.stream_flow(s) else {
                let a = self.model.arena.var(ph);
                let b = self.model.arena.var(s);
                self.model.equations.push(Equation {
                    lhs: a,
                    rhs: b,
                    origin,
                });
                continue;
            };
            let me = ConnPoint {
                var: f,
                inside: true,
            };
            let set = set_of(self, me);
            let rhs = self.stream_mix(s, &set, Some(me), eps);
            let a = self.model.arena.var(ph);
            self.model.equations.push(Equation {
                lhs: a,
                rhs,
                origin,
            });
            // Outside connectors of this set get their stream variable defined here.
            for q in set.clone() {
                if q.inside || !outside_done.insert(q.var) {
                    continue;
                }
                let Some(&qo) = self.leaf_owner.get(&q.var) else {
                    continue;
                };
                let Some(sq) = self.stream_peer(s, qo) else {
                    continue;
                };
                let rhs = self.stream_mix(s, &set, Some(q), eps);
                let a = self.model.arena.var(sq);
                self.model.equations.push(Equation {
                    lhs: a,
                    rhs,
                    origin,
                });
            }
        }
    }

    /// Mixing rule of stream variable `s` (field path) over connection set `set`,
    /// leaving out point `skip`.
    fn stream_mix(
        &mut self,
        s: VarId,
        set: &[ConnPoint],
        skip: Option<ConnPoint>,
        eps: f64,
    ) -> ExprId {
        let mut others: Vec<ConnPoint> = set.iter().copied().filter(|p| Some(*p) != skip).collect();
        // A connector whose flow can only enter its component (`m_flow(min = 0)`, as on
        // sensors) never supplies outflow to the others (MLS 15.2).
        let supplies = |this: &mut Self, q: &ConnPoint| -> bool {
            let v = &this.model.vars[q.var as usize];
            let bound = if q.inside { v.min } else { v.max };
            match bound.and_then(|b| this.eval_param_expr(b)) {
                Some(b) if q.inside => b < 0.0,
                Some(b) => b > 0.0,
                None => true,
            }
        };
        let keep: Vec<ConnPoint> = others
            .iter()
            .copied()
            .filter(|q| supplies(self, q))
            .collect();
        if !keep.is_empty() {
            others = keep;
        }
        // (value, flow, sign): an inside connector supplies its own stream variable when
        // its flow is negative; an outside one supplies the parent's inStream when its flow
        // is positive.
        let mut terms: Vec<(ExprId, ExprId)> = Vec::new();
        for q in &others {
            let Some(&qo) = self.leaf_owner.get(&q.var) else {
                continue;
            };
            let Some(sq) = self.stream_peer(s, qo) else {
                continue;
            };
            let val = if q.inside {
                self.model.arena.var(sq)
            } else {
                let v = self.in_stream_var(sq);
                self.model.arena.var(v)
            };
            let fl = self.model.arena.var(q.var);
            let out = if q.inside {
                self.model.arena.neg(fl)
            } else {
                fl
            };
            terms.push((val, out));
        }
        let a = &mut self.model.arena;
        match terms.len() {
            0 => match skip {
                // Unconnected: inStream(s) = s.
                Some(p) if p.inside => a.var(s),
                _ => a.var(s),
            },
            1 => terms[0].0,
            _ => {
                let e = a.constant(eps);
                let mut num = Arena::ZERO;
                let mut den = Arena::ZERO;
                for (val, out) in terms {
                    let w = a.f2(Fn2::Max, out, e);
                    let t = a.mul(w, val);
                    num = a.add(num, t);
                    den = a.add(den, w);
                }
                a.div(num, den)
            }
        }
    }

    /// `cardinality(c)`: connect equations naming connector `c`.
    fn cardinality(&mut self, arg: &'a E, scope: &Scope, ctx: &mut Ctx) -> Result<usize> {
        let (E::ComponentReference(cr), Some(inst)) = (arg, scope.inst) else {
            bail!("cardinality() expects a connector");
        };
        let (c, _) = self.resolve_connector(cr, inst, scope, ctx)?;
        let Some(c) = c else { return Ok(0) };
        let mut leaves = Vec::new();
        self.connector_leaves(&c, &mut leaves);
        Ok(leaves
            .first()
            .and_then(|v| self.conn_count.get(v))
            .copied()
            .unwrap_or(0))
    }

    fn translate_instance(&mut self, id: InstId) -> Result<()> {
        let eqs = std::mem::take(&mut self.insts[id as usize].equations);
        for (eq, env) in &eqs {
            let scope = Scope {
                inst: Some(id),
                env: env.clone(),
            };
            let mut ctx = Ctx::default();
            let mut out = Vec::new();
            self.translate_equation(eq, &scope, &mut ctx, &mut out, false)?;
            self.model.equations.extend(out);
        }
        let ieqs = std::mem::take(&mut self.insts[id as usize].initial_equations);
        for (eq, env) in &ieqs {
            let scope = Scope {
                inst: Some(id),
                env: env.clone(),
            };
            let mut ctx = Ctx::default();
            let mut out = Vec::new();
            self.translate_equation(eq, &scope, &mut ctx, &mut out, true)?;
            self.model.initial_equations.extend(out);
        }
        let algs = std::mem::take(&mut self.insts[id as usize].algorithms);
        for (alg, env) in &algs {
            let scope = Scope {
                inst: Some(id),
                env: env.clone(),
            };
            self.translate_algorithm(alg, &scope, false)?;
        }
        let ialgs = std::mem::take(&mut self.insts[id as usize].initial_algorithms);
        for (alg, env) in &ialgs {
            let scope = Scope {
                inst: Some(id),
                env: env.clone(),
            };
            self.translate_algorithm(alg, &scope, true)?;
        }
        Ok(())
    }

    fn eq_origin(&mut self, scope: &Scope, what: &str) -> u32 {
        let name = scope.inst.map(|i| self.inst_name(i)).unwrap_or_default();
        self.model.origin(format!("{what} in {name}"))
    }

    fn push_residuals(
        &mut self,
        l: Val,
        r: Val,
        out: &mut Vec<Equation>,
        origin: u32,
    ) -> Result<()> {
        match (&l, &r) {
            (Val::Tuple(ls), Val::Tuple(rs)) => {
                for (a, b) in ls.iter().zip(rs.iter()) {
                    if matches!(a, Val::S(e) if *e == Arena::ZERO && false) {
                        continue;
                    }
                    self.push_residuals(a.clone(), b.clone(), out, origin)?;
                }
                return Ok(());
            }
            (Val::Rec(lf), Val::Rec(rf)) => {
                for (n, lv) in lf {
                    if let Some((_, rv)) = rf.iter().find(|(rn, _)| rn == n) {
                        self.push_residuals(lv.clone(), rv.clone(), out, origin)?;
                    }
                }
                return Ok(());
            }
            (Val::Inst(_) | Val::IA(..), _)
            | (_, Val::Inst(_) | Val::IA(..))
            | (Val::Rec(_), _)
            | (_, Val::Rec(_)) => {
                let lr = if Self::is_record_like(&l) {
                    self.to_rec(l)?
                } else {
                    let (a, b) = self.complex_parts(l)?;
                    Self::complex(a, b)
                };
                let rr = if Self::is_record_like(&r) {
                    self.to_rec(r)?
                } else {
                    let (a, b) = self.complex_parts(r)?;
                    Self::complex(a, b)
                };
                return self.push_residuals(lr, rr, out, origin);
            }
            _ => {}
        }
        let (ld, le) = l.elems()?;
        let (rd, re) = r.elems()?;
        if le.len() != re.len() {
            if re.len() == 1 {
                for &x in &le {
                    out.push(Equation {
                        lhs: x,
                        rhs: re[0],
                        origin,
                    });
                }
                return Ok(());
            }
            bail!("equation size mismatch: {:?} vs {:?}", ld, rd);
        }
        for (a, b) in le.into_iter().zip(re) {
            out.push(Equation {
                lhs: a,
                rhs: b,
                origin,
            });
        }
        Ok(())
    }

    fn as_record(&mut self, v: &Val) -> Result<Val> {
        match v {
            Val::Inst(i) => {
                let mut fields = Vec::new();
                let comps: Vec<(String, Comp)> = self.insts[*i as usize]
                    .comps
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (n, c) in comps {
                    if let Some(val) = self.comp_value(&c) {
                        fields.push((n, val));
                    }
                }
                Ok(Val::Rec(fields))
            }
            Val::Rec(_) => Ok(v.clone()),
            other => bail!("expected record value, found {}", other.kind()),
        }
    }

    fn comp_value(&mut self, c: &Comp) -> Option<Val> {
        match c {
            Comp::Var(v) => Some(Val::S(self.var_ref(*v))),
            Comp::Inst(i) => self.as_record(&Val::Inst(*i)).ok(),
            Comp::Arr(..) => self.comp_to_val(c).ok(),
            Comp::Str(x) => Some(Val::Str(x.to_string())),
            Comp::Removed => None,
        }
    }

    fn translate_equation(
        &mut self,
        eq: &'a ast::Equation,
        scope: &Scope,
        ctx: &mut Ctx,
        out: &mut Vec<Equation>,
        initial: bool,
    ) -> Result<()> {
        match eq {
            ast::Equation::Empty => {}
            ast::Equation::Simple { lhs, rhs } => {
                let l = self.translate(lhs, scope, ctx)?;
                let r = self.translate(rhs, scope, ctx)?;
                let l = self.materialize(l)?;
                let r = self.materialize(r)?;
                let o = self.eq_origin(scope, "equation");
                self.push_residuals(l, r, out, o)?;
            }
            ast::Equation::Connect { lhs, rhs } => {
                self.translate_connect(lhs, rhs, scope, ctx)?;
            }
            ast::Equation::For { indices, equations } => {
                self.for_loop(indices, scope, ctx, &mut |this, ctx| {
                    for e in equations {
                        this.translate_equation(e, scope, ctx, out, initial)?;
                    }
                    Ok(())
                })?;
            }
            ast::Equation::If {
                cond_blocks,
                else_block,
            } => {
                // Parameter conditions select a branch at translation time.
                let mut conds = Vec::new();
                for b in cond_blocks {
                    let c = self.translate(&b.cond, scope, ctx)?.scalar()?;
                    conds.push(c);
                }
                let mut chosen: Option<Option<usize>> = None;
                let mut all_param = true;
                for (i, &c) in conds.iter().enumerate() {
                    match self.eval_param_expr(c) {
                        Some(v) if v != 0.0 => {
                            chosen = Some(Some(i));
                            break;
                        }
                        Some(_) => {}
                        None => {
                            all_param = false;
                            break;
                        }
                    }
                }
                if all_param {
                    let branch: Vec<&'a ast::Equation> = match chosen.unwrap_or(None) {
                        Some(i) => cond_blocks[i].eqs.iter().collect(),
                        None => else_block
                            .as_ref()
                            .map(|v| v.iter().collect())
                            .unwrap_or_default(),
                    };
                    for e in branch {
                        self.translate_equation(e, scope, ctx, out, initial)?;
                    }
                } else {
                    // Time-varying condition: equal equation counts in all branches.
                    let mut branches: Vec<Vec<Equation>> = Vec::new();
                    for b in cond_blocks {
                        let mut v = Vec::new();
                        for e in &b.eqs {
                            self.translate_equation(e, scope, ctx, &mut v, initial)?;
                        }
                        branches.push(v);
                    }
                    let mut ev = Vec::new();
                    if let Some(eb) = else_block {
                        for e in eb {
                            self.translate_equation(e, scope, ctx, &mut ev, initial)?;
                        }
                    }
                    let n = ev.len();
                    if branches.iter().any(|b| b.len() != n) {
                        bail!(
                            "if-equation branches with non-parameter conditions must have the same number of equations"
                        );
                    }
                    let o = self.eq_origin(scope, "if-equation");
                    for k in 0..n {
                        let mut res = {
                            let e = ev[k];
                            self.model.arena.sub(e.lhs, e.rhs)
                        };
                        for (bi, b) in branches.iter().enumerate().rev() {
                            let r = self.model.arena.sub(b[k].lhs, b[k].rhs);
                            res = self.model.arena.if_(conds[bi], r, res);
                        }
                        out.push(Equation {
                            lhs: res,
                            rhs: Arena::ZERO,
                            origin: o,
                        });
                    }
                }
            }
            ast::Equation::When(blocks) => {
                let maybe_clock = match &blocks[0].cond {
                    E::ComponentReference(_) => true,
                    E::FunctionCall { comp, .. } => matches!(
                        cref_name(comp).as_str(),
                        "Clock" | "subSample" | "superSample" | "shiftSample" | "backSample"
                    ),
                    _ => false,
                };
                if blocks.len() == 1 && maybe_clock {
                    let c = self.translate(&blocks[0].cond, scope, ctx)?;
                    if let Val::S(c) = c
                        && self.is_clock_expr(c)
                    {
                        let mut es = Vec::new();
                        for e in &blocks[0].eqs {
                            self.translate_equation(e, scope, ctx, &mut es, initial)?;
                        }
                        self.clocked_whens.push((c, es));
                        self.uses_clocks = true;
                        return Ok(());
                    }
                }
                let mut branches = Vec::new();
                for b in blocks {
                    let c = self.when_condition(&b.cond, scope, ctx)?;
                    let mut acts = Vec::new();
                    for e in &b.eqs {
                        self.when_actions(e, scope, ctx, &mut acts)?;
                    }
                    branches.push((c, acts));
                }
                let o = self.eq_origin(scope, "when-equation");
                self.model.whens.push(WhenClause {
                    branches,
                    origin: o,
                });
            }
            ast::Equation::FunctionCall { comp, args, .. } => {
                let name = cref_name(comp);
                match name.as_str() {
                    "assert" => {
                        if let Some(c) = args.first() {
                            let c = self
                                .translate(
                                    c,
                                    scope,
                                    &mut Ctx {
                                        no_event: true,
                                        ..ctx.clone()
                                    },
                                )?
                                .scalar()?;
                            let msg = args.get(1).map(|m| expr_string(m)).unwrap_or_default();
                            self.model.asserts.push((c, msg));
                        }
                    }
                    "terminate" => {
                        self.model
                            .warnings
                            .push("terminate() outside when-clause ignored".into());
                    }
                    "Connections.branch"
                    | "Connections.root"
                    | "Connections.potentialRoot"
                    | "Connections.uniqueRoot" => {
                        self.oc_log.used = true;
                        let (pos, named) = split_args(args);
                        let mut nodes = Vec::new();
                        for a in pos
                            .iter()
                            .take(if name == "Connections.branch" { 2 } else { 1 })
                        {
                            nodes.push(self.oc_node(a, scope, ctx)?);
                        }
                        match name.as_str() {
                            "Connections.branch" => {
                                if let [Some(a), Some(b)] = &nodes[..] {
                                    self.oc_log.branches.push((a.clone(), b.clone()));
                                }
                            }
                            "Connections.potentialRoot" => {
                                let pe = pos.get(1).copied().or_else(|| {
                                    named
                                        .iter()
                                        .find(|(n, _)| *n == "priority")
                                        .map(|(_, e)| *e)
                                });
                                let p = match pe {
                                    Some(e) => self.eval_structural(e, scope).unwrap_or(0.0) as i64,
                                    None => 0,
                                };
                                if let Some(Some(a)) = nodes.first() {
                                    self.oc_log.potential.push((a.clone(), p));
                                }
                            }
                            _ => {
                                if let Some(Some(a)) = nodes.first() {
                                    self.oc_log.roots.push(a.clone());
                                }
                            }
                        }
                    }
                    _ => {
                        // Function call with no outputs in an equation section: ignore.
                    }
                }
            }
            ast::Equation::Assert {
                condition, message, ..
            } => {
                let c = self
                    .translate(
                        condition,
                        scope,
                        &mut Ctx {
                            no_event: true,
                            ..ctx.clone()
                        },
                    )?
                    .scalar()?;
                self.model.asserts.push((c, expr_string(message)));
            }
        }
        Ok(())
    }

    fn materialize(&mut self, v: Val) -> Result<Val> {
        Ok(v)
    }

    fn when_condition(&mut self, cond: &'a E, scope: &Scope, ctx: &mut Ctx) -> Result<ExprId> {
        let v = self.translate(cond, scope, ctx)?;
        match v {
            Val::S(e) => Ok(e),
            Val::A(_, v) => {
                // when {c1, c2}: triggers when any element becomes true, also while another
                // one stays true (`when {u, not u}` fires on every change of u). Each
                // element gets a Boolean whose rising edge is tested on its own.
                let mut r = Arena::ZERO;
                let o = self.model.origin("when condition");
                for e in v {
                    let term = if e == self.model.arena.initial() || v_len_one(&self.model.arena, e)
                    {
                        e
                    } else {
                        let k = self.model.vars.len();
                        let mut b =
                            Variable::new(format!("$when{k}"), VarKind::Discrete, VarType::Boolean);
                        b.generated = true;
                        b.is_protected = true;
                        b.start = Some(Arena::ZERO);
                        let b = self.model.add_var(b);
                        let bv = self.model.arena.var(b);
                        self.model.equations.push(Equation {
                            lhs: bv,
                            rhs: e,
                            origin: o,
                        });
                        let a = &mut self.model.arena;
                        let pb = a.pre(b);
                        let npb = a.not(pb);
                        a.f2(Fn2::And, bv, npb)
                    };
                    r = self.model.arena.f2(Fn2::Or, r, term);
                }
                Ok(r)
            }
            other => bail!("invalid when condition ({})", other.kind()),
        }
    }

    fn when_actions(
        &mut self,
        e: &'a ast::Equation,
        scope: &Scope,
        ctx: &mut Ctx,
        acts: &mut Vec<WhenAction>,
    ) -> Result<()> {
        match e {
            ast::Equation::Simple { lhs, rhs } => {
                let l = self.translate(lhs, scope, ctx)?;
                let r = self.translate(rhs, scope, ctx)?;
                let targets: Vec<ExprId> = match &l {
                    Val::Tuple(t) => {
                        let mut v = Vec::new();
                        for x in t {
                            v.extend(x.elems()?.1);
                        }
                        v
                    }
                    _ => l.elems()?.1,
                };
                let values: Vec<ExprId> = match &r {
                    Val::Tuple(t) => {
                        let mut v = Vec::new();
                        for x in t {
                            v.extend(x.elems()?.1);
                        }
                        v
                    }
                    _ => r.elems()?.1,
                };
                for (i, t) in targets.iter().enumerate() {
                    let Node::Var(v) = self.model.arena.node(*t) else {
                        bail!("left-hand side of a when-equation must be a variable");
                    };
                    let val = values
                        .get(i)
                        .or(values.first())
                        .copied()
                        .unwrap_or(Arena::ZERO);
                    self.when_assigned.insert(v);
                    acts.push(WhenAction::Assign(v, val));
                }
            }
            ast::Equation::FunctionCall { comp, args, .. } => {
                let name = cref_name(comp);
                match name.as_str() {
                    "reinit" => {
                        if args.len() != 2 {
                            bail!("reinit expects two arguments");
                        }
                        let t = self.translate(&args[0], scope, ctx)?;
                        let v = self.translate(&args[1], scope, ctx)?;
                        let (_, te) = t.elems()?;
                        let (_, ve) = v.elems()?;
                        for (i, x) in te.iter().enumerate() {
                            let Node::Var(var) = self.model.arena.node(*x) else {
                                bail!("reinit target must be a variable");
                            };
                            acts.push(WhenAction::Reinit(var, *ve.get(i).unwrap_or(&ve[0])));
                        }
                    }
                    "terminate" => {
                        acts.push(WhenAction::Terminate(
                            args.first().map(expr_string).unwrap_or_default(),
                        ));
                    }
                    "assert" => {
                        let c = self.translate(&args[0], scope, ctx)?.scalar()?;
                        acts.push(WhenAction::Assert(
                            c,
                            args.get(1).map(expr_string).unwrap_or_default(),
                        ));
                    }
                    _ => {}
                }
            }
            ast::Equation::If {
                cond_blocks,
                else_block,
            } => {
                // Conditional assignments inside when: v = if c then a else b (else keeps pre(v)).
                let mut per_branch: Vec<(ExprId, Vec<WhenAction>)> = Vec::new();
                for b in cond_blocks {
                    let c = self.translate(&b.cond, scope, ctx)?.scalar()?;
                    let mut a = Vec::new();
                    for e in &b.eqs {
                        self.when_actions(e, scope, ctx, &mut a)?;
                    }
                    per_branch.push((c, a));
                }
                let mut else_acts = Vec::new();
                if let Some(eb) = else_block {
                    for e in eb {
                        self.when_actions(e, scope, ctx, &mut else_acts)?;
                    }
                }
                // Merge assignments by variable.
                let mut vars: Vec<VarId> = Vec::new();
                for (_, a) in per_branch
                    .iter()
                    .chain(std::iter::once(&(Arena::ZERO, else_acts.clone())))
                {
                    for x in a {
                        if let WhenAction::Assign(v, _) | WhenAction::Reinit(v, _) = x
                            && !vars.contains(v)
                        {
                            vars.push(*v);
                        }
                    }
                }
                for v in vars {
                    let find = |a: &Vec<WhenAction>, reinit: bool| {
                        a.iter().find_map(|x| match x {
                            WhenAction::Assign(w, e) if *w == v && !reinit => Some(*e),
                            WhenAction::Reinit(w, e) if *w == v && reinit => Some(*e),
                            _ => None,
                        })
                    };
                    for reinit in [false, true] {
                        let any = per_branch.iter().any(|(_, a)| find(a, reinit).is_some())
                            || find(&else_acts, reinit).is_some();
                        if !any {
                            continue;
                        }
                        let keep = if reinit {
                            self.model.arena.var(v)
                        } else {
                            self.model.arena.pre(v)
                        };
                        let mut r = find(&else_acts, reinit).unwrap_or(keep);
                        for (c, a) in per_branch.iter().rev() {
                            let x = find(a, reinit).unwrap_or(keep);
                            r = self.model.arena.if_(*c, x, r);
                        }
                        acts.push(if reinit {
                            WhenAction::Reinit(v, r)
                        } else {
                            WhenAction::Assign(v, r)
                        });
                        if !reinit {
                            self.when_assigned.insert(v);
                        }
                    }
                }
                for (_, a) in &per_branch {
                    for x in a {
                        if let WhenAction::Terminate(_) | WhenAction::Assert(..) = x {
                            acts.push(x.clone());
                        }
                    }
                }
            }
            ast::Equation::For { indices, equations } => {
                let mut tmp = Vec::new();
                self.for_loop(indices, scope, ctx, &mut |this, ctx| {
                    for e in equations {
                        this.when_actions(e, scope, ctx, &mut tmp)?;
                    }
                    Ok(())
                })?;
                acts.extend(tmp);
            }
            ast::Equation::Assert {
                condition, message, ..
            } => {
                let c = self.translate(condition, scope, ctx)?.scalar()?;
                acts.push(WhenAction::Assert(c, expr_string(message)));
            }
            _ => bail!("unsupported equation inside when-clause"),
        }
        Ok(())
    }

    fn for_loop(
        &mut self,
        indices: &'a [ast::ForIndex],
        scope: &Scope,
        ctx: &mut Ctx,
        body: &mut dyn FnMut(&mut Self, &mut Ctx) -> Result<()>,
    ) -> Result<()> {
        if indices.is_empty() {
            return body(self, ctx);
        }
        let idx = &indices[0];
        let range = self.translate(&idx.range, scope, ctx)?;
        let (_, vals) = range.elems()?;
        let name = idx.ident.text.to_string();
        for v in vals {
            ctx.locals.push(FxHashMap::default());
            ctx.locals
                .last_mut()
                .unwrap()
                .insert(name.clone(), Val::S(v));
            let r = self.for_loop(&indices[1..], scope, ctx, body);
            ctx.locals.pop();
            r?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Connections
    // ------------------------------------------------------------------

    /// The `equalityConstraint` function of an overconstrained record class, if any.
    fn oc_function(&mut self, class: ClassId) -> Option<ClassId> {
        if let Some(&r) = self.oc_class.get(&class) {
            return r;
        }
        let r = self
            .tree
            .member_class(class, "equalityConstraint")
            .filter(|&f| matches!(self.tree.class_type(f), ClassType::Function));
        self.oc_class.insert(class, r);
        r
    }

    /// Overconstrained record instances found at the same place in two connectors.
    fn oc_pairs(&mut self, a: &Comp, b: &Comp, out: &mut Vec<(InstId, InstId, ClassId)>) {
        match (a, b) {
            (Comp::Inst(i), Comp::Inst(j)) => {
                if let Some(f) = self.oc_function(self.insts[*i as usize].class) {
                    out.push((*i, *j, f));
                    return;
                }
                let ca: Vec<(String, Comp)> = self.insts[*i as usize]
                    .comps
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                for (k, x) in ca {
                    if let Some(y) = self.insts[*j as usize].comps.get(&k).cloned() {
                        self.oc_pairs(&x, &y, out);
                    }
                }
            }
            (Comp::Arr(_, xa), Comp::Arr(_, xb)) => {
                for (x, y) in xa.iter().zip(xb.iter()) {
                    self.oc_pairs(x, y, out);
                }
            }
            _ => {}
        }
    }

    /// Node name of a `Connections.*` argument (an overconstrained record instance).
    fn oc_node(&mut self, e: &'a E, scope: &Scope, ctx: &mut Ctx) -> Result<Option<String>> {
        let E::ComponentReference(cr) = e else {
            return Ok(None);
        };
        let Some(inst) = scope.inst else {
            return Ok(None);
        };
        let (c, _) = self.resolve_connector(cr, inst, scope, ctx)?;
        Ok(match c {
            Some(Comp::Inst(i)) => Some(self.insts[i as usize].prefix.clone()),
            _ => None,
        })
    }

    /// `0 = T.equalityConstraint(a, b)` for a connection that closes a loop.
    fn equality_constraint(&mut self, i: InstId, j: InstId, f: ClassId) -> Result<()> {
        let ra = self.as_record(&Val::Inst(i))?;
        let rb = self.as_record(&Val::Inst(j))?;
        let outs = self.inline_function(f, vec![ra, rb], vec![], Rc::new(FxHashMap::default()))?;
        let res = outs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("equalityConstraint has no output"))?;
        let (_, el) = res.elems()?;
        let o = self.model.origin(format!(
            "equalityConstraint({}, {})",
            self.insts[i as usize].prefix, self.insts[j as usize].prefix
        ));
        for e in el {
            self.model.equations.push(Equation {
                lhs: Arena::ZERO,
                rhs: e,
                origin: o,
            });
        }
        Ok(())
    }

    fn translate_connect(
        &mut self,
        a: &'a ast::ComponentReference,
        b: &'a ast::ComponentReference,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<()> {
        let inst = scope
            .inst
            .ok_or_else(|| anyhow!("connect outside of a model"))?;
        let (ca, ia) = self.resolve_connector(a, inst, scope, ctx)?;
        let (cb, ib) = self.resolve_connector(b, inst, scope, ctx)?;
        let (Some(ca), Some(cb)) = (ca, cb) else {
            return Ok(()); // conditional connector removed
        };
        let mut pa = Vec::new();
        let mut pb: Vec<VarId> = Vec::new();
        self.connector_leaves(&ca, &mut pa);
        self.connector_leaves(&cb, &mut pb);
        if self.counting_connects {
            for c in [&ca, &cb] {
                for el in comp_elements(c) {
                    let mut l = Vec::new();
                    self.connector_leaves(&el, &mut l);
                    if let Some(&v) = l.first() {
                        *self.conn_count.entry(v).or_default() += 1;
                    }
                }
            }
            return Ok(());
        }
        if pa.len() != pb.len() {
            bail!(
                "connect({}, {}): incompatible connectors ({} vs {} variables)",
                a,
                b,
                pa.len(),
                pb.len()
            );
        }
        // Overconstrained records (frame orientations, phasor references) in the connectors.
        let mut ocp = Vec::new();
        self.oc_pairs(&ca, &cb, &mut ocp);
        if !ocp.is_empty() {
            self.oc_log.used = true;
            let mut skip: FxHashSet<VarId> = FxHashSet::default();
            for &(i, j, fcls) in &ocp {
                let (na, nb) = (
                    self.insts[i as usize].prefix.clone(),
                    self.insts[j as usize].prefix.clone(),
                );
                self.oc_log.connects.push((na.clone(), nb.clone()));
                if let Some(ans) = self.oc_answers.clone()
                    && ans.is_broken(&na, &nb)
                {
                    let mut la = Vec::new();
                    self.connector_leaves(&Comp::Inst(i), &mut la);
                    skip.extend(la);
                    self.equality_constraint(i, j, fcls)
                        .with_context(|| format!("equalityConstraint for connect({a}, {b})"))?;
                }
            }
            if !skip.is_empty() {
                let (fa, fb): (Vec<VarId>, Vec<VarId>) = pa
                    .into_iter()
                    .zip(pb)
                    .filter(|(va, _)| !skip.contains(va))
                    .unzip();
                pa = fa;
                pb = fb;
            }
        }
        for (va, vb) in pa.into_iter().zip(pb) {
            // Stream variables are not equated; inStream() mixes them (MLS 15).
            if self.stream_vars.contains(&va) || self.stream_vars.contains(&vb) {
                continue;
            }
            // Parameters and constants in connectors (the phase number m of a plug) are not
            // connection variables.
            if matches!(
                self.model.vars[va as usize].kind,
                VarKind::Parameter | VarKind::Constant
            ) || matches!(
                self.model.vars[vb as usize].kind,
                VarKind::Parameter | VarKind::Constant
            ) {
                continue;
            }
            let flow = self.model.vars[va as usize].is_flow;
            self.conn_flow.insert(va, flow);
            self.conn_flow.insert(vb, flow);
            self.union(
                ConnPoint {
                    var: va,
                    inside: ia,
                },
                ConnPoint {
                    var: vb,
                    inside: ib,
                },
            );
        }
        Ok(())
    }

    /// Resolve a connect argument; returns the component and whether it is an inside connector.
    fn resolve_connector(
        &mut self,
        r: &'a ast::ComponentReference,
        inst: InstId,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<(Option<Comp>, bool)> {
        let first = r.parts[0].ident.text.to_string();
        let Some(c) = self.insts[inst as usize].comps.get(&first).cloned() else {
            bail!("connector '{}' not found", r);
        };
        let outside = self.insts[inst as usize].connector_comps.contains(&first);
        let mut cur = c;
        for (i, p) in r.parts.iter().enumerate() {
            if i > 0 {
                cur = match cur {
                    Comp::Inst(id) => match self.insts[id as usize].comps.get(&*p.ident.text) {
                        Some(c) => c.clone(),
                        None => bail!("'{}' has no element '{}'", r, p.ident.text),
                    },
                    Comp::Arr(dims, el) => {
                        // Element access on each array member: c[i].p for all i.
                        let mut v = Vec::new();
                        for e in el.iter() {
                            if let Comp::Inst(id) = e
                                && let Some(c) = self.insts[*id as usize].comps.get(&*p.ident.text)
                            {
                                v.push(c.clone());
                            }
                        }
                        Comp::Arr(dims, std::rc::Rc::new(v))
                    }
                    Comp::Removed => return Ok((None, !outside)),
                    Comp::Str(_) => bail!("cannot access '{}' in a string", p.ident.text),
                    Comp::Var(_) => bail!("cannot access '{}' in a scalar", p.ident.text),
                };
            }
            if let Some(subs) = &p.subs {
                // An element of a conditional array that is not present (`derv[1].y`
                // with `Der derv[3] if false`): the connection is dropped.
                if matches!(cur, Comp::Removed) {
                    return Ok((None, !outside));
                }
                cur = self.subscript_comp(cur, subs, scope, ctx)?;
            }
        }
        if matches!(cur, Comp::Removed) {
            return Ok((None, !outside));
        }
        Ok((Some(cur), !outside))
    }

    fn subscript_comp(
        &mut self,
        c: Comp,
        subs: &'a [ast::Subscript],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Comp> {
        let Comp::Arr(dims, el) = c else {
            bail!("subscript on a non-array component");
        };
        let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
        let (nd, idx) = select_indices(&dims, &sel);
        let v: Vec<Comp> = idx.into_iter().map(|i| el[i].clone()).collect();
        if nd.is_empty() {
            Ok(v.into_iter().next().unwrap())
        } else {
            Ok(Comp::Arr(nd, std::rc::Rc::new(v)))
        }
    }

    fn connector_leaves(&self, c: &Comp, out: &mut Vec<VarId>) {
        match c {
            Comp::Var(v) => out.push(*v),
            Comp::Inst(i) => {
                for c in self.insts[*i as usize].comps.values() {
                    self.connector_leaves(c, out);
                }
            }
            Comp::Arr(_, el) => {
                for e in el.iter() {
                    self.connector_leaves(e, out);
                }
            }
            Comp::Removed | Comp::Str(_) => {}
        }
    }

    fn find(&mut self, p: ConnPoint) -> ConnPoint {
        let mut r = p;
        while let Some(&q) = self.conn_parent.get(&r) {
            if q == r {
                break;
            }
            r = q;
        }
        // path compression
        let mut c = p;
        while let Some(&q) = self.conn_parent.get(&c) {
            if q == c {
                break;
            }
            self.conn_parent.insert(c, r);
            c = q;
        }
        r
    }

    fn union(&mut self, a: ConnPoint, b: ConnPoint) {
        self.conn_parent.entry(a).or_insert(a);
        self.conn_parent.entry(b).or_insert(b);
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.conn_parent.insert(ra, rb);
        }
    }

    fn finish_connections(&mut self) {
        let points: Vec<ConnPoint> = self.conn_parent.keys().copied().collect();
        let mut sets: IndexMap<ConnPoint, Vec<ConnPoint>> = IndexMap::new();
        let mut sorted = points.clone();
        sorted.sort_by_key(|p| (p.var, p.inside));
        for p in sorted {
            let r = self.find(p);
            sets.entry(r).or_default().push(p);
        }
        let origin = self.model.origin("connection");
        let mut flow_sets: Vec<(Vec<VarId>, usize)> = Vec::new();
        let mut pot_set: FxHashMap<VarId, usize> = FxHashMap::default();
        let mut inside_connected: FxHashSet<VarId> = FxHashSet::default();
        // Bus signals that only link expandable connectors to each other, with no
        // component writing or reading them, are not part of the model.
        // A variable can sit in two sets (seen from inside and from outside), so sets that
        // share a variable are judged together.
        let mut dead_set = vec![false; sets.len()];
        if !self.expandable_vars.is_empty() {
            let expandable: FxHashSet<VarId> = self.expandable_vars.iter().copied().collect();
            let used = self.referenced_vars();
            let mut group: Vec<usize> = (0..sets.len()).collect();
            fn root(g: &mut [usize], mut x: usize) -> usize {
                while g[x] != x {
                    g[x] = g[g[x]];
                    x = g[x];
                }
                x
            }
            let mut owner: FxHashMap<VarId, usize> = FxHashMap::default();
            for (k, (_, set)) in sets.iter().enumerate() {
                for p in set {
                    if let Some(&j) = owner.get(&p.var) {
                        let (a, b) = (root(&mut group, j), root(&mut group, k));
                        group[a] = b;
                    } else {
                        owner.insert(p.var, k);
                    }
                }
            }
            let mut live = vec![false; sets.len()];
            for (k, (_, set)) in sets.iter().enumerate() {
                if set
                    .iter()
                    .any(|p| !expandable.contains(&p.var) || used.contains(&p.var))
                {
                    let r = root(&mut group, k);
                    live[r] = true;
                }
            }
            for k in 0..sets.len() {
                let r = root(&mut group, k);
                dead_set[k] = !live[r];
            }
        }
        for (k, (_, set)) in sets.iter().enumerate() {
            if set.len() < 2 || dead_set[k] {
                continue;
            }
            for p in set {
                if p.inside {
                    inside_connected.insert(p.var);
                }
            }
            let flow = self.conn_flow.get(&set[0].var).copied().unwrap_or(false);
            if flow && set.iter().all(|p| p.inside) {
                flow_sets.push((
                    set.iter().map(|p| p.var).collect(),
                    self.model.equations.len(),
                ));
            }
            if !flow {
                for p in set {
                    pot_set.insert(p.var, k);
                }
            }
            if flow {
                let mut sum = Arena::ZERO;
                for p in set {
                    let v = self.model.arena.var(p.var);
                    sum = if p.inside {
                        self.model.arena.add(sum, v)
                    } else {
                        self.model.arena.sub(sum, v)
                    };
                }
                self.model.equations.push(Equation {
                    lhs: sum,
                    rhs: Arena::ZERO,
                    origin,
                });
            } else {
                let v0 = self.model.arena.var(set[0].var);
                for p in &set[1..] {
                    if p.var == set[0].var {
                        continue;
                    }
                    let v = self.model.arena.var(p.var);
                    self.model.equations.push(Equation {
                        lhs: v0,
                        rhs: v,
                        origin,
                    });
                }
            }
        }
        self.semi_linear_rule(&flow_sets, &pot_set);
        // Unconnected flow variables (as inside connectors) are zero (MLS §9.2).
        let flows = std::mem::take(&mut self.connector_flows);
        let mut seen = FxHashSet::default();
        let o2 = self.model.origin("unconnected flow variable");
        for f in flows {
            if !seen.insert(f) || inside_connected.contains(&f) {
                continue;
            }
            if self.model.vars[f as usize].kind != VarKind::Continuous {
                continue;
            }
            let v = self.model.arena.var(f);
            self.model.equations.push(Equation {
                lhs: v,
                rhs: Arena::ZERO,
                origin: o2,
            });
        }
    }

    /// MLS 3.7.4.5: components joined by one connection, each with
    /// `H_i = semiLinear(m_i, s, h_i)`, leave `s` undetermined at zero flow. The balance
    /// `sum(H_i) = 0` becomes the mixing rule it implies,
    /// `s = sum(max(-m_i, 0) h_i) / sum(max(-m_i, 0))` (`s = if m_1 >= 0 then h_2 else h_1`
    /// for two), which falls back to `h_1` when nothing flows.
    fn semi_linear_rule(
        &mut self,
        flow_sets: &[(Vec<VarId>, usize)],
        pot_set: &FxHashMap<VarId, usize>,
    ) {
        if self.semi_linear.is_empty() {
            return;
        }
        let by_res: FxHashMap<ExprId, usize> = self
            .semi_linear
            .iter()
            .enumerate()
            .map(|(i, r)| (r.0, i))
            .collect();
        // y variable of each semiLinear equation `y = semiLinear(..)`.
        let mut semi_of: FxHashMap<VarId, usize> = FxHashMap::default();
        for e in &self.model.equations {
            for (a, b) in [(e.lhs, e.rhs), (e.rhs, e.lhs)] {
                if let (Node::Var(y), Some(&i)) = (self.model.arena.node(a), by_res.get(&b)) {
                    semi_of.insert(y, i);
                }
            }
        }
        let mut set_of: FxHashMap<VarId, usize> = FxHashMap::default();
        for (k, (vs, _)) in flow_sets.iter().enumerate() {
            for &v in vs {
                set_of.insert(v, k);
            }
        }
        for (ys, eq) in flow_sets {
            let Some(mut recs) = ys
                .iter()
                .map(|y| semi_of.get(y).map(|&i| (*y, self.semi_linear[i])))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            // Members in name order, as OpenModelica orders a connection set; that
            // decides which side wins at zero flow.
            recs.sort_by(|a, b| {
                self.model.vars[a.0 as usize]
                    .name
                    .cmp(&self.model.vars[b.0 as usize].name)
            });
            let recs: Vec<_> = recs.into_iter().map(|r| r.1).collect();
            let (_, x0, s0, h0) = recs[0];
            let mset = set_of.get(&x0);
            let ok = mset.is_some()
                && recs.iter().all(|&(_, x, s, _)| {
                    set_of.get(&x) == mset
                        && (s == s0 || pot_set.get(&s).is_some_and(|k| pot_set.get(&s0) == Some(k)))
                })
                && flow_sets[*mset.unwrap()].0.len() == recs.len();
            if !ok {
                continue;
            }
            let a = &mut self.model.arena;
            let rhs = if recs.len() == 2 {
                let xv = a.var(x0);
                let c = a.cmp(Cmp::Ge, xv, Arena::ZERO);
                a.if_(c, recs[1].3, h0)
            } else {
                let (mut w, mut num) = (Arena::ZERO, Arena::ZERO);
                for &(_, x, _, h) in &recs {
                    let xv = a.var(x);
                    let nx = a.neg(xv);
                    let out = a.f2(Fn2::Max, nx, Arena::ZERO);
                    w = a.add(w, out);
                    let t = a.mul(out, h);
                    num = a.add(num, t);
                }
                let c = a.cmp(Cmp::Gt, w, Arena::ZERO);
                let q = a.div(num, w);
                a.if_(c, q, h0)
            };
            let lhs = a.var(s0);
            let e = &mut self.model.equations[*eq];
            e.lhs = lhs;
            e.rhs = rhs;
        }
    }

    fn finish_discrete(&mut self) {
        for &v in &self.when_assigned {
            let var = &mut self.model.vars[v as usize];
            if var.kind == VarKind::Continuous {
                var.kind = VarKind::Discrete;
            }
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    pub fn translate(&mut self, e: &'a E, scope: &Scope, ctx: &mut Ctx) -> Result<Val> {
        match e {
            E::Empty { .. } => Ok(Val::S(Arena::ZERO)),
            E::Terminal {
                terminal_type,
                token,
                ..
            } => match terminal_type {
                TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                    let v: f64 = token
                        .text
                        .parse()
                        .map_err(|_| anyhow!("bad number '{}'", token.text))?;
                    Ok(Val::S(self.model.arena.constant(v)))
                }
                TerminalType::Bool => Ok(Val::S(if &*token.text == "true" {
                    Arena::ONE
                } else {
                    Arena::ZERO
                })),
                TerminalType::String => Ok(Val::Str(token.text.trim_matches('"').to_string())),
                TerminalType::End => {
                    let v = ctx
                        .end_value
                        .ok_or_else(|| anyhow!("'end' outside of a subscript"))?;
                    Ok(Val::S(self.model.arena.constant(v as f64)))
                }
                TerminalType::Empty => Ok(Val::S(Arena::ZERO)),
            },
            E::Parenthesized { inner, .. } => self.translate(inner, scope, ctx),
            E::Unary { op, rhs, .. } => {
                let v = self.translate(rhs, scope, ctx)?;
                if Self::is_record_like(&v) && matches!(op, OpUnary::Minus | OpUnary::DotMinus) {
                    let (re, im) = self.complex_parts(v)?;
                    let re = self.map1(re, &mut |a, x| a.neg(x))?;
                    let im = self.map1(im, &mut |a, x| a.neg(x))?;
                    return Ok(Self::complex(re, im));
                }
                match op {
                    OpUnary::Minus | OpUnary::DotMinus => self.map1(v, &mut |a, x| a.neg(x)),
                    OpUnary::Not => self.map1(v, &mut |a, x| a.not(x)),
                    _ => Ok(v),
                }
            }
            E::Binary { op, lhs, rhs, .. } => {
                let l = self.translate(lhs, scope, ctx)?;
                let r = self.translate(rhs, scope, ctx)?;
                self.binary(op, l, r, ctx)
            }
            E::ComponentReference(cr) => self.resolve_ref(cr, scope, ctx),
            E::FunctionCall {
                comp,
                args,
                is_partial_application: true,
                ..
            } => self.function_value(comp, args, scope, ctx),
            E::FunctionCall { comp, args, .. } => self.call(comp, args, scope, ctx),
            E::Array { elements, kind, .. } => {
                let mut vals = Vec::new();
                for el in elements {
                    vals.push(self.translate(el, scope, ctx)?);
                }
                self.array_construct(vals, *kind)
            }
            E::Tuple { elements, .. } => {
                let mut v = Vec::new();
                for el in elements {
                    if matches!(el, E::Empty { .. }) {
                        v.push(Val::S(Arena::ZERO));
                    } else {
                        v.push(self.translate(el, scope, ctx)?);
                    }
                }
                Ok(Val::Tuple(v))
            }
            E::If {
                branches,
                else_branch,
                ..
            } => {
                // Conditions that are constant pick their branch statically, so the branches
                // not taken are never translated (they may not even be valid there).
                let mut live: Vec<(ExprId, &'a E)> = Vec::new();
                let mut last: &'a E = else_branch;
                for (c, b) in branches.iter() {
                    let cv = self.translate(c, scope, ctx)?.scalar()?;
                    let known = self
                        .model
                        .arena
                        .as_const(cv)
                        .or_else(|| self.eval_param_expr(cv));
                    match known {
                        Some(x) if x != 0.0 => {
                            last = b;
                            break;
                        }
                        Some(_) => {}
                        None => live.push((cv, b)),
                    }
                }
                let mut r = self.translate(last, scope, ctx)?;
                for (cv, b) in live.into_iter().rev() {
                    let bv = self.translate(b, scope, ctx)?;
                    r = self.merge_value(cv, bv, r)?;
                }
                Ok(r)
            }
            E::Range {
                start, step, end, ..
            } => {
                let s = self.translate(start, scope, ctx)?.scalar()?;
                let en = self.translate(end, scope, ctx)?.scalar()?;
                let st = match step {
                    Some(x) => self.translate(x, scope, ctx)?.scalar()?,
                    None => Arena::ONE,
                };
                let (s, en, st) = (
                    self.eval_param_expr(s)
                        .ok_or_else(|| anyhow!("range start must be a parameter expression"))?,
                    self.eval_param_expr(en)
                        .ok_or_else(|| anyhow!("range end must be a parameter expression"))?,
                    self.eval_param_expr(st)
                        .ok_or_else(|| anyhow!("range step must be a parameter expression"))?,
                );
                let mut v = Vec::new();
                if st != 0.0 {
                    let n = ((en - s) / st + 1e-9).floor();
                    if n >= 0.0 {
                        for k in 0..=(n as i64) {
                            v.push(self.model.arena.constant(s + k as f64 * st));
                        }
                    }
                }
                let n = v.len();
                Ok(Val::A(vec![n], v))
            }
            E::ArrayComprehension { expr, indices, .. }
                if self.comprehension_is_record(expr, indices, scope, ctx) =>
            {
                let mut vals = Vec::new();
                let mut outer = Vec::new();
                self.for_loop_vals(indices, scope, ctx, expr, &mut vals, &mut outer)?;
                self.stack_vals(outer, vals)
            }
            E::ArrayComprehension { expr, indices, .. } => {
                let mut out = Vec::new();
                let mut inner_dims = Vec::new();
                let mut outer = Vec::new();
                self.for_loop_collect(
                    indices,
                    scope,
                    ctx,
                    expr,
                    &mut out,
                    &mut inner_dims,
                    &mut outer,
                )?;
                let mut dims = outer;
                dims.extend(inner_dims);
                Ok(mk_arr(dims, out))
            }
            E::ArrayIndex {
                base, subscripts, ..
            } => {
                let b = self.translate(base, scope, ctx)?;
                self.index_val(b, subscripts, scope, ctx)
            }
            E::FieldAccess { base, field, .. } => {
                let b = self.translate(base, scope, ctx)?;
                self.field(b, field)
            }
            E::NamedArgument { value, .. } => self.translate(value, scope, ctx),
            E::Modification { value, .. } => self.translate(value, scope, ctx),
            E::ClassModification { .. } => bail!("unexpected class modification in expression"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn for_loop_collect(
        &mut self,
        indices: &'a [ast::ForIndex],
        scope: &Scope,
        ctx: &mut Ctx,
        body: &'a E,
        out: &mut Vec<ExprId>,
        inner_dims: &mut Vec<usize>,
        outer_dims: &mut Vec<usize>,
    ) -> Result<()> {
        if indices.is_empty() {
            let v = self.translate(body, scope, ctx)?;
            let (d, e) = v.elems()?;
            *inner_dims = d;
            out.extend(e);
            return Ok(());
        }
        let idx = &indices[0];
        let range = self.translate(&idx.range, scope, ctx)?;
        let (_, vals) = range.elems()?;
        if outer_dims.len() < indices.len() {
            outer_dims.push(vals.len());
        }
        let name = idx.ident.text.to_string();
        for v in vals {
            ctx.locals.push(FxHashMap::default());
            ctx.locals
                .last_mut()
                .unwrap()
                .insert(name.clone(), Val::S(v));
            let r =
                self.for_loop_collect(&indices[1..], scope, ctx, body, out, inner_dims, outer_dims);
            ctx.locals.pop();
            r?;
        }
        Ok(())
    }

    /// Does the body of `{body for i in ..}` produce records? (evaluated for the first index)
    fn comprehension_is_record(
        &mut self,
        body: &'a E,
        indices: &'a [ast::ForIndex],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> bool {
        let mut vals = Vec::new();
        let mut outer = Vec::new();
        let first: Vec<ast::ForIndex> = Vec::new();
        let _ = first;
        let r = self.for_loop_vals_first(indices, scope, ctx, body, &mut vals, &mut outer);
        r.is_ok() && vals.first().is_some_and(Self::is_record_like)
    }

    fn for_loop_vals_first(
        &mut self,
        indices: &'a [ast::ForIndex],
        scope: &Scope,
        ctx: &mut Ctx,
        body: &'a E,
        out: &mut Vec<Val>,
        outer_dims: &mut Vec<usize>,
    ) -> Result<()> {
        if indices.is_empty() {
            out.push(self.translate(body, scope, ctx)?);
            return Ok(());
        }
        let idx = &indices[0];
        let range = self.translate(&idx.range, scope, ctx)?;
        let (_, vals) = range.elems()?;
        let Some(&v) = vals.first() else {
            return Ok(());
        };
        outer_dims.push(vals.len());
        ctx.locals.push(FxHashMap::default());
        ctx.locals
            .last_mut()
            .unwrap()
            .insert(idx.ident.text.to_string(), Val::S(v));
        let r = self.for_loop_vals_first(&indices[1..], scope, ctx, body, out, outer_dims);
        ctx.locals.pop();
        r
    }

    fn for_loop_vals(
        &mut self,
        indices: &'a [ast::ForIndex],
        scope: &Scope,
        ctx: &mut Ctx,
        body: &'a E,
        out: &mut Vec<Val>,
        outer_dims: &mut Vec<usize>,
    ) -> Result<()> {
        if indices.is_empty() {
            out.push(self.translate(body, scope, ctx)?);
            return Ok(());
        }
        let idx = &indices[0];
        let range = self.translate(&idx.range, scope, ctx)?;
        let (_, vals) = range.elems()?;
        if outer_dims.len() < indices.len() {
            outer_dims.push(vals.len());
        }
        let name = idx.ident.text.to_string();
        for v in vals {
            ctx.locals.push(FxHashMap::default());
            ctx.locals
                .last_mut()
                .unwrap()
                .insert(name.clone(), Val::S(v));
            let r = self.for_loop_vals(&indices[1..], scope, ctx, body, out, outer_dims);
            ctx.locals.pop();
            r?;
        }
        Ok(())
    }

    fn field(&mut self, b: Val, field: &str) -> Result<Val> {
        match b {
            Val::Rec(fields) => {
                let names: Vec<String> = fields.iter().map(|(n, _)| n.clone()).collect();
                fields
                    .into_iter()
                    .find(|(n, _)| n == field)
                    .map(|(_, v)| v)
                    .ok_or_else(|| {
                        anyhow!(
                            "record has no field '{field}' (fields: {})",
                            names.join(", ")
                        )
                    })
            }
            Val::Inst(i) => {
                let c = self.insts[i as usize]
                    .comps
                    .get(field)
                    .cloned()
                    .ok_or_else(|| anyhow!("no element '{field}'"))?;
                self.comp_to_val(&c)
            }
            Val::IA(dims, ids) => {
                let mut vals = Vec::with_capacity(ids.len());
                for i in ids {
                    vals.push(self.field(Val::Inst(i), field)?);
                }
                self.stack_vals(dims, vals)
            }
            other => bail!("field access '.{field}' on {}", other.kind()),
        }
    }

    /// Stack equally shaped values into an array with leading `dims`. Records become
    /// records of arrays (an array of records is stored field by field).
    fn stack_vals(&mut self, dims: Vec<usize>, vals: Vec<Val>) -> Result<Val> {
        if vals.is_empty() {
            return Ok(Val::A(dims, vec![]));
        }
        if vals.iter().all(|v| matches!(v, Val::Inst(_))) {
            let ids = vals
                .iter()
                .map(|v| if let Val::Inst(i) = v { *i } else { 0 })
                .collect();
            return Ok(Val::IA(dims, ids));
        }
        if vals
            .iter()
            .any(|v| matches!(v, Val::Rec(_) | Val::Inst(_) | Val::IA(..)))
        {
            let mut recs = Vec::with_capacity(vals.len());
            for v in vals {
                recs.push(self.to_rec(v)?);
            }
            let Val::Rec(first) = &recs[0] else {
                unreachable!()
            };
            let names: Vec<String> = first.iter().map(|(n, _)| n.clone()).collect();
            let mut out = Vec::with_capacity(names.len());
            for n in names {
                let mut fv = Vec::with_capacity(recs.len());
                for r in &recs {
                    let Val::Rec(f) = r else { unreachable!() };
                    fv.push(
                        f.iter()
                            .find(|(m, _)| *m == n)
                            .map(|(_, v)| v.clone())
                            .unwrap_or(Val::S(Arena::ZERO)),
                    );
                }
                let st = if fv.iter().all(|v| matches!(v, Val::Str(_))) {
                    fv[0].clone()
                } else {
                    self.stack_vals(dims.clone(), fv)?
                };
                out.push((n, st));
            }
            return Ok(Val::Rec(out));
        }
        let mut inner: Option<Vec<usize>> = None;
        let mut out = Vec::new();
        for v in &vals {
            let (d, e) = v.elems()?;
            if inner.as_ref().is_some_and(|i| *i != d) {
                bail!("array of values with inconsistent sizes");
            }
            inner = Some(d);
            out.extend(e);
        }
        let mut d = dims;
        d.extend(inner.unwrap_or_default());
        Ok(mk_arr(d, out))
    }

    /// A record value: instances and instance arrays are read field by field.
    fn to_rec(&mut self, v: Val) -> Result<Val> {
        match v {
            Val::Rec(_) => Ok(v),
            Val::Inst(_) => self.as_record(&v),
            Val::IA(dims, ids) => {
                let mut recs = Vec::with_capacity(ids.len());
                for i in ids {
                    recs.push(self.as_record(&Val::Inst(i))?);
                }
                self.stack_vals(dims, recs)
            }
            Val::Tuple(t) if !t.is_empty() => self.to_rec(t[0].clone()),
            other => bail!("expected a record, found {}", other.kind()),
        }
    }

    fn is_record_like(v: &Val) -> bool {
        matches!(v, Val::Rec(_) | Val::Inst(_) | Val::IA(..))
    }

    /// Complex view (re, im) of a value: a record with fields re/im, or a real promoted.
    fn complex_parts(&mut self, v: Val) -> Result<(Val, Val)> {
        if Self::is_record_like(&v) {
            let r = self.to_rec(v)?;
            let re = self.field(r.clone(), "re")?;
            let im = self.field(r, "im")?;
            return Ok((re, im));
        }
        let z = self.map1(v.clone(), &mut |_, _| Arena::ZERO)?;
        Ok((v, z))
    }

    fn complex(re: Val, im: Val) -> Val {
        Val::Rec(vec![("re".to_string(), re), ("im".to_string(), im)])
    }

    /// Operators on records: field-wise addition and subtraction, and the Complex
    /// operator record's multiplication, division and comparison.
    fn record_binary(&mut self, op: &OpBinary, l: Val, r: Val, ctx: &Ctx) -> Result<Val> {
        let (a, b) = self.complex_parts(l)?;
        let (c, d) = self.complex_parts(r)?;
        let add = |s: &mut Self, x: Val, y: Val| s.zip2(x, y, &mut |ar, p, q| ar.add(p, q));
        let sub = |s: &mut Self, x: Val, y: Val| s.zip2(x, y, &mut |ar, p, q| ar.sub(p, q));
        let mul = |s: &mut Self, x: Val, y: Val| s.zip2(x, y, &mut |ar, p, q| ar.mul(p, q));
        match op {
            OpBinary::Add | OpBinary::AddElem => {
                let re = add(self, a, c)?;
                let im = add(self, b, d)?;
                Ok(Self::complex(re, im))
            }
            OpBinary::Sub | OpBinary::SubElem => {
                let re = sub(self, a, c)?;
                let im = sub(self, b, d)?;
                Ok(Self::complex(re, im))
            }
            OpBinary::Mul if matches!((&a, &c), (Val::A(d1, _), Val::A(d2, _)) if d1.len() == 1 && d2.len() == 1) =>
            {
                // Complex vector * vector: scalar product sum(conj(c1[k])*c2[k]).
                let ac = mul(self, a.clone(), c.clone())?;
                let bd = mul(self, b.clone(), d.clone())?;
                let ad = mul(self, a, d)?;
                let bc = mul(self, b, c)?;
                let re = add(self, ac, bd)?;
                let im = sub(self, ad, bc)?;
                let (_, re) = re.elems()?;
                let (_, im) = im.elems()?;
                let ar = &mut self.model.arena;
                let mut sr = Arena::ZERO;
                let mut si = Arena::ZERO;
                for x in re {
                    sr = ar.add(sr, x);
                }
                for x in im {
                    si = ar.add(si, x);
                }
                Ok(Self::complex(Val::S(sr), Val::S(si)))
            }
            OpBinary::Mul | OpBinary::MulElem => {
                let ac = mul(self, a.clone(), c.clone())?;
                let bd = mul(self, b.clone(), d.clone())?;
                let ad = mul(self, a, d)?;
                let bc = mul(self, b, c)?;
                let re = sub(self, ac, bd)?;
                let im = add(self, ad, bc)?;
                Ok(Self::complex(re, im))
            }
            OpBinary::Div | OpBinary::DivElem => {
                let cc = mul(self, c.clone(), c.clone())?;
                let dd = mul(self, d.clone(), d.clone())?;
                let den = add(self, cc, dd)?;
                let ac = mul(self, a.clone(), c.clone())?;
                let bd = mul(self, b.clone(), d.clone())?;
                let bc = mul(self, b, c)?;
                let ad = mul(self, a, d)?;
                let nre = add(self, ac, bd)?;
                let nim = sub(self, bc, ad)?;
                let re = self.zip2(nre, den.clone(), &mut |ar, p, q| ar.div(p, q))?;
                let im = self.zip2(nim, den, &mut |ar, p, q| ar.div(p, q))?;
                Ok(Self::complex(re, im))
            }
            OpBinary::Eq | OpBinary::Neq => {
                let e1 = self.binary(&OpBinary::Eq, a, c, ctx)?;
                let e2 = self.binary(&OpBinary::Eq, b, d, ctx)?;
                let both = self.zip2(e1, e2, &mut |ar, p, q| ar.f2(Fn2::And, p, q))?;
                if matches!(op, OpBinary::Neq) {
                    self.map1(both, &mut |ar, x| ar.not(x))
                } else {
                    Ok(both)
                }
            }
            _ => bail!("operator not defined for records"),
        }
    }

    fn map1(&mut self, v: Val, f: &mut dyn FnMut(&mut Arena, ExprId) -> ExprId) -> Result<Val> {
        match v {
            Val::S(x) => Ok(Val::S(f(&mut self.model.arena, x))),
            Val::A(d, xs) => Ok(Val::A(
                d,
                xs.into_iter()
                    .map(|x| f(&mut self.model.arena, x))
                    .collect(),
            )),
            Val::Tuple(t) if !t.is_empty() => self.map1(t[0].clone(), f),
            v @ (Val::Rec(_) | Val::Inst(_) | Val::IA(..)) => {
                let Val::Rec(fields) = self.to_rec(v)? else {
                    unreachable!()
                };
                let mut out = Vec::with_capacity(fields.len());
                for (n, x) in fields {
                    let m = if matches!(x, Val::Str(_)) {
                        x
                    } else {
                        self.map1(x, f)?
                    };
                    out.push((n, m));
                }
                Ok(Val::Rec(out))
            }
            other => bail!("numeric operation on {}", other.kind()),
        }
    }

    fn zip2(
        &mut self,
        a: Val,
        b: Val,
        f: &mut dyn FnMut(&mut Arena, ExprId, ExprId) -> ExprId,
    ) -> Result<Val> {
        let (ad, ae) = a.elems()?;
        let (bd, be) = b.elems()?;
        if ae.len() == be.len() {
            let dims = if ad.len() >= bd.len() { ad } else { bd };
            let v = ae
                .into_iter()
                .zip(be)
                .map(|(x, y)| f(&mut self.model.arena, x, y))
                .collect();
            return Ok(mk_arr(dims, v));
        }
        if ae.len() == 1 && ad.is_empty() {
            let v = be
                .into_iter()
                .map(|y| f(&mut self.model.arena, ae[0], y))
                .collect();
            return Ok(mk_arr(bd, v));
        }
        if be.len() == 1 && bd.is_empty() {
            let v = ae
                .into_iter()
                .map(|x| f(&mut self.model.arena, x, be[0]))
                .collect();
            return Ok(mk_arr(ad, v));
        }
        bail!("array size mismatch {:?} vs {:?}", ad, bd)
    }

    fn binary(&mut self, op: &OpBinary, l: Val, r: Val, ctx: &Ctx) -> Result<Val> {
        // String concatenation (messages of assert, error and print).
        if let (Val::Str(x), Val::Str(y)) = (&l, &r)
            && matches!(op, OpBinary::Add | OpBinary::AddElem)
        {
            return Ok(Val::Str(format!("{x}{y}")));
        }
        if Self::is_record_like(&l) || Self::is_record_like(&r) {
            return self.record_binary(op, l, r, ctx);
        }
        // Relations in function bodies do not generate events (MLS 8.5).
        let no_event = ctx.no_event || ctx.in_function;
        match op {
            OpBinary::Add | OpBinary::AddElem => self.zip2(l, r, &mut |a, x, y| a.add(x, y)),
            OpBinary::Sub | OpBinary::SubElem => self.zip2(l, r, &mut |a, x, y| a.sub(x, y)),
            OpBinary::MulElem => self.zip2(l, r, &mut |a, x, y| a.mul(x, y)),
            OpBinary::DivElem | OpBinary::Div => self.zip2(l, r, &mut |a, x, y| a.div(x, y)),
            OpBinary::ExpElem | OpBinary::Exp => self.zip2(l, r, &mut |a, x, y| a.pow(x, y)),
            OpBinary::Mul => self.mul_val(l, r),
            OpBinary::And => self.zip2(l, r, &mut |a, x, y| a.f2(Fn2::And, x, y)),
            OpBinary::Or => self.zip2(l, r, &mut |a, x, y| a.f2(Fn2::Or, x, y)),
            OpBinary::Lt
            | OpBinary::Le
            | OpBinary::Gt
            | OpBinary::Ge
            | OpBinary::Eq
            | OpBinary::Neq => {
                let c = match op {
                    OpBinary::Lt => Cmp::Lt,
                    OpBinary::Le => Cmp::Le,
                    OpBinary::Gt => Cmp::Gt,
                    OpBinary::Ge => Cmp::Ge,
                    OpBinary::Eq => Cmp::Eq,
                    _ => Cmp::Ne,
                };
                if let (Val::Str(a), Val::Str(b)) = (&l, &r) {
                    let eq = a == b;
                    let v = match c {
                        Cmp::Eq => eq,
                        Cmp::Ne => !eq,
                        _ => false,
                    };
                    return Ok(Val::S(if v { Arena::ONE } else { Arena::ZERO }));
                }
                let x = l.scalar()?;
                let y = r.scalar()?;
                Ok(Val::S(self.relation(c, x, y, no_event)))
            }
            OpBinary::Empty | OpBinary::Assign => bail!("unsupported operator"),
        }
    }

    /// Relation: event-generating unless inside noEvent or time-invariant / discrete.
    fn relation(&mut self, c: Cmp, x: ExprId, y: ExprId, no_event: bool) -> ExprId {
        if no_event
            || matches!(c, Cmp::Eq | Cmp::Ne)
            || (self.is_discrete_expr(x) && self.is_discrete_expr(y))
        {
            return self.model.arena.cmp(c, x, y);
        }
        self.model.arena.relation(c, x, y)
    }

    /// True if the expression only changes at events (parameters, discrete vars, pre()).
    fn is_discrete_expr(&self, e: ExprId) -> bool {
        let mut ok = true;
        self.model.arena.visit_leaves(e, &mut |n| match n {
            Node::Var(v) => {
                let k = self.model.vars[v as usize].kind;
                if matches!(k, VarKind::Continuous) && !self.when_assigned.contains(&v) {
                    ok = false;
                }
            }
            Node::Der(_) | Node::Time => ok = false,
            _ => {}
        });
        ok
    }

    fn mul_val(&mut self, l: Val, r: Val) -> Result<Val> {
        let (ld, le) = l.elems()?;
        let (rd, re) = r.elems()?;
        let a = &mut self.model.arena;
        match (ld.len(), rd.len()) {
            (0, _) | (_, 0) => self.zip2(
                Val::A(ld, le).normalize(),
                Val::A(rd, re).normalize(),
                &mut |a, x, y| a.mul(x, y),
            ),
            (1, 1) => {
                if ld[0] != rd[0] {
                    bail!("vector size mismatch in product");
                }
                let mut s = Arena::ZERO;
                for (x, y) in le.into_iter().zip(re) {
                    let p = a.mul(x, y);
                    s = a.add(s, p);
                }
                Ok(Val::S(s))
            }
            (2, 1) => {
                let (n, m) = (ld[0], ld[1]);
                if m != rd[0] {
                    bail!("matrix-vector size mismatch");
                }
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let mut s = Arena::ZERO;
                    for j in 0..m {
                        let p = a.mul(le[i * m + j], re[j]);
                        s = a.add(s, p);
                    }
                    out.push(s);
                }
                Ok(Val::A(vec![n], out))
            }
            (1, 2) => {
                let (n, m) = (rd[0], rd[1]);
                if n != ld[0] {
                    bail!("vector-matrix size mismatch");
                }
                let mut out = Vec::with_capacity(m);
                for j in 0..m {
                    let mut s = Arena::ZERO;
                    for i in 0..n {
                        let p = a.mul(le[i], re[i * m + j]);
                        s = a.add(s, p);
                    }
                    out.push(s);
                }
                Ok(Val::A(vec![m], out))
            }
            (2, 2) => {
                let (n, k, m) = (ld[0], ld[1], rd[1]);
                if k != rd[0] {
                    bail!("matrix size mismatch");
                }
                let mut out = Vec::with_capacity(n * m);
                for i in 0..n {
                    for j in 0..m {
                        let mut s = Arena::ZERO;
                        for t in 0..k {
                            let p = a.mul(le[i * k + t], re[t * m + j]);
                            s = a.add(s, p);
                        }
                        out.push(s);
                    }
                }
                Ok(Val::A(vec![n, m], out))
            }
            _ => bail!("unsupported array product"),
        }
    }

    fn array_construct(&mut self, vals: Vec<Val>, kind: ArrayConstructor) -> Result<Val> {
        match kind {
            ArrayConstructor::Array => {
                if !vals.is_empty() && vals.iter().all(|v| matches!(v, Val::Str(_) | Val::SA(..))) {
                    let mut inner: Option<Vec<usize>> = None;
                    let mut out = Vec::new();
                    for v in &vals {
                        let (d, e) = match v {
                            Val::Str(x) => (vec![], vec![x.clone()]),
                            Val::SA(d, e) => (d.clone(), e.clone()),
                            _ => unreachable!(),
                        };
                        if inner.as_ref().is_some_and(|i| *i != d) {
                            bail!("array constructor with inconsistent element sizes");
                        }
                        inner = Some(d);
                        out.extend(e);
                    }
                    let mut dims = vec![vals.len()];
                    dims.extend(inner.unwrap_or_default());
                    return Ok(Val::SA(dims, out));
                }
                if vals.iter().all(|v| matches!(v, Val::Inst(_))) && !vals.is_empty() {
                    let v: Vec<InstId> = vals
                        .iter()
                        .map(|x| if let Val::Inst(i) = x { *i } else { 0 })
                        .collect();
                    return Ok(Val::IA(vec![v.len()], v));
                }
                if vals.iter().any(Self::is_record_like) {
                    let n = vals.len();
                    return self.stack_vals(vec![n], vals);
                }
                let mut inner: Option<Vec<usize>> = None;
                let mut out = Vec::new();
                for v in &vals {
                    let (d, e) = v.elems()?;
                    if let Some(id) = &inner {
                        if *id != d {
                            bail!("array constructor with inconsistent element sizes");
                        }
                    } else {
                        inner = Some(d);
                    }
                    out.extend(e);
                }
                let mut dims = vec![vals.len()];
                dims.extend(inner.unwrap_or_default());
                Ok(Val::A(dims, out))
            }
            ArrayConstructor::Horizontal => {
                // [a, b, c]: concatenate along dim 2, promoting scalars/vectors to columns.
                let mut rows: Option<usize> = None;
                let mut cols_data: Vec<Vec<ExprId>> = Vec::new(); // per column
                for v in &vals {
                    let (d, e) = v.elems()?;
                    let (r, c) = match d.len() {
                        0 => (1, 1),
                        1 => (d[0], 1),
                        _ => (d[0], d[1]),
                    };
                    if let Some(rr) = rows {
                        if rr != r {
                            bail!("horizontal concatenation row mismatch");
                        }
                    } else {
                        rows = Some(r);
                    }
                    for j in 0..c {
                        cols_data.push((0..r).map(|i| e[i * c + j]).collect());
                    }
                }
                let r = rows.unwrap_or(0);
                let c = cols_data.len();
                let mut out = Vec::with_capacity(r * c);
                for i in 0..r {
                    for col in &cols_data {
                        out.push(col[i]);
                    }
                }
                Ok(Val::A(vec![r, c], out))
            }
            ArrayConstructor::Vertical => {
                let mut cols: Option<usize> = None;
                let mut out = Vec::new();
                let mut nrows = 0;
                for v in &vals {
                    let (d, e) = v.elems()?;
                    let (r, c) = match d.len() {
                        0 => (1, 1),
                        1 => (1, d[0]),
                        _ => (d[0], d[1]),
                    };
                    if let Some(cc) = cols {
                        if cc != c {
                            bail!("vertical concatenation column mismatch");
                        }
                    } else {
                        cols = Some(c);
                    }
                    nrows += r;
                    out.extend(e);
                }
                Ok(Val::A(vec![nrows, cols.unwrap_or(0)], out))
            }
        }
    }

    /// `a[i, 1:4]` with run-time scalar subscripts (mixed with constant ones): every
    /// element of the result is a balanced selection tree over the elements the run-time
    /// subscripts can pick (the Digital logic tables, spline coefficient rows).
    fn dynamic_index(
        &mut self,
        dims: &[usize],
        el: &[ExprId],
        subs: &'a [ast::Subscript],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Option<Val>> {
        enum Sub {
            Fixed(Vec<usize>, bool),
            Dyn(ExprId),
        }
        if subs.len() != dims.len() || el.is_empty() {
            return Ok(None);
        }
        let mut spec = Vec::with_capacity(subs.len());
        for (k, s) in subs.iter().enumerate() {
            match s {
                ast::Subscript::Range { .. } | ast::Subscript::Empty => {
                    spec.push(Sub::Fixed((0..dims[k]).collect(), true))
                }
                ast::Subscript::Expression(e) => {
                    let saved = ctx.end_value;
                    ctx.end_value = Some(dims[k]);
                    let v = self.translate(e, scope, ctx);
                    ctx.end_value = saved;
                    let (d, es) = v?.elems()?;
                    let consts: Option<Vec<f64>> =
                        es.iter().map(|&x| self.eval_param_expr(x)).collect();
                    match consts {
                        Some(c) => {
                            let mut idx = Vec::with_capacity(c.len());
                            for x in c {
                                let i = x.round() as i64;
                                if i < 1 || i as usize > dims[k] {
                                    bail!("subscript {} out of bounds 1..{}", i, dims[k]);
                                }
                                idx.push(i as usize - 1);
                            }
                            spec.push(Sub::Fixed(idx, !d.is_empty()))
                        }
                        None if d.is_empty() => spec.push(Sub::Dyn(es[0])),
                        None => return Ok(None),
                    }
                }
            }
        }
        let mut strides = vec![1usize; dims.len()];
        for k in (0..dims.len().saturating_sub(1)).rev() {
            strides[k] = strides[k + 1] * dims[k + 1];
        }
        // linear index over the run-time dimensions
        let a = &mut self.model.arena;
        let mut lin = Arena::ZERO;
        let mut dstride = 1usize;
        let mut dyn_dims = Vec::new();
        for k in (0..dims.len()).rev() {
            if let Sub::Dyn(x) = spec[k] {
                let one = a.constant(1.0);
                let i0 = a.sub(x, one);
                let st = a.constant(dstride as f64);
                let t = a.mul(i0, st);
                lin = a.add(lin, t);
                dstride *= dims[k];
                dyn_dims.push(k);
            }
        }
        dyn_dims.reverse();
        fn tree(a: &mut Arena, lin: ExprId, el: &[ExprId], lo: usize) -> ExprId {
            if el.len() == 1 {
                return el[0];
            }
            let mid = el.len() / 2;
            let l = tree(a, lin, &el[..mid], lo);
            let r = tree(a, lin, &el[mid..], lo + mid);
            if l == r {
                return l;
            }
            let th = a.constant((lo + mid) as f64 - 0.5);
            let c = a.cmp(Cmp::Lt, lin, th);
            a.if_(c, l, r)
        }
        // cartesian product over the fixed subscripts (row-major)
        let mut out_dims = Vec::new();
        let mut fixed: Vec<(usize, &Vec<usize>)> = Vec::new();
        for (k, sp) in spec.iter().enumerate() {
            if let Sub::Fixed(idx, keep) = sp {
                if *keep {
                    out_dims.push(idx.len());
                }
                fixed.push((k, idx));
            }
        }
        let n_out: usize = fixed.iter().map(|(_, i)| i.len()).product();
        let mut vals = Vec::with_capacity(n_out);
        for o in 0..n_out {
            let mut rem = o;
            let mut off = 0;
            for (k, idx) in fixed.iter().rev() {
                off += idx[rem % idx.len()] * strides[*k];
                rem /= idx.len();
            }
            let n_dyn: usize = dyn_dims.iter().map(|&k| dims[k]).product();
            let mut cand = Vec::with_capacity(n_dyn);
            for c in 0..n_dyn {
                let mut rem = c;
                let mut lin_off = off;
                for &k in dyn_dims.iter().rev() {
                    lin_off += (rem % dims[k]) * strides[k];
                    rem /= dims[k];
                }
                cand.push(el[lin_off]);
            }
            vals.push(tree(&mut self.model.arena, lin, &cand, 0));
        }
        Ok(Some(if out_dims.is_empty() {
            Val::S(vals[0])
        } else {
            Val::A(out_dims, vals)
        }))
    }

    fn eval_subscripts(
        &mut self,
        subs: &'a [ast::Subscript],
        dims: &[usize],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Vec<Option<Vec<usize>>>> {
        let mut sel = Vec::new();
        for (k, s) in subs.iter().enumerate() {
            let dim = dims.get(k).copied().unwrap_or(1);
            match s {
                ast::Subscript::Range { .. } | ast::Subscript::Empty => sel.push(None),
                ast::Subscript::Expression(e) => {
                    let saved = ctx.end_value;
                    ctx.end_value = Some(dim);
                    let v = self.translate(e, scope, ctx);
                    ctx.end_value = saved;
                    let v = v?;
                    let (d, es) = v.elems()?;
                    let mut idx = Vec::new();
                    for x in es {
                        let val = self.eval_param_expr(x).ok_or_else(|| {
                            anyhow!(
                                "subscript '{}' must be a constant or parameter expression",
                                self.model.show(x)
                            )
                        })?;
                        let i = val.round() as i64;
                        if i < 1 || i as usize > dim {
                            bail!("subscript {} out of bounds 1..{}", i, dim);
                        }
                        idx.push(i as usize - 1);
                    }
                    if d.is_empty() {
                        sel.push(Some(idx));
                    } else {
                        // vector subscript keeps the dimension: marker via usize::MAX prefix
                        let mut v = vec![usize::MAX];
                        v.extend(idx);
                        sel.push(Some(v));
                    }
                }
            }
        }
        Ok(sel)
    }

    fn index_val(
        &mut self,
        b: Val,
        subs: &'a [ast::Subscript],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        match b {
            Val::A(dims, el) => {
                let sel = match self.eval_subscripts(subs, &dims, scope, ctx) {
                    Ok(sel) => sel,
                    Err(err) => {
                        return match self.dynamic_index(&dims, &el, subs, scope, ctx)? {
                            Some(v) => Ok(v),
                            None => Err(err),
                        };
                    }
                };
                let (nd, idx) = select_indices(&dims, &sel);
                Ok(mk_arr(nd, idx.into_iter().map(|i| el[i]).collect()))
            }
            Val::SA(dims, el) => {
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (nd, idx) = select_indices(&dims, &sel);
                let v: Vec<String> = idx.into_iter().map(|i| el[i].clone()).collect();
                if nd.is_empty() {
                    Ok(Val::Str(v[0].clone()))
                } else {
                    Ok(Val::SA(nd, v))
                }
            }
            Val::IA(dims, el) => {
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (nd, idx) = select_indices(&dims, &sel);
                let v: Vec<InstId> = idx.into_iter().map(|i| el[i]).collect();
                if nd.is_empty() {
                    Ok(Val::Inst(v[0]))
                } else {
                    Ok(Val::IA(nd, v))
                }
            }
            Val::S(e)
                if subs
                    .iter()
                    .all(|s| matches!(s, ast::Subscript::Expression(_))) =>
            {
                Ok(Val::S(e))
            }
            Val::Rec(fields) => {
                // Array of records stored field by field: index every field.
                let mut out = Vec::with_capacity(fields.len());
                for (n, v) in fields {
                    let iv = if matches!(v, Val::Str(_)) {
                        v
                    } else {
                        self.index_val(v, subs, scope, ctx)?
                    };
                    out.push((n, iv));
                }
                Ok(Val::Rec(out))
            }
            other => bail!("subscript on {}", other.kind()),
        }
    }

    fn comp_to_val(&mut self, c: &Comp) -> Result<Val> {
        match c {
            Comp::Var(v) => Ok(Val::S(self.var_ref(*v))),
            Comp::Inst(i) => Ok(Val::Inst(*i)),
            Comp::Arr(dims, el) => {
                if el.iter().all(|e| matches!(e, Comp::Var(_))) {
                    let v = el
                        .iter()
                        .map(|e| {
                            if let Comp::Var(x) = e {
                                self.var_ref(*x)
                            } else {
                                Arena::ZERO
                            }
                        })
                        .collect();
                    Ok(Val::A(dims.clone(), v))
                } else if el.iter().all(|e| matches!(e, Comp::Inst(_))) {
                    let v = el
                        .iter()
                        .map(|e| if let Comp::Inst(x) = e { *x } else { 0 })
                        .collect();
                    Ok(Val::IA(dims.clone(), v))
                } else if !el.is_empty() && el.iter().all(|e| matches!(e, Comp::Str(_))) {
                    let v = el
                        .iter()
                        .map(|e| {
                            if let Comp::Str(x) = e {
                                x.to_string()
                            } else {
                                String::new()
                            }
                        })
                        .collect();
                    Ok(Val::SA(dims.clone(), v))
                } else {
                    // Arrays of arrays (from field access over component arrays): flatten.
                    let mut out = Vec::new();
                    let mut inner = Vec::new();
                    for e in el.iter() {
                        let v = self.comp_to_val(e)?;
                        let (d, x) = v.elems()?;
                        inner = d;
                        out.extend(x);
                    }
                    let mut d = dims.clone();
                    d.extend(inner);
                    Ok(Val::A(d, out))
                }
            }
            Comp::Str(x) => Ok(Val::Str(x.to_string())),
            Comp::Removed => bail!("reference to a conditional component that is not present"),
        }
    }

    /// Expression for a variable reference; constants are folded to their value.
    fn var_ref(&mut self, v: VarId) -> ExprId {
        if self.model.vars[v as usize].kind == VarKind::Constant {
            let _ = self.ensure_binding(v);
            if let Some(b) = self.model.vars[v as usize].binding
                && let Some(x) = self.eval_param_expr(b)
            {
                return self.model.arena.constant(x);
            }
        }
        self.model.arena.var(v)
    }

    fn resolve_ref(
        &mut self,
        cr: &'a ast::ComponentReference,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        let first = &*cr.parts[0].ident.text;
        // Locals (for iterators, function variables)
        for frame in ctx.locals.iter().rev() {
            if let Some(v) = frame.get(first) {
                let mut val = v.clone();
                if let Some(s) = &cr.parts[0].subs {
                    val = self.index_val(val, s, scope, ctx)?;
                }
                for p in &cr.parts[1..] {
                    val = self.field(val, &p.ident.text)?;
                    if let Some(s) = &p.subs {
                        val = self.index_val(val, s, scope, ctx)?;
                    }
                }
                return Ok(val);
            }
        }
        if first == "time" && cr.parts.len() == 1 {
            return Ok(Val::S(self.model.arena.time()));
        }
        if !ctx.in_function
            && !cr.local
            && let Some(inst) = scope.inst
        {
            self.ensure_component(inst, first)?;
        }
        if !ctx.in_function
            && !cr.local
            && let Some(inst) = scope.inst
            && let Some(c) = self.insts[inst as usize].comps.get(first).cloned()
        {
            let mut cur = c;
            for (i, p) in cr.parts.iter().enumerate() {
                if i > 0 {
                    cur = self
                        .child_comp(cur, &p.ident.text)
                        .with_context(|| format!("in reference '{cr}'"))?;
                }
                if let Some(subs) = &p.subs {
                    cur = match self.subscript_comp(cur.clone(), subs, scope, ctx) {
                        Ok(c) => c,
                        // run-time subscripts on the last part: index the value instead
                        Err(err) if i + 1 == cr.parts.len() => {
                            let v = self.comp_to_val(&cur)?;
                            return self.index_val(v, subs, scope, ctx).map_err(|_| err);
                        }
                        Err(err) => return Err(err),
                    };
                }
            }
            return self.comp_to_val(&cur);
        }
        // Constants and enumeration literals from classes.
        if let Some(v) = self.lookup_constant(cr, scope, ctx)? {
            return Ok(v);
        }
        bail!("unknown variable '{}'", cr)
    }

    fn child_comp(&self, c: Comp, name: &str) -> Result<Comp> {
        match c {
            Comp::Inst(id) => self.insts[id as usize]
                .comps
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("no element '{name}'")),
            Comp::Arr(dims, el) => {
                let mut v = Vec::new();
                for e in el.iter() {
                    v.push(self.child_comp(e.clone(), name)?);
                }
                Ok(Comp::Arr(dims, std::rc::Rc::new(v)))
            }
            Comp::Removed => Ok(Comp::Removed),
            Comp::Var(_) | Comp::Str(_) => bail!("cannot access '.{name}' of a scalar"),
        }
    }

    fn lookup_constant(
        &mut self,
        cr: &'a ast::ComponentReference,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Option<Val>> {
        let parts: Vec<&str> = cr.parts.iter().map(|p| &*p.ident.text).collect();
        let env = if cr.local {
            self.env_of(ROOT)
        } else {
            scope.env.clone()
        };
        if parts.len() == 1 {
            return self.lookup_unqualified_constant(cr, &env, scope, ctx);
        }
        // 2) Qualified: longest class prefix, then constant / enumeration literal.
        for k in (1..parts.len()).rev() {
            let Some(cls) = self.lookup_class(&env, &parts[..k]) else {
                continue;
            };
            let name = parts[k];
            if k == parts.len() - 1 {
                if let Some(v) = self.enum_literal(cls, name) {
                    return Ok(Some(Val::S(self.model.arena.constant(v))));
                }
                if let Some(v) = builtin_enum_literal(&self.tree.node(cls).name, name) {
                    return Ok(Some(Val::S(self.model.arena.constant(v))));
                }
            }
            if let Some((owner, comp, modv)) = self.find_constant(cls, name, 0) {
                // Respect overrides from the current environment when the class is redeclared.
                let oenv = if owner == cls {
                    Rc::new(Env {
                        class: owner,
                        overrides: env.overrides.clone(),
                        parent: self.env_of(owner).parent.clone(),
                    })
                } else {
                    self.inherited_env(cls, owner, env.overrides.clone())
                };
                let mut v = self.class_constant_in(cls, owner, comp, modv, oenv)?;
                v = self.apply_subs(v, &cr.parts[k], scope, ctx)?;
                for p in &cr.parts[k + 1..] {
                    v = self.field(v, &p.ident.text)?;
                }
                return Ok(Some(v));
            }
        }
        // Builtin enumerations without a declaring class (StateSelect.prefer, ...).
        if parts.len() == 2
            && let Some(v) = builtin_enum_literal(parts[0], parts[1])
        {
            return Ok(Some(Val::S(self.model.arena.constant(v))));
        }
        // A field of a record constant (`data.MM`).
        self.lookup_unqualified_constant(cr, &env, scope, ctx)
    }

    /// Constant `cr.parts[0]` visible in an enclosing class (or the function itself),
    /// followed by the record fields of the remaining parts.
    fn lookup_unqualified_constant(
        &mut self,
        cr: &'a ast::ComponentReference,
        env: &Rc<Env>,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Option<Val>> {
        let v = self.unqualified_constant(cr, env, scope, ctx)?;
        let Some(mut v) = v else { return Ok(None) };
        for p in &cr.parts[1..] {
            v = self.field(v, &p.ident.text)?;
            v = self.apply_subs(v, p, scope, ctx)?;
        }
        Ok(Some(v))
    }

    fn unqualified_constant(
        &mut self,
        cr: &'a ast::ComponentReference,
        env: &Rc<Env>,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Option<Val>> {
        let parts: Vec<&str> = cr.parts.iter().map(|p| &*p.ident.text).collect();
        let mut e: Option<Rc<Env>> = Some(env.clone());
        while let Some(en) = e {
            if let Some((owner, comp, modv)) = self.find_constant(en.class, parts[0], 0)
                && matches!(
                    comp.variability,
                    Variability::Constant(_) | Variability::Parameter(_)
                )
            {
                let oenv = if owner == en.class {
                    Rc::new(Env {
                        class: owner,
                        overrides: en.overrides.clone(),
                        parent: en.parent.clone(),
                    })
                } else {
                    self.inherited_env(en.class, owner, en.overrides.clone())
                };
                let v = self.class_constant_in(en.class, owner, comp, modv, oenv)?;
                return Ok(Some(self.apply_subs(v, &cr.parts[0], scope, ctx)?));
            }
            e = en.parent.clone();
        }
        if let Some((pkg, name)) = self.tree.lookup_import_constant(env.class, parts[0])
            && let Some((owner, comp)) = self.tree.find_component(pkg, &name)
        {
            let oenv = self.env_of(owner);
            let v = self.class_constant(owner, comp, oenv)?;
            return Ok(Some(v));
        }
        Ok(None)
    }

    fn apply_subs(
        &mut self,
        v: Val,
        part: &'a ast::ComponentRefPart,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        match &part.subs {
            Some(s) => self.index_val(v, s, scope, ctx),
            None => Ok(v),
        }
    }

    fn enum_size(&self, cls: ClassId) -> Option<usize> {
        let def = self.tree.def(cls)?;
        if !def.enum_literals.is_empty() {
            return Some(def.enum_literals.len());
        }
        self.tree
            .bases(cls)
            .into_iter()
            .find_map(|b| self.enum_size(b))
    }

    fn enum_literal(&self, cls: ClassId, name: &str) -> Option<f64> {
        let def = self.tree.def(cls)?;
        if let Some(i) = def
            .enum_literals
            .iter()
            .position(|l| &*l.ident.text == name)
        {
            return Some((i + 1) as f64);
        }
        // type E = OtherEnum;
        for b in self.tree.bases(cls) {
            if let Some(v) = self.enum_literal(b, name) {
                return Some(v);
            }
        }
        None
    }

    /// Constant `name` as seen from class `ctx`: its declaration (possibly inherited) and
    /// the outermost `extends B(name = value)` modifier on the way, with the class
    /// whose scope the modifier value is written in.
    fn find_constant(
        &self,
        ctx: ClassId,
        name: &str,
        depth: usize,
    ) -> Option<(ClassId, &'a ast::Component, Option<(&'a E, ClassId)>)> {
        if depth > 40 {
            return None;
        }
        let def = self.tree.def(ctx)?;
        if let Some(c) = def.components.get(name) {
            return Some((ctx, c, None));
        }
        for (i, b) in self.tree.bases_by_ext(ctx) {
            if let Some((owner, comp, inner)) = self.find_constant(b, name, depth + 1) {
                let here = def.extends[i]
                    .modifications
                    .iter()
                    .find_map(|m| match &m.expr {
                        E::Modification { target, value, .. }
                            if target.parts.len() == 1 && &*target.parts[0].ident.text == name =>
                        {
                            Some((&**value, ctx))
                        }
                        _ => None,
                    });
                return Some((owner, comp, here.or(inner)));
            }
        }
        None
    }

    /// Scope for an element `owner` declares but `ctx` inherits: names resolve in `ctx`
    /// first (its members carry its modifiers and redeclarations), then lexically
    /// around `owner`.
    fn inherited_env(
        &mut self,
        ctx: ClassId,
        owner: ClassId,
        overrides: Rc<FxHashMap<String, ClassId>>,
    ) -> Rc<Env> {
        let parent = Some(self.env_of(owner));
        Rc::new(Env {
            class: ctx,
            overrides,
            parent,
        })
    }

    fn class_constant_in(
        &mut self,
        ctx: ClassId,
        owner: ClassId,
        comp: &'a ast::Component,
        modv: Option<(&'a E, ClassId)>,
        env: Rc<Env>,
    ) -> Result<Val> {
        if ctx == owner && modv.is_none() {
            return self.class_constant(owner, comp, env);
        }
        let key = (ctx, comp.name.clone());
        if let Some(v) = self.const_cache.get(&key) {
            return Ok(v.clone());
        }
        if !self.const_in_progress.insert(key.clone()) {
            bail!("cyclic constant definition '{}'", comp.name);
        }
        let r = match modv {
            Some((value, mc)) => {
                // Written in `mc`, but names resolve in the class that inherits it first
                // (`substanceNames = {data.name}` sees the `data` its user supplies).
                let menv = if mc == ctx {
                    let menv = self.env_of(mc);
                    Rc::new(Env {
                        class: menv.class,
                        overrides: env.overrides.clone(),
                        parent: menv.parent.clone(),
                    })
                } else {
                    self.inherited_env(ctx, mc, env.overrides.clone())
                };
                self.translate(
                    value,
                    &Scope {
                        inst: None,
                        env: menv,
                    },
                    &mut Ctx::default(),
                )
            }
            None => match &comp.binding {
                Some(b) => self.translate(b, &Scope { inst: None, env }, &mut Ctx::default()),
                None => Err(anyhow!("constant '{}' has no value", comp.name)),
            },
        };
        self.const_in_progress.remove(&key);
        let v = r?;
        self.const_cache.insert(key, v.clone());
        Ok(v)
    }

    fn class_constant(
        &mut self,
        owner: ClassId,
        comp: &'a ast::Component,
        env: Rc<Env>,
    ) -> Result<Val> {
        let key = (owner, comp.name.clone());
        if let Some(v) = self.const_cache.get(&key) {
            return Ok(v.clone());
        }
        if !self.const_in_progress.insert(key.clone()) {
            bail!("cyclic constant definition '{}'", comp.name);
        }
        let scope = Scope { inst: None, env };
        let r = match &comp.binding {
            Some(b) => self.translate(b, &scope, &mut Ctx::default()),
            None => {
                // Look for a start modifier as value.
                let mut val = None;
                for m in &comp.source_modifications {
                    if let E::Modification { target, value, .. } = m
                        && target.parts.len() == 1
                        && &*target.parts[0].ident.text == "start"
                    {
                        val = Some(self.translate(value, &scope, &mut Ctx::default()));
                    }
                }
                match val {
                    Some(v) => v,
                    None => self.record_constant_from_mods(comp, &scope, &key),
                }
            }
        };
        self.const_in_progress.remove(&key);
        let v = r?;
        self.const_cache.insert(key, v.clone());
        Ok(v)
    }

    /// A record constant given by modifiers (`constant DataRecord N2(name = "N2", ..)`).
    fn record_constant_from_mods(
        &mut self,
        comp: &'a ast::Component,
        scope: &Scope,
        key: &(ClassId, String),
    ) -> Result<Val> {
        let tparts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
        let rec = if tparts.len() == 1 && builtin_name(tparts[0]).is_some() {
            None
        } else {
            self.lookup_class(&scope.env, &tparts)
                .filter(|&k| matches!(self.tree.class_type(k), ClassType::Record))
        };
        let Some(rec) = rec else {
            bail!("constant '{}' has no value", comp.name);
        };
        // An array of records is stored field by field; `each` values are repeated.
        let mut dims = Vec::new();
        for sub in &comp.shape_expr {
            match sub {
                ast::Subscript::Expression(e) => {
                    dims.push(self.eval_structural(e, scope)? as usize)
                }
                _ => bail!("constant '{}' has no value", comp.name),
            }
        }
        let n: usize = dims.iter().product();
        let mut nargv = Vec::new();
        for (i, m) in comp.source_modifications.iter().enumerate() {
            if let E::Modification { target, value, .. } = m
                && target.parts.len() == 1
            {
                let each = comp
                    .source_modification_each_flags
                    .get(i)
                    .copied()
                    .unwrap_or(false);
                // Later fields may refer to earlier ones (`R_s = R_NASA_2002/N2.MM`).
                let partial = Val::Rec(nargv.clone());
                self.const_cache.insert(key.clone(), partial);
                let v = self.translate(value, scope, &mut Ctx::default());
                self.const_cache.remove(key);
                let v = match v? {
                    Val::S(e) if each && !dims.is_empty() => Val::A(dims.clone(), vec![e; n]),
                    Val::Str(x) if each && !dims.is_empty() => Val::SA(dims.clone(), vec![x; n]),
                    v => v,
                };
                nargv.push((target.parts[0].ident.text.to_string(), v));
            }
        }
        self.record_constructor(rec, Vec::new(), nargv)
    }

    // ------------------------------------------------------------------
    // Function calls
    // ------------------------------------------------------------------

    fn call(
        &mut self,
        comp: &'a ast::ComponentReference,
        args: &'a [E],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        let name = cref_name(comp);
        let (pos, named) = split_args(args);
        macro_rules! arg {
            ($i:expr) => {
                self.translate(
                    pos.get($i)
                        .copied()
                        .ok_or_else(|| anyhow!("{}: missing argument {}", name, $i + 1))?,
                    scope,
                    ctx,
                )?
            };
        }
        let f1 = |n: &str| -> Option<Fn1> {
            Some(match n {
                "sin" => Fn1::Sin,
                "cos" => Fn1::Cos,
                "tan" => Fn1::Tan,
                "asin" => Fn1::Asin,
                "acos" => Fn1::Acos,
                "atan" => Fn1::Atan,
                "sinh" => Fn1::Sinh,
                "cosh" => Fn1::Cosh,
                "tanh" => Fn1::Tanh,
                "exp" => Fn1::Exp,
                "log" => Fn1::Log,
                "log10" => Fn1::Log10,
                "sqrt" => Fn1::Sqrt,
                "abs" => Fn1::Abs,
                "sign" => Fn1::Sign,
                "floor" => Fn1::Floor,
                "ceil" => Fn1::Ceil,
                "integer" => Fn1::Floor,
                _ => return None,
            })
        };
        if !name.contains('.')
            && let Some(r) = self.clock_call(&name, &pos, &named, scope, ctx)
        {
            return r;
        }
        let short = name.rsplit('.').next().unwrap_or(&name).to_string();
        let is_builtin_ns = !name.contains('.') || name.starts_with("Modelica.Math.");
        if is_builtin_ns && let Some(f) = f1(&short) {
            let v = arg!(0);
            if !Self::is_record_like(&v) {
                return self.map1(v, &mut |a, x| a.f1(f, x));
            }
            // A record argument: an imported function of that name (Modelica.ComplexMath.abs)
            // takes precedence; otherwise abs of a Complex is its modulus.
            let env = scope.env.clone();
            let user = self
                .lookup_class(&env, &[short.as_str()])
                .filter(|&c| matches!(self.tree.class_type(c), ClassType::Function));
            if user.is_none() {
                if short == "abs" {
                    let (re, im) = self.complex_parts(v)?;
                    return self.zip2(re, im, &mut |a, x, y| {
                        let xx = a.mul(x, x);
                        let yy = a.mul(y, y);
                        let s = a.add(xx, yy);
                        a.f1(Fn1::Sqrt, s)
                    });
                }
                return self.map1(v, &mut |a, x| a.f1(f, x));
            }
        }
        match name.as_str() {
            "der" => {
                let v = arg!(0);
                return self.map1(v, &mut |a, x| time_derivative(a, x));
            }
            "pre" => {
                let v = arg!(0);
                return self.map1(v, &mut |a, x| match a.node(x) {
                    Node::Var(v) => a.pre(v),
                    _ => x,
                });
            }
            "edge" => {
                let v = arg!(0);
                return self.map1(v, &mut |a, x| match a.node(x) {
                    Node::Var(v) => {
                        let p = a.pre(v);
                        let np = a.not(p);
                        a.f2(Fn2::And, x, np)
                    }
                    _ => x,
                });
            }
            "change" => {
                let v = arg!(0);
                return self.map1(v, &mut |a, x| match a.node(x) {
                    Node::Var(v) => {
                        let p = a.pre(v);
                        a.cmp(Cmp::Ne, x, p)
                    }
                    _ => Arena::ZERO,
                });
            }
            "initial" => return Ok(Val::S(self.model.arena.initial())),
            "terminal" => return Ok(Val::S(Arena::ZERO)),
            "noEvent" => {
                let saved = ctx.no_event;
                ctx.no_event = true;
                let r = self.translate(pos[0], scope, ctx);
                ctx.no_event = saved;
                return r;
            }
            "smooth" => {
                let order = self.translate(pos[0], scope, ctx)?.scalar()?;
                let o = self.eval_param_expr(order).unwrap_or(0.0);
                let saved = ctx.no_event;
                if o >= 1.0 {
                    ctx.no_event = true;
                }
                let r = self.translate(pos[1], scope, ctx);
                ctx.no_event = saved;
                return r;
            }
            "homotopy" => {
                let actual = named
                    .iter()
                    .find(|(n, _)| *n == "actual")
                    .map(|(_, e)| *e)
                    .or(pos.first().copied());
                let simplified = named
                    .iter()
                    .find(|(n, _)| *n == "simplified")
                    .map(|(_, e)| *e)
                    .or(pos.get(1).copied());
                let a = self.translate(
                    actual.ok_or_else(|| anyhow!("homotopy: missing actual"))?,
                    scope,
                    ctx,
                )?;
                let Some(simplified) = simplified else {
                    return Ok(a);
                };
                let s = self.translate(simplified, scope, ctx)?;
                // `lambda*actual + (1 - lambda)*simplified`; initialization walks lambda
                // from 0 to 1 (homotopy continuation), simulation runs at lambda = 1.
                let l = self.homotopy_lambda();
                return self.zip2(a, s, &mut |ar, x, y| {
                    if x == y {
                        return x;
                    }
                    let lx = ar.mul(l, x);
                    let ml = ar.sub(Arena::ONE, l);
                    let ly = ar.mul(ml, y);
                    ar.add(lx, ly)
                });
            }
            "delay" => {
                let x = arg!(0);
                let d = arg!(1).scalar()?;
                if ctx.in_function {
                    self.model.warnings.push(
                        "delay() in a function is approximated by its undelayed argument".into(),
                    );
                    return Ok(x);
                }
                let (dims, xs) = x.elems()?;
                let mut out = Vec::with_capacity(xs.len());
                for xe in xs {
                    if self.model.arena.as_const(xe).is_some() {
                        out.push(xe);
                        continue;
                    }
                    // the delayed signal becomes a hidden variable, so that the history
                    // buffer records it
                    let k = self.model.arena.delays.len();
                    let mut v =
                        Variable::new(format!("$delay{k}"), VarKind::Continuous, VarType::Real);
                    v.generated = true;
                    v.is_protected = true;
                    let id = self.model.add_var(v);
                    let lhs = self.model.arena.var(id);
                    let origin = self.model.origin(format!("delay({})", self.model.show(xe)));
                    self.model.equations.push(Equation {
                        lhs,
                        rhs: xe,
                        origin,
                    });
                    out.push(self.model.arena.delay(id, d));
                }
                return Ok(if dims.is_empty() {
                    Val::S(out[0])
                } else {
                    Val::A(dims, out)
                });
            }
            "sample" => {
                let start = arg!(0).scalar()?;
                let interval = arg!(1).scalar()?;
                return Ok(Val::S(self.sample(start, interval)));
            }
            "semiLinear" => {
                // Vectorised over the enthalpies / mass fractions (`port.mXi_flow`).
                let x = arg!(0).scalar()?;
                let k1 = arg!(1);
                let k2 = arg!(2);
                let r = self.zip2(k1.clone(), k2.clone(), &mut |a, k1, k2| {
                    let c = a.cmp(Cmp::Ge, x, Arena::ZERO);
                    let p1 = a.mul(k1, x);
                    let p2 = a.mul(k2, x);
                    a.if_(c, p1, p2)
                })?;
                if let (Val::S(res), Val::S(s), Val::S(h)) = (&r, &k1, &k2)
                    && let (Node::Var(xv), Node::Var(sv)) =
                        (self.model.arena.node(x), self.model.arena.node(*s))
                {
                    self.semi_linear.push((*res, xv, sv, *h));
                }
                return Ok(r);
            }
            "atan2" | "div" | "mod" | "rem" => {
                let f = match name.as_str() {
                    "atan2" => Fn2::Atan2,
                    "div" => Fn2::Div,
                    "mod" => Fn2::Mod,
                    _ => Fn2::Rem,
                };
                let a = arg!(0);
                let b = arg!(1);
                return self.zip2(a, b, &mut |ar, x, y| ar.f2(f, x, y));
            }
            "min" | "max" => {
                let f = if name == "min" { Fn2::Min } else { Fn2::Max };
                if pos.len() == 2 {
                    let a = arg!(0);
                    let b = arg!(1);
                    return self.zip2(a, b, &mut |ar, x, y| ar.f2(f, x, y));
                }
                let (_, e) = arg!(0).elems()?;
                let mut r = *e.first().ok_or_else(|| anyhow!("{name} of empty array"))?;
                for &x in &e[1..] {
                    r = self.model.arena.f2(f, r, x);
                }
                return Ok(Val::S(r));
            }
            "sum" | "product" => {
                let v = arg!(0);
                if name == "sum" && Self::is_record_like(&v) {
                    // sum of an array of records (Complex): field by field.
                    let Val::Rec(fields) = self.to_rec(v)? else {
                        unreachable!()
                    };
                    let mut out = Vec::new();
                    for (n, x) in fields {
                        let (_, e) = x.elems()?;
                        let mut r = Arena::ZERO;
                        for y in e {
                            r = self.model.arena.add(r, y);
                        }
                        out.push((n, Val::S(r)));
                    }
                    return Ok(Val::Rec(out));
                }
                let (_, e) = v.elems()?;
                let a = &mut self.model.arena;
                let mut r = if name == "sum" {
                    Arena::ZERO
                } else {
                    Arena::ONE
                };
                for x in e {
                    r = if name == "sum" {
                        a.add(r, x)
                    } else {
                        a.mul(r, x)
                    };
                }
                return Ok(Val::S(r));
            }
            "size" => {
                let v = arg!(0);
                let dims = match &v {
                    Val::A(d, _) | Val::IA(d, _) | Val::SA(d, _) => d.clone(),
                    Val::Rec(f) => match f.first() {
                        Some((_, Val::A(d, _) | Val::IA(d, _))) => d.clone(),
                        _ => vec![],
                    },
                    _ => vec![],
                };
                if pos.len() > 1 {
                    let k = self.translate(pos[1], scope, ctx)?.scalar()?;
                    let k = self.eval_param_expr(k).unwrap_or(1.0) as usize;
                    let d = dims.get(k.saturating_sub(1)).copied().unwrap_or(1);
                    return Ok(Val::S(self.model.arena.constant(d as f64)));
                }
                let v: Vec<ExprId> = dims
                    .iter()
                    .map(|&d| self.model.arena.constant(d as f64))
                    .collect();
                return Ok(Val::A(vec![v.len()], v));
            }
            "ndims" => {
                let v = arg!(0);
                let n = match &v {
                    Val::A(d, _) | Val::IA(d, _) | Val::SA(d, _) => d.len(),
                    _ => 0,
                };
                return Ok(Val::S(self.model.arena.constant(n as f64)));
            }
            "fill" if pos.len() > 1 => {
                let v = arg!(0);
                if Self::is_record_like(&v) {
                    let mut dims = Vec::new();
                    for p in &pos[1..] {
                        let d = self.translate(p, scope, ctx)?.scalar()?;
                        dims.push(
                            self.eval_param_expr(d)
                                .ok_or_else(|| anyhow!("fill: size must be a parameter"))?
                                as usize,
                        );
                    }
                    let n: usize = dims.iter().product();
                    return self.stack_vals(dims, vec![v; n]);
                }
                let mut dims = Vec::new();
                for p in &pos[1..] {
                    let d = self.translate(p, scope, ctx)?.scalar()?;
                    dims.push(
                        self.eval_param_expr(d)
                            .ok_or_else(|| anyhow!("fill: size must be a parameter"))?
                            as usize,
                    );
                }
                let n: usize = dims.iter().product();
                match v {
                    Val::Str(x) => return Ok(Val::SA(dims, vec![x; n])),
                    Val::SA(d, e) => {
                        let mut all = Vec::with_capacity(n * e.len());
                        for _ in 0..n {
                            all.extend(e.iter().cloned());
                        }
                        dims.extend(d);
                        return Ok(Val::SA(dims, all));
                    }
                    Val::A(d, e) => {
                        let mut all = Vec::with_capacity(n * e.len());
                        for _ in 0..n {
                            all.extend(e.iter().copied());
                        }
                        dims.extend(d);
                        return Ok(Val::A(dims, all));
                    }
                    v => {
                        let x = v.scalar()?;
                        return Ok(Val::A(dims, vec![x; n]));
                    }
                }
            }
            "zeros" | "ones" | "fill" => {
                let (val, start) = match name.as_str() {
                    "zeros" => (Arena::ZERO, 0),
                    "ones" => (Arena::ONE, 0),
                    _ => (arg!(0).scalar()?, 1),
                };
                let mut dims = Vec::new();
                for p in &pos[start..] {
                    let d = self.translate(p, scope, ctx)?.scalar()?;
                    dims.push(
                        self.eval_param_expr(d)
                            .ok_or_else(|| anyhow!("{name}: size must be a parameter"))?
                            as usize,
                    );
                }
                let n: usize = dims.iter().product();
                return Ok(Val::A(dims, vec![val; n]));
            }
            "identity" => {
                let n = arg!(0).scalar()?;
                let n = self.eval_param_expr(n).unwrap_or(0.0) as usize;
                let mut v = vec![Arena::ZERO; n * n];
                for i in 0..n {
                    v[i * n + i] = Arena::ONE;
                }
                return Ok(Val::A(vec![n, n], v));
            }
            "linspace" => {
                let a = arg!(0).scalar()?;
                let b = arg!(1).scalar()?;
                let n = arg!(2).scalar()?;
                let n = self.eval_param_expr(n).unwrap_or(2.0) as usize;
                let ar = &mut self.model.arena;
                let mut v = Vec::new();
                for i in 0..n {
                    let t = if n > 1 {
                        i as f64 / (n - 1) as f64
                    } else {
                        0.0
                    };
                    let ct = ar.constant(t);
                    let d = ar.sub(b, a);
                    let s = ar.mul(ct, d);
                    v.push(ar.add(a, s));
                }
                return Ok(Val::A(vec![n], v));
            }
            "transpose" => {
                let (d, e) = arg!(0).elems()?;
                if d.len() != 2 {
                    bail!("transpose expects a matrix");
                }
                let (r, c) = (d[0], d[1]);
                let mut v = Vec::with_capacity(r * c);
                for j in 0..c {
                    for i in 0..r {
                        v.push(e[i * c + j]);
                    }
                }
                return Ok(Val::A(vec![c, r], v));
            }
            "scalar" => {
                let (_, e) = arg!(0).elems()?;
                return Ok(Val::S(e[0]));
            }
            "diagonal" => {
                let (_, e) = arg!(0).elems()?;
                let n = e.len();
                let mut v = vec![Arena::ZERO; n * n];
                for (i, x) in e.into_iter().enumerate() {
                    v[i * n + i] = x;
                }
                return Ok(Val::A(vec![n, n], v));
            }
            "outerProduct" => {
                let (_, a) = arg!(0).elems()?;
                let (_, b) = arg!(1).elems()?;
                let ar = &mut self.model.arena;
                let mut v = Vec::with_capacity(a.len() * b.len());
                for &x in &a {
                    for &y in &b {
                        v.push(ar.mul(x, y));
                    }
                }
                return Ok(Val::A(vec![a.len(), b.len()], v));
            }
            "symmetric" => {
                let (d, e) = arg!(0).elems()?;
                if d.len() != 2 || d[0] != d[1] {
                    bail!("symmetric expects a square matrix");
                }
                let n = d[0];
                let v = (0..n * n)
                    .map(|k| {
                        let (i, j) = (k / n, k % n);
                        if i <= j { e[k] } else { e[j * n + i] }
                    })
                    .collect();
                return Ok(Val::A(d, v));
            }
            "skew" => {
                let (_, x) = arg!(0).elems()?;
                if x.len() != 3 {
                    bail!("skew expects a 3-vector");
                }
                let ar = &mut self.model.arena;
                let n = |ar: &mut Arena, e| ar.neg(e);
                let v = vec![
                    Arena::ZERO,
                    n(ar, x[2]),
                    x[1],
                    x[2],
                    Arena::ZERO,
                    n(ar, x[0]),
                    n(ar, x[1]),
                    x[0],
                    Arena::ZERO,
                ];
                return Ok(Val::A(vec![3, 3], v));
            }
            "matrix" => {
                let (d, e) = arg!(0).elems()?;
                let r = d.first().copied().unwrap_or(1);
                let c = d.get(1).copied().unwrap_or(1);
                let stride = (e.len() / (r * c).max(1)).max(1);
                let v: Vec<ExprId> = (0..r * c).map(|k| e[k * stride]).collect();
                return Ok(Val::A(vec![r, c], v));
            }
            "vector" => {
                let (_, e) = arg!(0).elems()?;
                let n = e.len();
                return Ok(Val::A(vec![n], e));
            }
            "cross" => {
                let (_, a) = arg!(0).elems()?;
                let (_, b) = arg!(1).elems()?;
                let ar = &mut self.model.arena;
                let mut c = |i: usize, j: usize| {
                    let p = ar.mul(a[i], b[j]);
                    let q = ar.mul(a[j], b[i]);
                    ar.sub(p, q)
                };
                let v = vec![c(1, 2), c(2, 0), c(0, 1)];
                return Ok(Val::A(vec![3], v));
            }
            "array" => {
                let mut vals = Vec::with_capacity(pos.len());
                for p in &pos {
                    vals.push(self.translate(p, scope, ctx)?);
                }
                return self.array_construct(vals, ArrayConstructor::Array);
            }
            "cat" => {
                let k = arg!(0).scalar()?;
                let k = self.eval_param_expr(k).unwrap_or(1.0) as usize;
                let mut vals = Vec::new();
                for p in &pos[1..] {
                    vals.push(self.translate(p, scope, ctx)?);
                }
                // cat(k, A, B, ...): concatenation along dimension k (row-major data).
                let mut parts = Vec::with_capacity(vals.len());
                for v in vals {
                    let (d, e) = v.elems()?;
                    parts.push((if d.is_empty() { vec![1] } else { d }, e));
                }
                let Some(first) = parts.first() else {
                    return Ok(Val::A(vec![0], Vec::new()));
                };
                let nd = first.0.len();
                if k == 0 || k > nd || parts.iter().any(|(d, _)| d.len() != nd) {
                    bail!("cat({k}, ...): arguments must have the same number of dimensions");
                }
                for (d, _) in &parts {
                    if (0..nd).any(|i| i != k - 1 && d[i] != first.0[i]) {
                        bail!("cat({k}, ...): sizes differ outside dimension {k}");
                    }
                }
                let outer: usize = first.0[..k - 1].iter().product();
                let inner: usize = first.0[k..].iter().product();
                let mut dims = first.0.clone();
                dims[k - 1] = parts.iter().map(|(d, _)| d[k - 1]).sum();
                let mut out = Vec::with_capacity(dims.iter().product());
                for o in 0..outer {
                    for (d, e) in &parts {
                        let slab = d[k - 1] * inner;
                        out.extend_from_slice(&e[o * slab..(o + 1) * slab]);
                    }
                }
                return Ok(Val::A(dims, out));
            }
            "Integer" | "Real" | "Boolean" => return Ok(arg!(0)),
            "String" => {
                // The text of a translation-time value; run-time values only reach
                // diagnostic messages, which are not evaluated.
                let v = match pos.first() {
                    Some(e) => self.translate(e, scope, ctx).ok(),
                    None => None,
                };
                let text = match v {
                    Some(Val::Str(x)) => x,
                    Some(Val::S(e)) => match self.eval_param_expr(e) {
                        Some(x) if x.fract() == 0.0 && x.abs() < 1e15 => format!("{}", x as i64),
                        Some(x) => format!("{x:.6}"),
                        None => String::new(),
                    },
                    _ => String::new(),
                };
                return Ok(Val::Str(text));
            }
            "assert" | "terminate" | "print" => return Ok(Val::S(Arena::ZERO)),
            "inStream" | "actualStream" => {
                let v = arg!(0);
                let (dims, xs) = v.elems()?;
                let mut out = Vec::with_capacity(xs.len());
                for x in xs {
                    let Node::Var(sv) = self.model.arena.node(x) else {
                        bail!("{name}() expects a stream variable of a connector");
                    };
                    if !self.stream_vars.contains(&sv) {
                        bail!(
                            "{name}(): '{}' is not a stream variable",
                            self.model.vars[sv as usize].name
                        );
                    }
                    let ins = self.in_stream_var(sv);
                    let r = if name == "inStream" {
                        self.model.arena.var(ins)
                    } else {
                        // actualStream(h) = if m_flow > 0 then inStream(h) else h
                        let f = self
                            .stream_flow(sv)
                            .ok_or_else(|| anyhow!("stream connector without flow variable"))?;
                        let a = &mut self.model.arena;
                        let fv = a.var(f);
                        let c = a.cmp(Cmp::Gt, fv, Arena::ZERO);
                        let iv = a.var(ins);
                        let sv = a.var(sv);
                        a.if_(c, iv, sv)
                    };
                    out.push(r);
                }
                return Ok(if dims.is_empty() {
                    Val::S(out[0])
                } else {
                    Val::A(dims, out)
                });
            }
            "cardinality" => {
                let arg = pos
                    .first()
                    .ok_or_else(|| anyhow!("cardinality() needs an argument"))?;
                let n = self.cardinality(arg, scope, ctx)?;
                return Ok(Val::S(self.model.arena.constant(n as f64)));
            }
            "getInstanceName" => return Ok(Val::Str(String::new())),
            "Connections.isRoot" | "Connections.rooted" => {
                self.oc_log.used = true;
                let node = match pos.first() {
                    Some(a) => self.oc_node(a, scope, ctx)?,
                    None => None,
                };
                let r = match (&self.oc_answers, node) {
                    (Some(ans), Some(n)) => {
                        if name == "Connections.isRoot" {
                            ans.is_root(&n)
                        } else {
                            ans.rooted(&n)
                        }
                    }
                    _ => false,
                };
                return Ok(Val::S(if r { Arena::ONE } else { Arena::ZERO }));
            }
            _ => {}
        }
        // Call of a functional input (`f(u)` where `f` was passed as `function g(a = 1)`).
        if comp.parts.len() == 1
            && ctx.in_function
            && let Some(Val::Func(fv)) = ctx
                .locals
                .iter()
                .rev()
                .find_map(|f| f.get(&*comp.parts[0].ident.text))
                .cloned()
        {
            let mut argv = Vec::new();
            for p in &pos {
                argv.push(self.translate(p, scope, ctx)?);
            }
            let mut nargv = Vec::new();
            for (n, e) in &named {
                nargv.push((n.to_string(), self.translate(e, scope, ctx)?));
            }
            for (n, v) in &fv.bound {
                if !nargv.iter().any(|(m, _)| m == n) {
                    nargv.push((n.clone(), v.clone()));
                }
            }
            let outs =
                self.inline_function_in(fv.cls, argv, nargv, fv.overrides.clone(), fv.penv.clone())?;
            return Ok(if outs.len() == 1 {
                outs.into_iter().next().unwrap()
            } else {
                Val::Tuple(outs)
            });
        }
        // User-defined function or record constructor.
        let parts: Vec<&str> = comp.parts.iter().map(|p| &*p.ident.text).collect();
        let env = if comp.local {
            self.env_of(ROOT)
        } else {
            scope.env.clone()
        };
        let mut redeclare_defaults: Vec<(String, Scoped<'a>)> = Vec::new();
        let (cls, mod_scope) = match self.lookup_class(&env, &parts) {
            Some(c) => {
                // `redeclare function f = g(a = ..)` on an enclosing instance.
                if parts.len() == 1 && !ctx.in_function && env.overrides.get(parts[0]) == Some(&c) {
                    let mut cur = scope.inst;
                    while let Some(i) = cur {
                        if let Some(ms) = self.class_mods.get(&(i, parts[0].to_string())) {
                            redeclare_defaults = ms.clone();
                            break;
                        }
                        cur = self.insts[i as usize].parent;
                    }
                }
                let penv = self.env_of(self.tree.node(c).parent.unwrap_or(ROOT));
                (
                    c,
                    Scope {
                        inst: None,
                        env: penv,
                    },
                )
            }
            None => {
                // A function declared in a component's class (`world.gravityAcceleration(r)`):
                // its short-class modifiers refer to that component's parameters.
                let found = if parts.len() > 1
                    && !ctx.in_function
                    && let Some(inst) = scope.inst
                {
                    self.component_function(inst, &parts)?
                } else {
                    None
                };
                let found = found.ok_or_else(|| anyhow!("function '{}' not found", name))?;
                if let Some(i) = found.1.inst
                    && let Some(ms) = self
                        .class_mods
                        .get(&(i, parts[parts.len() - 1].to_string()))
                {
                    redeclare_defaults = ms.clone();
                }
                found
            }
        };
        let mut argv = Vec::new();
        for p in &pos {
            argv.push(self.translate(p, scope, ctx)?);
        }
        let mut nargv = Vec::new();
        for (n, e) in &named {
            nargv.push((n.to_string(), self.translate(e, scope, ctx)?));
        }
        // Modifiers of the redeclaration that selected the function.
        for (n, v) in &redeclare_defaults {
            if !nargv.iter().any(|(m, _)| m == n) {
                let x = self.translate(v.expr, &v.scope, &mut Ctx::default())?;
                nargv.push((n.clone(), x));
            }
        }
        // Defaults from a short function definition `function f = g(a = ..)`.
        if let Some(def) = self.tree.def(cls) {
            for ext in &def.extends {
                for em in &ext.modifications {
                    if let E::Modification { target, value, .. } = &em.expr
                        && target.parts.len() == 1
                    {
                        let n = target.parts[0].ident.text.to_string();
                        if !nargv.iter().any(|(m, _)| *m == n) {
                            let v = self.translate(value, &mod_scope, &mut Ctx::default())?;
                            nargv.push((n, v));
                        }
                    }
                }
            }
        }
        match self.tree.class_type(cls) {
            ClassType::Function => {
                // A function reached through a package that inherits it (`Medium.f`) sees
                // that package's constants and redeclarations.
                let via = self.class_via(&env, &parts, cls);
                let lexical = self.tree.node(cls).parent;
                let penv = via.map(|v| {
                    let owner = lexical.unwrap_or(ROOT);
                    self.inherited_env(v, owner, env.overrides.clone())
                });
                // A function of scalars applied to arrays: element by element.
                if let Some(v) = self.vectorized_call(cls, &argv, &nargv, &penv, &env.overrides)? {
                    return Ok(v);
                }
                let outs =
                    self.inline_function_in(cls, argv, nargv, env.overrides.clone(), penv)?;
                if outs.len() == 1 {
                    Ok(outs.into_iter().next().unwrap())
                } else {
                    Ok(Val::Tuple(outs))
                }
            }
            ClassType::Record | ClassType::Operator => self.record_constructor(cls, argv, nargv),
            _ if self.tree.builtin_base(cls) == Some("ExternalObject") => {
                let ctor = self
                    .tree
                    .member_class(cls, "constructor")
                    .ok_or_else(|| anyhow!("external object '{}' has no constructor", name))?;
                let outs = self.inline_function(ctor, argv, nargv, env.overrides.clone())?;
                outs.into_iter()
                    .next()
                    .ok_or_else(|| anyhow!("constructor of '{}' returned nothing", name))
            }
            _ => bail!("'{}' is not a function", name),
        }
    }

    /// `function f(a = 1)`: a function value with some inputs bound. A functional input
    /// passed on (`function f()` where `f` is itself a function value) keeps its bindings.
    fn function_value(
        &mut self,
        comp: &'a ast::ComponentReference,
        args: &'a [E],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        let (_, named) = split_args(args);
        let mut bound = Vec::new();
        for (n, e) in &named {
            bound.push((n.to_string(), self.translate(e, scope, ctx)?));
        }
        if comp.parts.len() == 1
            && let Some(Val::Func(fv)) = ctx
                .locals
                .iter()
                .rev()
                .find_map(|f| f.get(&*comp.parts[0].ident.text))
                .cloned()
        {
            for (n, v) in &fv.bound {
                if !bound.iter().any(|(m, _)| m == n) {
                    bound.push((n.clone(), v.clone()));
                }
            }
            return Ok(Val::Func(Rc::new(FuncVal {
                cls: fv.cls,
                overrides: fv.overrides.clone(),
                penv: fv.penv.clone(),
                bound,
            })));
        }
        let parts: Vec<&str> = comp.parts.iter().map(|p| &*p.ident.text).collect();
        let env = if comp.local {
            self.env_of(ROOT)
        } else {
            scope.env.clone()
        };
        let name = cref_name(comp);
        let cls = self
            .lookup_class(&env, &parts)
            .filter(|&c| matches!(self.tree.class_type(c), ClassType::Function))
            .ok_or_else(|| anyhow!("function '{name}' not found"))?;
        let via = self.class_via(&env, &parts, cls);
        let lexical = self.tree.node(cls).parent;
        let penv = via.map(|v| {
            let owner = lexical.unwrap_or(ROOT);
            self.inherited_env(v, owner, env.overrides.clone())
        });
        Ok(Val::Func(Rc::new(FuncVal {
            cls,
            overrides: env.overrides.clone(),
            penv,
            bound,
        })))
    }

    /// Whether a type class carries array dimensions (`type Orientation = Real[3, 3]`).
    fn type_has_dims(&self, t: ClassId, depth: usize) -> bool {
        if depth > 16 {
            return false;
        }
        if self
            .tree
            .def(t)
            .is_some_and(|d| !d.array_subscripts.is_empty())
        {
            return true;
        }
        self.tree
            .bases(t)
            .into_iter()
            .any(|b| self.type_has_dims(b, depth + 1))
    }

    /// A call of a function with scalar inputs that passes arrays to some of them is
    /// applied element-wise (Modelica's function vectorization).
    fn vectorized_call(
        &mut self,
        cls: ClassId,
        argv: &[Val],
        nargv: &[(String, Val)],
        penv: &Option<Rc<Env>>,
        overrides: &Rc<FxHashMap<String, ClassId>>,
    ) -> Result<Option<Val>> {
        let is_arr = |v: &Val| matches!(v, Val::A(d, _) if !d.is_empty());
        if !argv.iter().chain(nargv.iter().map(|x| &x.1)).any(is_arr) {
            return Ok(None);
        }
        let mut elems = Vec::new();
        self.function_elements(cls, &mut elems, 0);
        let inputs: Vec<(String, &'a ast::Component, ClassId)> = elems
            .into_iter()
            .filter(|(_, c, _)| matches!(c.causality, Causality::Input(_)))
            .collect();
        let scalar_formal = |this: &mut Self, name: &str| -> bool {
            let Some((_, c, owner)) = inputs.iter().find(|(n, _, _)| n == name) else {
                return false;
            };
            if !c.shape_expr.is_empty() {
                return false;
            }
            let tparts: Vec<&str> = c.type_name.name.iter().map(|t| &*t.text).collect();
            if tparts.len() == 1 && builtin_name(tparts[0]).is_some() {
                return true;
            }
            let env = this.env_of(*owner);
            match this.lookup_class(&env, &tparts) {
                Some(t) => this.tree.builtin_base(t).is_some() && !this.type_has_dims(t, 0),
                None => false,
            }
        };
        let mut pos_vec = Vec::with_capacity(argv.len());
        let mut dims: Option<Vec<usize>> = None;
        for (i, v) in argv.iter().enumerate() {
            let name = inputs.get(i).map(|x| x.0.clone()).unwrap_or_default();
            let vec = is_arr(v) && scalar_formal(self, &name);
            if vec {
                let Val::A(d, _) = v else { unreachable!() };
                if dims.as_ref().is_some_and(|x| x != d) {
                    return Ok(None);
                }
                dims = Some(d.clone());
            }
            pos_vec.push(vec);
        }
        let mut named_vec = Vec::with_capacity(nargv.len());
        for (n, v) in nargv {
            let vec = is_arr(v) && scalar_formal(self, n);
            if vec {
                let Val::A(d, _) = v else { unreachable!() };
                if dims.as_ref().is_some_and(|x| x != d) {
                    return Ok(None);
                }
                dims = Some(d.clone());
            }
            named_vec.push(vec);
        }
        let Some(dims) = dims else { return Ok(None) };
        let n: usize = dims.iter().product();
        let pick = |v: &Val, k: usize| match v {
            Val::A(_, e) => Val::S(e[k]),
            _ => v.clone(),
        };
        let mut out = Vec::with_capacity(n);
        // Record results (Complex): an array of records is stored field by field.
        let mut rec_fields: Vec<(String, Vec<ExprId>)> = Vec::new();
        for k in 0..n {
            let a: Vec<Val> = argv
                .iter()
                .zip(&pos_vec)
                .map(|(v, &vec)| if vec { pick(v, k) } else { v.clone() })
                .collect();
            let na: Vec<(String, Val)> = nargv
                .iter()
                .zip(&named_vec)
                .map(|((m, v), &vec)| (m.clone(), if vec { pick(v, k) } else { v.clone() }))
                .collect();
            let outs = self.inline_function_in(cls, a, na, overrides.clone(), penv.clone())?;
            let first = outs
                .into_iter()
                .next()
                .ok_or_else(|| anyhow!("vectorized call of a function without outputs"))?;
            match first {
                Val::S(x) if rec_fields.is_empty() => out.push(x),
                Val::Rec(fs) if out.is_empty() => {
                    if k == 0 {
                        rec_fields = fs.iter().map(|(f, _)| (f.clone(), Vec::new())).collect();
                    }
                    if fs.len() != rec_fields.len() {
                        return Ok(None);
                    }
                    for ((f, v), (g, col)) in fs.iter().zip(rec_fields.iter_mut()) {
                        let Val::S(x) = v else { return Ok(None) };
                        if f != g {
                            return Ok(None);
                        }
                        col.push(*x);
                    }
                }
                // array results are left to the general call path
                _ => return Ok(None),
            }
        }
        if !rec_fields.is_empty() {
            return Ok(Some(Val::Rec(
                rec_fields
                    .into_iter()
                    .map(|(f, col)| (f, Val::A(dims.clone(), col)))
                    .collect(),
            )));
        }
        Ok(Some(Val::A(dims, out)))
    }

    /// Resolve `a.b.f` where `a.b` is a component and `f` a function class of its type.
    fn component_function(
        &mut self,
        inst: InstId,
        parts: &[&str],
    ) -> Result<Option<(ClassId, Scope)>> {
        self.ensure_component(inst, parts[0])?;
        let Some(mut cur) = self.insts[inst as usize].comps.get(parts[0]).cloned() else {
            return Ok(None);
        };
        for p in &parts[1..parts.len() - 1] {
            cur = match self.child_comp(cur, p) {
                Ok(c) => c,
                Err(_) => return Ok(None),
            };
        }
        let Comp::Inst(i) = cur else { return Ok(None) };
        let class = self.insts[i as usize].class;
        let fname = parts[parts.len() - 1];
        let redeclared = self.insts[i as usize].env.overrides.get(fname).copied();
        let Some(f) = redeclared.or_else(|| self.tree.member_class(class, fname)) else {
            return Ok(None);
        };
        let env = self.env_of(class);
        Ok(Some((f, Scope { inst: Some(i), env })))
    }

    fn sample(&mut self, start: ExprId, interval: ExprId) -> ExprId {
        // sample(start, interval): tick counter `n`, condition `time >= start + n*interval`.
        // Computing the instant from the counter avoids accumulating round-off.
        let k = self.sample_count;
        self.sample_count += 1;
        let mut v = Variable::new(format!("$sample{k}.n"), VarKind::Discrete, VarType::Real);
        v.generated = true;
        v.start = Some(Arena::ZERO);
        v.is_protected = true;
        let id = self.model.add_var(v);
        let mut nv = Variable::new(format!("$sample{k}.next"), VarKind::Discrete, VarType::Real);
        nv.generated = true;
        nv.is_protected = true;
        let a = &mut self.model.arena;
        let n = a.var(id);
        let ni = a.mul(n, interval);
        let next = a.add(start, ni);
        nv.start = Some(start);
        let next_id = self.model.add_var(nv);
        let a = &mut self.model.arena;
        let t = a.time();
        let nx = a.var(next_id);
        let cond = a.relation(Cmp::Ge, t, nx);
        let p = a.pre(id);
        let upd = a.add(p, Arena::ONE);
        let pi = a.mul(upd, interval);
        let upd_next = a.add(start, pi);
        let o = self.model.origin("sample()");
        self.model.whens.push(WhenClause {
            branches: vec![(
                cond,
                vec![
                    WhenAction::Assign(id, upd),
                    WhenAction::Assign(next_id, upd_next),
                ],
            )],
            origin: o,
        });
        self.when_assigned.insert(id);
        self.when_assigned.insert(next_id);
        let _ = next;
        cond
    }

    /// A record value of class `cls` with all numeric fields zero (`dims` for arrays of records).
    fn zero_record(&mut self, cls: ClassId, dims: &[usize], depth: usize) -> Result<Val> {
        if depth > 8 {
            bail!("record nesting too deep");
        }
        let mut names = Vec::new();
        self.function_elements(cls, &mut names, 0);
        let mut fields = Vec::new();
        for (n, c, owner) in names {
            if matches!(c.variability, Variability::Constant(_)) {
                continue;
            }
            // A field with a default (its binding, or `extends R(field = ..)`) starts with it.
            if dims.is_empty()
                && let Some((fo, fc, modv)) = self.find_constant(cls, &n, 0)
                && (fc.binding.is_some() || modv.is_some())
            {
                let fenv = if fo == cls {
                    self.env_of(fo)
                } else {
                    self.inherited_env(cls, fo, Rc::new(FxHashMap::default()))
                };
                if let Ok(v) = self.class_constant_in(cls, fo, fc, modv, fenv) {
                    fields.push((n, v));
                    continue;
                }
            }
            let env = self.env_of(owner);
            let sc = Scope {
                inst: None,
                env: env.clone(),
            };
            let mut d = dims.to_vec();
            for sub in &c.shape_expr {
                if let ast::Subscript::Expression(e) = sub {
                    d.push(self.eval_structural(e, &sc).unwrap_or(0.0) as usize);
                }
            }
            let tparts: Vec<&str> = c.type_name.name.iter().map(|t| &*t.text).collect();
            let sub_rec = if tparts.len() == 1 && builtin_name(tparts[0]).is_some() {
                None
            } else {
                self.lookup_class(&env, &tparts).filter(|&k| {
                    matches!(
                        self.tree.class_type(k),
                        ClassType::Record | ClassType::Operator
                    )
                })
            };
            let v = match sub_rec {
                Some(k) => self.zero_record(k, &d, depth + 1)?,
                None if d.is_empty() => Val::S(Arena::ZERO),
                None => {
                    let n: usize = d.iter().product();
                    Val::A(d, vec![Arena::ZERO; n])
                }
            };
            fields.push((n, v));
        }
        Ok(Val::Rec(fields))
    }

    fn record_constructor(
        &mut self,
        cls: ClassId,
        argv: Vec<Val>,
        nargv: Vec<(String, Val)>,
    ) -> Result<Val> {
        let mut fields = Vec::new();
        let mut names = Vec::new();
        self.function_elements(cls, &mut names, 0);
        let mut pi = 0;
        for (fname, comp, owner) in names {
            let val = if let Some((_, v)) = nargv.iter().find(|(n, _)| *n == fname) {
                v.clone()
            } else if pi < argv.len() && !matches!(comp.variability, Variability::Constant(_)) {
                pi += 1;
                argv[pi - 1].clone()
            } else if let Some(b) = &comp.binding {
                let env = self.env_of(owner);
                let mut ctx = Ctx {
                    in_function: true,
                    ..Default::default()
                };
                ctx.locals.push(fields.iter().cloned().collect());
                self.translate(b, &Scope { inst: None, env }, &mut ctx)?
            } else {
                Val::S(Arena::ZERO)
            };
            fields.push((fname, val));
        }
        Ok(Val::Rec(fields))
    }

    fn function_elements(
        &self,
        cls: ClassId,
        out: &mut Vec<(String, &'a ast::Component, ClassId)>,
        depth: usize,
    ) {
        if depth > 16 {
            return;
        }
        for b in self.tree.bases(cls) {
            self.function_elements(b, out, depth + 1);
        }
        if let Some(d) = self.tree.def(cls) {
            for (n, c) in &d.components {
                if !out.iter().any(|(m, _, _)| m == n) {
                    out.push((n.clone(), c, cls));
                }
            }
        }
    }

    fn function_algorithms(
        &self,
        cls: ClassId,
        out: &mut Vec<(&'a [ast::Statement], ClassId)>,
        depth: usize,
    ) {
        if depth > 16 {
            return;
        }
        for b in self.tree.bases(cls) {
            self.function_algorithms(b, out, depth + 1);
        }
        if let Some(d) = self.tree.def(cls) {
            for a in &d.algorithms {
                out.push((a.as_slice(), cls));
            }
        }
    }

    fn function_external(
        &self,
        cls: ClassId,
        depth: usize,
    ) -> Option<(&'a ast::ExternalFunction, String)> {
        if depth > 16 {
            return None;
        }
        let d = self.tree.def(cls)?;
        if let Some(e) = &d.external {
            return Some((e, self.tree.node(cls).name.clone()));
        }
        for b in self.tree.bases(cls) {
            if let Some(r) = self.function_external(b, depth + 1) {
                return Some(r);
            }
        }
        None
    }

    /// Inline a function call by symbolic execution of its algorithm.
    fn inline_function(
        &mut self,
        cls: ClassId,
        argv: Vec<Val>,
        nargv: Vec<(String, Val)>,
        overrides: Rc<FxHashMap<String, ClassId>>,
    ) -> Result<Vec<Val>> {
        self.inline_function_in(cls, argv, nargv, overrides, None)
    }

    /// Scope for the elements of function class `owner` (the called function or one it
    /// extends). Reached through package `via`, names resolve in `via` before `owner`'s
    /// lexical scope.
    fn function_owner_env(
        &mut self,
        owner: ClassId,
        via: Option<ClassId>,
        overrides: &Rc<FxHashMap<String, ClassId>>,
    ) -> Rc<Env> {
        match via {
            Some(v) => {
                let lexical = self.tree.node(owner).parent.unwrap_or(ROOT);
                let p = self.inherited_env(v, lexical, overrides.clone());
                Rc::new(Env {
                    class: owner,
                    overrides: overrides.clone(),
                    parent: Some(p),
                })
            }
            None => self.env_of(owner),
        }
    }

    /// Inline a call; `parent` replaces the function's lexical enclosing scope.
    fn inline_function_in(
        &mut self,
        cls: ClassId,
        argv: Vec<Val>,
        nargv: Vec<(String, Val)>,
        overrides: Rc<FxHashMap<String, ClassId>>,
        parent: Option<Rc<Env>>,
    ) -> Result<Vec<Val>> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            bail!("function inlining depth exceeded (recursive function?)");
        }
        let fname = self.tree.node(cls).path.clone();
        let mut elems = Vec::new();
        self.function_elements(cls, &mut elems, 0);
        let via = parent.as_ref().map(|p| p.class);
        let env = match parent {
            Some(p) => Rc::new(Env {
                class: cls,
                overrides: overrides.clone(),
                parent: Some(p),
            }),
            None => self.env_with(cls, overrides.clone()),
        };
        let scope = Scope { inst: None, env };
        let mut ctx = Ctx {
            in_function: true,
            ..Default::default()
        };
        ctx.locals.push(FxHashMap::default());
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();
        let mut pi = 0;
        // Bind inputs (positional, then named, then defaults).
        for (n, c, _) in &elems {
            if matches!(c.causality, Causality::Input(_)) {
                inputs.push(n.clone());
                let v = if let Some((_, v)) = nargv.iter().find(|(m, _)| m == n) {
                    Some(v.clone())
                } else if pi < argv.len() {
                    pi += 1;
                    Some(argv[pi - 1].clone())
                } else {
                    None
                };
                let v = match v {
                    Some(v) => v,
                    None => match &c.binding {
                        Some(b) => self.translate(b, &scope, &mut ctx)?,
                        None => bail!("function '{}': missing input '{}'", fname, n),
                    },
                };
                // A record argument passed as a component (`f(frame.R)`) becomes a record value.
                let v = if matches!(v, Val::Inst(_)) {
                    self.as_record(&v).unwrap_or(v)
                } else {
                    v
                };
                ctx.locals.last_mut().unwrap().insert(n.clone(), v);
            }
        }
        // A Modelica function this compiler implements natively:
        // annotation(__Daedalus_builtin = "interpolate").
        if let Some(op) = self
            .tree
            .def(cls)
            .and_then(|d| builtin_annotation(&d.annotation))
        {
            let ins: Vec<Val> = inputs.iter().map(|n| ctx.locals[0][n].clone()).collect();
            self.depth -= 1;
            return self
                .intrinsic(&op, ins)
                .with_context(|| format!("in builtin function '{fname}'"));
        }
        // External builtin intrinsics.
        if let Some((ext, short)) = self.function_external(cls, 0) {
            let lang = ext.language.clone().unwrap_or_default();
            let ins: Vec<Val> = inputs.iter().map(|n| ctx.locals[0][n].clone()).collect();
            self.depth -= 1;
            if lang.trim_matches('"') == "builtin" {
                return self
                    .intrinsic(&short, ins)
                    .with_context(|| format!("in builtin function '{fname}'"));
            }
            let cname = ext
                .function_name
                .as_ref()
                .map(|t| t.text.to_string())
                .unwrap_or_default();
            if let Some(op) = cname.strip_prefix("ModelicaStandardTables_") {
                self.depth += 1;
                let mut args = Vec::with_capacity(ext.args.len());
                for a in &ext.args {
                    args.push(self.translate(a, &scope, &mut ctx)?);
                }
                self.depth -= 1;
                return self
                    .standard_table(op, args)
                    .with_context(|| format!("in function '{fname}'"));
            }
            if let Some(op) = cname.strip_prefix("ModelicaStrings_") {
                self.depth += 1;
                let mut args = Vec::with_capacity(ext.args.len());
                for a in &ext.args {
                    args.push(self.translate(a, &scope, &mut ctx)?);
                }
                self.depth -= 1;
                return self
                    .strings(op, args)
                    .with_context(|| format!("in function '{fname}'"));
            }
            if matches!(cname.as_str(), "dgesv" | "dgesv_vec")
                || matches!(short.as_str(), "dgesv" | "dgesv_vec")
            {
                let ins: Vec<Val> = inputs.iter().map(|n| ctx.locals[0][n].clone()).collect();
                return self
                    .lapack_solve(ins)
                    .with_context(|| format!("in function '{fname}'"));
            }
            bail!("external function '{}' is not supported", fname);
        }
        // A large function called with run-time arguments is compiled once and called,
        // instead of being inlined at every call (property functions of media).
        if !self.outlining.contains(&cls) {
            let ins: Vec<Val> = inputs.iter().map(|n| ctx.locals[0][n].clone()).collect();
            if !self.outline_failed.contains(&cls) && self.should_outline(cls, &ins) {
                // Not every function compiles on its own (recursion on a structural
                // input, references to model variables): those are inlined after all.
                let saved = self.depth;
                self.depth -= 1;
                match self.outlined_call(cls, ins, overrides.clone(), via) {
                    Ok(v) => return Ok(v),
                    Err(_) => {
                        self.depth = saved;
                        self.outline_failed.insert(cls);
                    }
                }
            }
        }
        // Outputs and protected locals: dims + bindings.
        for (n, c, owner) in &elems {
            if matches!(c.causality, Causality::Input(_)) {
                continue;
            }
            if matches!(c.causality, Causality::Output(_)) {
                outputs.push(n.clone());
            }
            let oenv = self.function_owner_env(*owner, via, &overrides);
            let oscope = Scope {
                inst: None,
                env: oenv,
            };
            let mut dims = Vec::new();
            for s in &c.shape_expr {
                if let ast::Subscript::Expression(e) = s {
                    let d = self.translate(e, &oscope, &mut ctx)?.scalar()?;
                    dims.push(
                        self.eval_param_expr(d)
                            .ok_or_else(|| anyhow!("function array size must be known"))?
                            as usize,
                    );
                }
            }
            let tparts: Vec<&str> = c.type_name.name.iter().map(|t| &*t.text).collect();
            let rcls = if tparts.len() == 1 && builtin_name(tparts[0]).is_some() {
                None
            } else {
                self.lookup_class(&oscope.env, &tparts).filter(|&k| {
                    matches!(
                        self.tree.class_type(k),
                        ClassType::Record | ClassType::Operator
                    )
                })
            };
            let init = if let Some(b) = &c.binding {
                self.translate(b, &oscope, &mut ctx)?
            } else if let Some(rc) = rcls {
                // Record-typed output or local: zero fields, then its own modifiers.
                let mut r = self.zero_record(rc, &dims, 0)?;
                if let Val::Rec(fields) = &mut r {
                    for m in &c.source_modifications {
                        if let E::Modification { target, value, .. } = m
                            && target.parts.len() == 1
                        {
                            let n = &*target.parts[0].ident.text;
                            let v = self.translate(value, &oscope, &mut ctx)?;
                            if let Some(f) = fields.iter_mut().find(|(k, _)| k == n) {
                                f.1 = v;
                            }
                        }
                    }
                }
                r
            } else if dims.is_empty() {
                Val::S(Arena::ZERO)
            } else {
                let n: usize = dims.iter().product();
                Val::A(dims, vec![Arena::ZERO; n])
            };
            ctx.locals.last_mut().unwrap().insert(n.clone(), init);
        }
        let mut algs = Vec::new();
        self.function_algorithms(cls, &mut algs, 0);
        let mut returned = false;
        for (stmts, owner) in algs {
            let oenv = self.function_owner_env(owner, via, &overrides);
            let oscope = Scope {
                inst: None,
                env: oenv,
            };
            self.exec(stmts, &oscope, &mut ctx, &mut returned)
                .with_context(|| format!("in function '{fname}'"))?;
        }
        self.depth -= 1;
        Ok(outputs.iter().map(|n| ctx.locals[0][n].clone()).collect())
    }

    /// Native implementation of the `ModelicaStandardTables` C API behind
    /// `CombiTable1D*` and `CombiTimeTable`. A table object is an integer handle into
    /// `ext_tables`; lookups become [`Node::Interp`] nodes on the arena table.
    fn standard_table(&mut self, op: &str, args: Vec<Val>) -> Result<Vec<Val>> {
        let num = |f: &mut Self, v: &Val, what: &str| -> Result<f64> {
            let e = v.scalar()?;
            f.eval_param_expr(e)
                .ok_or_else(|| anyhow!("{what} must be a parameter expression"))
        };
        let handle = |f: &mut Self, v: &Val| -> Result<(u32, Vec<usize>)> {
            let e = v.scalar()?;
            if let Node::Var(var) = f.model.arena.node(e) {
                // Surface the constructor's error instead of "must be a parameter".
                f.ensure_binding(var).with_context(|| {
                    format!("in table object '{}'", f.model.vars[var as usize].name)
                })?;
            }
            let k = num(f, v, "table object")? as usize;
            f.ext_tables
                .get(k)
                .cloned()
                .ok_or_else(|| anyhow!("invalid table object"))
        };
        let column = |f: &mut Self, cols: &[usize], v: &Val| -> Result<u32> {
            let i = num(f, v, "table column")? as usize;
            let c = *cols
                .get(i.wrapping_sub(1))
                .ok_or_else(|| anyhow!("table column {i} out of range"))?;
            Ok(c.saturating_sub(1) as u32)
        };
        let (kind, fun) = op
            .split_once('_')
            .ok_or_else(|| anyhow!("unknown table function '{op}'"))?;
        let time = kind == "CombiTimeTable";
        if kind == "CombiTable2D" {
            bail!("2-D tables are not supported yet");
        }
        let out = match fun {
            "init2" | "init3" => {
                // (fileName, tableName, table, nrow, ncol, [startTime,] columns, ncols, smoothness,
                //  extrapolation, [shiftTime, timeEvents,] verboseRead, ...); nrow and ncol follow table
                let (d, e) = args[2].elems()?;
                if d.len() != 2 || d[0] == 0 {
                    bail!("tables read from files are not supported; give the table as a matrix");
                }
                let mut data = Vec::with_capacity(e.len());
                for v in e {
                    data.push(
                        self.eval_param_expr(v)
                            .ok_or_else(|| anyhow!("table data must be constant or parameter"))?,
                    );
                }
                let o = usize::from(time);
                let (_, ce) = args[5 + o].elems()?;
                let mut columns = Vec::with_capacity(ce.len());
                for v in ce {
                    columns.push(
                        self.eval_param_expr(v)
                            .ok_or_else(|| anyhow!("table columns must be parameters"))?
                            as usize,
                    );
                }
                let smooth = num(self, &args[7 + o], "smoothness")? as u8;
                let extrap = num(self, &args[8 + o], "extrapolation")? as u8;
                let mut t = Table::linear(
                    d[0],
                    d[1],
                    data,
                    match extrap {
                        2 => 1,
                        3 => 2,
                        _ => 0,
                    },
                );
                match smooth {
                    3 => t.smoothness = 1,
                    2 | 6 => t.make_cubic(false),
                    4 | 5 => t.make_cubic(true),
                    _ => {}
                }
                if time {
                    t.start = num(self, &args[5], "startTime")?;
                    t.shift = num(self, &args[9 + o], "shiftTime")?;
                }
                if columns.iter().any(|&c| c == 0 || c > t.cols) {
                    bail!(
                        "table columns {columns:?} out of range for a table with {} columns",
                        t.cols
                    );
                }
                let id = self.model.arena.add_table(t);
                self.ext_tables.push((id, columns));
                Val::S(
                    self.model
                        .arena
                        .constant((self.ext_tables.len() - 1) as f64),
                )
            }
            "getValue" => {
                let (t, cols) = handle(self, &args[0])?;
                let c = column(self, &cols, &args[1])?;
                let mut x = args[2].scalar()?;
                // (.., time, nextTimeEvent, pre(nextTimeEvent)): until the event at
                // nextTimeEvent is handled, the table stays on the segment left of it.
                if time && args.len() >= 4 {
                    let ne = args[3].scalar()?;
                    let a = &mut self.model.arena;
                    let an = a.f1(Fn1::Abs, ne);
                    let sc = a.f2(Fn2::Max, an, Arena::ONE);
                    let eps = a.constant(1e-13);
                    let d = a.mul(sc, eps);
                    let left = a.sub(ne, d);
                    let at = a.cmp(Cmp::Ge, x, ne);
                    x = a.if_(at, left, x);
                }
                Val::S(self.model.arena.interp(t, c, x))
            }
            "getDerValue" => {
                let (t, cols) = handle(self, &args[0])?;
                let c = column(self, &cols, &args[1])?;
                let x = args[2].scalar()?;
                let dx = args[if time { 5 } else { 3 }].scalar()?;
                let s = self.model.arena.interp_der(t, c, x);
                Val::S(self.model.arena.mul(s, dx))
            }
            "minimumTime" | "minimumAbscissa" | "maximumTime" | "maximumAbscissa" => {
                let (t, _) = handle(self, &args[0])?;
                let tb = &self.model.arena.tables[t as usize];
                let v = if fun.starts_with("min") {
                    tb.at(0, 0)
                } else {
                    tb.at(tb.rows - 1, 0)
                };
                Val::S(self.model.arena.constant(v))
            }
            "nextTimeEvent" => {
                let (t, _) = handle(self, &args[0])?;
                Val::S(
                    self.model
                        .arena
                        .interp(t, Table::NEXT_EVENT, args[1].scalar()?),
                )
            }
            "close" => Val::S(Arena::ZERO),
            _ => bail!("table function '{op}' is not supported"),
        };
        Ok(vec![out])
    }

    /// `Modelica.Utilities.Strings` on translation-time strings (the `ModelicaStrings` C API).
    fn strings(&mut self, op: &str, args: Vec<Val>) -> Result<Vec<Val>> {
        let text = |v: &Val| -> Result<String> {
            match v {
                Val::Str(s) => Ok(s.clone()),
                _ => bail!("expected a string, found {}", v.kind()),
            }
        };
        let mut int = |v: &Val| -> Result<i64> {
            let e = v.scalar()?;
            Ok(self
                .eval_param_expr(e)
                .ok_or_else(|| anyhow!("string index must be a parameter expression"))?
                as i64)
        };
        let chars: Vec<char> = text(&args[0])?.chars().collect();
        let n = chars.len() as i64;
        let r = match op {
            "length" => Val::S(self.model.arena.constant(n as f64)),
            "substring" => {
                let (a, b) = (int(&args[1])?.max(1), int(&args[2])?.min(n));
                let s: String = if b >= a {
                    chars[(a - 1) as usize..b as usize].iter().collect()
                } else {
                    String::new()
                };
                Val::Str(s)
            }
            "skipWhiteSpace" => {
                let mut i = int(&args[1])?.max(1);
                while i <= n && chars[(i - 1) as usize].is_whitespace() {
                    i += 1;
                }
                Val::S(self.model.arena.constant(i as f64))
            }
            "compare" => {
                let b = text(&args[1])?;
                let cs = int(&args[2])? != 0;
                let a: String = chars.iter().collect();
                let ord = if cs {
                    a.cmp(&b)
                } else {
                    a.to_lowercase().cmp(&b.to_lowercase())
                };
                let k = match ord {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => 2.0,
                    std::cmp::Ordering::Greater => 3.0,
                };
                Val::S(self.model.arena.constant(k))
            }
            _ => bail!("string function '{op}' is not supported"),
        };
        Ok(vec![r])
    }

    /// `(x, info) = dgesv(A, b)`: the solution of `A*x = b` (`b` a vector or a matrix).
    /// Constant systems are solved numerically with partial pivoting, others by symbolic
    /// elimination with pivots chosen at translation time.
    fn lapack_solve(&mut self, ins: Vec<Val>) -> Result<Vec<Val>> {
        let (da, a) = ins.first().ok_or_else(|| anyhow!("missing A"))?.elems()?;
        let (db, b) = ins.get(1).ok_or_else(|| anyhow!("missing b"))?.elems()?;
        if da.len() != 2 || da[0] != da[1] {
            bail!("A must be a square matrix");
        }
        let n = da[0];
        let m = if db.len() == 2 { db[1] } else { 1 };
        if db.first() != Some(&n) {
            bail!("b must have {n} rows");
        }
        let ar = &mut self.model.arena;
        let mut a: Vec<Vec<ExprId>> = (0..n).map(|i| a[i * n..(i + 1) * n].to_vec()).collect();
        let mut b: Vec<Vec<ExprId>> = (0..n).map(|i| b[i * m..(i + 1) * m].to_vec()).collect();
        let num = |ar: &Arena, e: ExprId| ar.as_const(e);
        for k in 0..n {
            // Pivot: the largest constant entry, else the first that is not structurally zero.
            let mut piv = None;
            let mut best = 0.0;
            for i in k..n {
                if let Some(v) = num(ar, a[i][k]) {
                    if v.abs() > best {
                        best = v.abs();
                        piv = Some(i);
                    }
                }
            }
            if piv.is_none() {
                piv = (k..n).find(|&i| !ar.is_zero(a[i][k]));
            }
            let Some(p) = piv else {
                bail!("singular matrix");
            };
            a.swap(k, p);
            b.swap(k, p);
            for i in k + 1..n {
                if ar.is_zero(a[i][k]) {
                    continue;
                }
                let f = ar.div(a[i][k], a[k][k]);
                for j in k..n {
                    let t = ar.mul(f, a[k][j]);
                    a[i][j] = ar.sub(a[i][j], t);
                }
                for j in 0..m {
                    let t = ar.mul(f, b[k][j]);
                    b[i][j] = ar.sub(b[i][j], t);
                }
            }
        }
        let mut x = vec![vec![Arena::ZERO; m]; n];
        for k in (0..n).rev() {
            for j in 0..m {
                let mut s = b[k][j];
                for l in k + 1..n {
                    let t = ar.mul(a[k][l], x[l][j]);
                    s = ar.sub(s, t);
                }
                x[k][j] = ar.div(s, a[k][k]);
            }
        }
        let flat: Vec<ExprId> = x.into_iter().flatten().collect();
        let xv = Val::A(db.clone(), flat);
        Ok(vec![xv, Val::S(Arena::ZERO)])
    }

    fn intrinsic(&mut self, name: &str, ins: Vec<Val>) -> Result<Vec<Val>> {
        match name {
            // interpolate(table[:,:], x, col) -> y   (linear, hold extrapolation)
            "interpolate" | "interpolateLinear" | "interpolateExtrapolate" => {
                let (d, e) = ins[0].elems()?;
                if d.len() != 2 {
                    bail!("interpolate expects a matrix table");
                }
                let x = ins[1].scalar()?;
                let col = match ins.get(2) {
                    Some(c) => self.eval_param_expr(c.scalar()?).unwrap_or(2.0) as usize,
                    None => 2,
                };
                let mut data = Vec::with_capacity(e.len());
                for &v in &e {
                    data.push(
                        self.eval_param_expr(v)
                            .ok_or_else(|| anyhow!("table data must be constant or parameter"))?,
                    );
                }
                let t = Table::linear(d[0], d[1], data, u8::from(name == "interpolateExtrapolate"));
                let id = self.model.arena.add_table(t);
                let col0 = col.saturating_sub(1) as u32;
                Ok(vec![Val::S(self.model.arena.interp(id, col0, x))])
            }
            _ => bail!("unknown builtin function '{name}'"),
        }
    }

    fn exec(
        &mut self,
        stmts: &'a [ast::Statement],
        scope: &Scope,
        ctx: &mut Ctx,
        returned: &mut bool,
    ) -> Result<()> {
        for s in stmts {
            if *returned || ctx.broke {
                return Ok(());
            }
            match s {
                ast::Statement::Empty => {}
                ast::Statement::Assignment { comp, value } => {
                    let v = self.translate(value, scope, ctx)?;
                    let guard = self.exit_guard(ctx);
                    let before = guard.map(|_| ctx.locals.clone());
                    self.assign_local(comp, v, scope, ctx)?;
                    if let (Some(g), Some(b)) = (guard, before) {
                        self.keep_if_exited(g, b, ctx)?;
                    }
                }
                ast::Statement::FunctionCall {
                    comp,
                    args,
                    outputs,
                } => {
                    let v = self.call(comp, args, scope, ctx)?;
                    let vals = match v {
                        Val::Tuple(t) => t,
                        other => vec![other],
                    };
                    let guard = self.exit_guard(ctx);
                    let before = guard.map(|_| ctx.locals.clone());
                    for (o, val) in outputs.iter().zip(vals) {
                        if let E::ComponentReference(cr) = o {
                            self.assign_local(cr, val, scope, ctx)?;
                        }
                    }
                    if let (Some(g), Some(b)) = (guard, before) {
                        self.keep_if_exited(g, b, ctx)?;
                    }
                }
                ast::Statement::If {
                    cond_blocks,
                    else_block,
                } => {
                    self.exec_if(cond_blocks, else_block.as_deref(), scope, ctx, returned)?;
                }
                ast::Statement::For { indices, equations } => {
                    let idx = indices
                        .first()
                        .ok_or_else(|| anyhow!("for without index"))?;
                    let range = self.translate(&idx.range, scope, ctx)?;
                    let (_, vals) = range.elems()?;
                    let name = idx.ident.text.to_string();
                    let outer_brk = ctx.sbrk.take();
                    for v in vals {
                        ctx.locals
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), Val::S(v));
                        self.exec(equations, scope, ctx, returned)?;
                        if *returned || ctx.broke {
                            break;
                        }
                    }
                    ctx.broke = false;
                    ctx.sbrk = outer_brk;
                    ctx.locals.last_mut().unwrap().remove(&name);
                }
                ast::Statement::While(block) => {
                    let mut n = 0;
                    let outer_brk = ctx.sbrk.take();
                    loop {
                        let c = self.translate(&block.cond, scope, ctx)?.scalar()?;
                        let Some(cv) = self.eval_param_expr(c) else {
                            // the condition depends on run-time values: a loop program
                            self.runtime_while(block, scope, ctx)?;
                            break;
                        };
                        if cv == 0.0 {
                            break;
                        }
                        self.exec(&block.stmts, scope, ctx, returned)?;
                        n += 1;
                        if n > 100_000 || *returned || ctx.broke {
                            break;
                        }
                        // A `break` under a run-time condition: once it holds for good the
                        // remaining iterations change nothing, but the loop still has to
                        // run to its translation-time end.
                    }
                    ctx.broke = false;
                    ctx.sbrk = outer_brk;
                }
                ast::Statement::Return { .. } => *returned = true,
                ast::Statement::Break { .. } => {
                    ctx.broke = true;
                    return Ok(());
                }
                ast::Statement::Assert { .. } => {}
                ast::Statement::When(_) | ast::Statement::Reinit { .. } => {
                    bail!("when-statements are not supported inside functions")
                }
            }
        }
        Ok(())
    }

    /// Whether a call of function `cls` with arguments `ins` is compiled as a callable
    /// instead of inlined: sizeable algorithmic functions with run-time arguments.
    fn should_outline(&mut self, cls: ClassId, ins: &[Val]) -> bool {
        if std::env::var_os("DAEDALUS_NO_OUTLINE").is_some() {
            return false;
        }
        let mut scalars = Vec::new();
        for v in ins {
            match v {
                Val::S(_) | Val::A(..) | Val::Rec(_) => flatten_scalars(v, &mut scalars),
                _ => return false,
            }
        }
        // Arguments fixed at translation time (parameters): inlining folds the body.
        if scalars.iter().all(|&e| self.eval_param_expr(e).is_some()) {
            return false;
        }
        if self
            .tree
            .def(cls)
            .is_some_and(|d| annotation_flag(&d.annotation, "Inline"))
        {
            return false;
        }
        let mut algs = Vec::new();
        self.function_algorithms(cls, &mut algs, 0);
        let size: usize = algs.iter().map(|(st, _)| stmt_weight(st)).sum();
        size >= 12
    }

    /// Call of function `cls` compiled once per argument shape: a one-shot loop program
    /// whose state is `[flag, inputs.., outputs..]`.
    fn outlined_call(
        &mut self,
        cls: ClassId,
        ins: Vec<Val>,
        overrides: Rc<FxHashMap<String, ClassId>>,
        via: Option<ClassId>,
    ) -> Result<Vec<Val>> {
        let mut args = Vec::new();
        for v in &ins {
            flatten_scalars(v, &mut args);
        }
        let shapes: Vec<String> = ins.iter().map(val_shape).collect();
        let mut ov: Vec<(String, ClassId)> =
            overrides.iter().map(|(k, v)| (k.clone(), *v)).collect();
        ov.sort();
        let key = format!("{cls}|{via:?}|{ov:?}|{}", shapes.join(","));
        let (pid, n_in, outs) = match self.outlined.get(&key) {
            Some(x) => x.clone(),
            None => {
                let built = self.build_outlined(cls, &ins, overrides, via)?;
                self.outlined.insert(key, built.clone());
                built
            }
        };
        let mut full = Vec::with_capacity(1 + n_in + 8);
        full.push(Arena::ONE);
        full.extend(args.iter().copied());
        let mut n_out = 0;
        for o in &outs {
            let mut f = Vec::new();
            flatten_scalars(o, &mut f);
            n_out += f.len();
        }
        full.extend(std::iter::repeat_n(Arena::ZERO, n_out));
        let mut k = 0u32;
        let mut res = Vec::with_capacity(outs.len());
        for o in &outs {
            let mut f = Vec::new();
            flatten_scalars(o, &mut f);
            let calls: Vec<ExprId> = (0..f.len())
                .map(|_| {
                    let e = self
                        .model
                        .arena
                        .call(pid, full.clone(), (1 + n_in) as u32 + k);
                    k += 1;
                    e
                })
                .collect();
            let mut it = calls.into_iter();
            res.push(rebuild_scalars(o, &mut it));
        }
        Ok(res)
    }

    fn build_outlined(
        &mut self,
        cls: ClassId,
        ins: &[Val],
        overrides: Rc<FxHashMap<String, ClassId>>,
        via: Option<ClassId>,
    ) -> Result<(u32, usize, Vec<Val>)> {
        let mut flat = Vec::new();
        for v in ins {
            flatten_scalars(v, &mut flat);
        }
        let n_in = flat.len() as u32;
        let base = self.next_local;
        self.next_local += n_in;
        let mut k = 0u32;
        let mut sym = Vec::with_capacity(ins.len());
        for v in ins {
            let mut f = Vec::new();
            flatten_scalars(v, &mut f);
            let ls: Vec<ExprId> = f
                .iter()
                .map(|_| {
                    let e = self.model.arena.local(base + k);
                    k += 1;
                    e
                })
                .collect();
            let mut it = ls.into_iter();
            sym.push(rebuild_scalars(v, &mut it));
        }
        let parent = via.map(|c| self.env_of(c));
        self.outlining.insert(cls);
        let r = self.inline_function_in(cls, sym, Vec::new(), overrides, parent);
        self.outlining.remove(&cls);
        let outs = r?;
        let mut out_exprs = Vec::new();
        for o in &outs {
            match o {
                Val::S(_) | Val::A(..) | Val::Rec(_) | Val::Tuple(_) => {
                    flatten_scalars(o, &mut out_exprs)
                }
                _ => bail!("function output of kind {} cannot be compiled", o.kind()),
            }
        }
        let n_out = out_exprs.len() as u32;
        let mut memo = FxHashMap::default();
        let mut bad = None;
        let mut map_leaf = |a: &mut Arena, n: Node| -> Option<ExprId> {
            match n {
                Node::Local(j) if j >= base && j < base + n_in => Some(a.local(1 + j - base)),
                Node::Local(_)
                | Node::Var(_)
                | Node::Pre(_)
                | Node::Der(_)
                | Node::Time
                | Node::Initial
                | Node::Rel(_)
                | Node::Delay(..) => {
                    bad = Some(n);
                    Some(Arena::ZERO)
                }
                _ => None,
            }
        };
        let arena = &mut self.model.arena;
        let mut next = vec![Arena::ZERO];
        for j in 0..n_in {
            next.push(arena.local(1 + j));
        }
        for &e in &out_exprs {
            next.push(arena.rewrite(e, &mut memo, &mut map_leaf));
        }
        if let Some(n) = bad {
            bail!("function body refers to {n:?}, which a compiled function cannot see");
        }
        let cond = arena.local(0);
        let pid = arena.add_prog(LoopProg {
            n: (1 + n_in + n_out) as usize,
            cond,
            next,
        });
        Ok((pid, n_in as usize, outs))
    }

    /// `while c loop .. end while` whose condition depends on run-time values: the locals
    /// the body changes become the state of a loop program, executed at run time.
    fn runtime_while(
        &mut self,
        block: &'a ast::StatementBlock,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<()> {
        if stmts_break(&block.stmts) {
            bail!("break in a while-loop with a run-time condition is not supported");
        }
        let keys: Vec<(usize, String)> = ctx
            .locals
            .iter()
            .enumerate()
            .flat_map(|(fi, f)| {
                let mut k: Vec<String> = f.keys().cloned().collect();
                k.sort();
                k.into_iter().map(move |n| (fi, n))
            })
            .collect();
        let saved = ctx.locals.clone();
        let mut init = Vec::new();
        for (fi, k) in &keys {
            flatten_scalars(&saved[*fi][k], &mut init);
        }
        // Pass 1 (every local a loop variable) finds the locals the body changes; pass 2
        // makes only those the state, so that constants (indices, sizes) stay constant.
        let mut changed = vec![true; init.len()];
        let mut result = None;
        for pass in 0..2 {
            let base = self.next_local;
            let mut slots = Vec::new(); // positions in `init` that are state
            let mut j = 0u32;
            let mut repl = Vec::with_capacity(init.len());
            for (k, &e) in init.iter().enumerate() {
                if changed[k] {
                    repl.push(self.model.arena.local(base + j));
                    slots.push(k);
                    j += 1;
                } else {
                    repl.push(e);
                }
            }
            self.next_local += j;
            let mut it = repl.iter().copied();
            let mut locals = saved.clone();
            for (fi, k) in &keys {
                let v = rebuild_scalars(&saved[*fi][k], &mut it);
                locals[*fi].insert(k.clone(), v);
            }
            ctx.locals = locals;
            let saved_path = ctx.path;
            ctx.path = None;
            let cond = self
                .translate(&block.cond, scope, ctx)
                .and_then(|v| v.scalar());
            let mut ret = false;
            let body = cond.and_then(|c| {
                self.exec(&block.stmts, scope, ctx, &mut ret)?;
                Ok(c)
            });
            ctx.path = saved_path;
            let cond = match body {
                Ok(c) => c,
                Err(e) => {
                    ctx.locals = saved;
                    return Err(e);
                }
            };
            if ret {
                ctx.locals = saved;
                bail!("return in a while-loop with a run-time condition is not supported");
            }
            let mut after = Vec::new();
            for (fi, k) in &keys {
                match ctx.locals[*fi].get(k) {
                    Some(v) => flatten_scalars(v, &mut after),
                    None => {}
                }
            }
            if after.len() != init.len() {
                ctx.locals = saved;
                bail!("a while-loop changes the size of a local");
            }
            if pass == 0 {
                for k in 0..init.len() {
                    changed[k] = after[k] != repl[k];
                }
                if !changed.iter().any(|&c| c) {
                    ctx.locals = saved;
                    bail!("while-loop with a run-time condition that changes nothing");
                }
                continue;
            }
            result = Some((base, slots, cond, after));
        }
        let (base, slots, cond, after) = result.unwrap();
        let n_state = slots.len() as u32;
        // Renumber the state to Local(0..n); everything else the loop reads from outside
        // (outer loop state, model variables, time) becomes an extra input.
        let mut foreign: Vec<ExprId> = Vec::new();
        let mut fmap: FxHashMap<Node, u32> = FxHashMap::default();
        let mut memo = FxHashMap::default();
        let mut map_leaf = |a: &mut Arena, n: Node| -> Option<ExprId> {
            match n {
                Node::Local(k) if k >= base && k < base + n_state => Some(a.local(k - base)),
                Node::Local(_)
                | Node::Var(_)
                | Node::Pre(_)
                | Node::Der(_)
                | Node::Time
                | Node::Initial
                | Node::Rel(_)
                | Node::Delay(..) => {
                    let i = match fmap.get(&n) {
                        Some(&i) => i,
                        None => {
                            foreign.push(a.node_id(n));
                            fmap.insert(n, (foreign.len() - 1) as u32);
                            (foreign.len() - 1) as u32
                        }
                    };
                    Some(a.local(n_state + i))
                }
                _ => None,
            }
        };
        let arena = &mut self.model.arena;
        let mut cond = arena.rewrite(cond, &mut memo, &mut map_leaf);
        let next: Vec<ExprId> = slots
            .iter()
            .map(|&k| arena.rewrite(after[k], &mut memo, &mut map_leaf))
            .collect();
        if let Some(p) = ctx.path {
            let pl = arena.rewrite(p, &mut memo, &mut map_leaf);
            cond = arena.f2(Fn2::And, pl, cond);
        }
        let n_foreign = foreign.len() as u32;
        let mut next = next;
        for i in 0..n_foreign {
            next.push(self.model.arena.local(n_state + i));
        }
        let pid = self.model.arena.add_prog(LoopProg {
            n: (n_state + n_foreign) as usize,
            cond,
            next,
        });
        let mut args: Vec<ExprId> = slots.iter().map(|&k| init[k]).collect();
        args.extend(foreign.iter().copied());
        let mut outv = init.clone();
        for (j, &k) in slots.iter().enumerate() {
            outv[k] = self.model.arena.call(pid, args.clone(), j as u32);
        }
        let mut it = outv.into_iter();
        let mut locals = saved.clone();
        for (fi, k) in &keys {
            let v = rebuild_scalars(&saved[*fi][k], &mut it);
            locals[*fi].insert(k.clone(), v);
        }
        ctx.locals = locals;
        Ok(())
    }

    fn exec_if(
        &mut self,
        blocks: &'a [ast::StatementBlock],
        else_block: Option<&'a [ast::Statement]>,
        scope: &Scope,
        ctx: &mut Ctx,
        returned: &mut bool,
    ) -> Result<()> {
        let Some((first, rest)) = blocks.split_first() else {
            if let Some(eb) = else_block {
                self.exec(eb, scope, ctx, returned)?;
            }
            return Ok(());
        };
        let c = self.translate(&first.cond, scope, ctx)?.scalar()?;
        if let Some(cv) = self.eval_param_expr(c) {
            return if cv != 0.0 {
                self.exec(&first.stmts, scope, ctx, returned)
            } else {
                self.exec_if(rest, else_block, scope, ctx, returned)
            };
        }
        // Symbolic branch: execute both and merge locals with if-expressions.
        let base = ctx.locals.clone();
        let saved_path = ctx.path;
        let and = |a: &mut Arena, p: Option<ExprId>, c: ExprId| match p {
            Some(p) => a.f2(Fn2::And, p, c),
            None => c,
        };
        let (outer_ret, outer_brk) = (ctx.sret, ctx.sbrk);
        ctx.path = Some(and(&mut self.model.arena, saved_path, c));
        let mut r1 = false;
        let res = self.exec(&first.stmts, scope, ctx, &mut r1);
        ctx.path = saved_path;
        res?;
        // How each branch leaves: unconditionally (static flag) or under a run-time
        // condition (symbolic flag); `None` if it falls through.
        let exit_of = |stat: bool, sym: Option<ExprId>| -> ExprId {
            if stat { Arena::ONE } else { sym.unwrap_or(Arena::ZERO) }
        };
        let t_ret = exit_of(r1, ctx.sret);
        let t_brk = exit_of(ctx.broke, ctx.sbrk);
        let then_broke = ctx.broke;
        (ctx.sret, ctx.sbrk, ctx.broke) = (outer_ret, outer_brk, false);
        let then_locals = std::mem::replace(&mut ctx.locals, base);
        let nc = self.model.arena.not(c);
        ctx.path = Some(and(&mut self.model.arena, saved_path, nc));
        let mut r2 = false;
        let res = self.exec_if(rest, else_block, scope, ctx, &mut r2);
        ctx.path = saved_path;
        res?;
        let e_ret = exit_of(r2, ctx.sret);
        let e_brk = exit_of(ctx.broke, ctx.sbrk);
        let else_broke = ctx.broke;
        // Both branches leave: so does the `if`. Otherwise later statements are guarded
        // by the run-time condition under which one of them left.
        *returned = r1 && r2;
        ctx.broke = !*returned && then_broke && else_broke;
        let merge_flag = |a: &mut Arena, t: ExprId, e: ExprId| -> Option<ExprId> {
            if t == Arena::ZERO && e == Arena::ZERO {
                None
            } else if t == e {
                Some(t)
            } else {
                Some(a.if_(c, t, e))
            }
        };
        ctx.sret = merge_flag(&mut self.model.arena, t_ret, e_ret);
        ctx.sbrk = merge_flag(&mut self.model.arena, t_brk, e_brk);
        let else_locals = std::mem::take(&mut ctx.locals);
        let mut merged = else_locals.clone();
        for (fi, frame) in then_locals.iter().enumerate() {
            for (k, tv) in frame {
                let ev = else_locals.get(fi).and_then(|f| f.get(k)).cloned();
                let m = match ev {
                    Some(ev) => self.merge_branch(c, tv.clone(), ev)?,
                    None => tv.clone(),
                };
                merged[fi].insert(k.clone(), m);
            }
        }
        ctx.locals = merged;
        Ok(())
    }

    /// Run-time condition under which a statement no longer takes effect: the function
    /// has returned, or the innermost loop has broken, under a time-varying condition.
    fn exit_guard(&mut self, ctx: &Ctx) -> Option<ExprId> {
        match (ctx.sret, ctx.sbrk) {
            (None, None) => None,
            (Some(r), None) => Some(r),
            (None, Some(b)) => Some(b),
            (Some(r), Some(b)) => Some(self.model.arena.f2(Fn2::Or, r, b)),
        }
    }

    /// After an assignment under exit guard `g`, keep each changed local's old value
    /// where `g` holds.
    fn keep_if_exited(
        &mut self,
        g: ExprId,
        before: Vec<FxHashMap<String, Val>>,
        ctx: &mut Ctx,
    ) -> Result<()> {
        for (fi, frame) in before.into_iter().enumerate() {
            for (k, old) in frame {
                let Some(new) = ctx.locals.get(fi).and_then(|f| f.get(&k)).cloned() else {
                    continue;
                };
                if vals_identical(&old, &new) {
                    continue;
                }
                let m = self.merge_value(g, old, new)?;
                ctx.locals[fi].insert(k, m);
            }
        }
        Ok(())
    }

    /// `if c then t else e` on values of any shape (records field by field).
    fn merge_value(&mut self, c: ExprId, t: Val, e: Val) -> Result<Val> {
        let t = if matches!(t, Val::Inst(_)) && matches!(e, Val::Rec(_) | Val::Inst(_)) {
            self.as_record(&t)?
        } else {
            t
        };
        let e = if matches!(e, Val::Inst(_)) && matches!(t, Val::Rec(_)) {
            self.as_record(&e)?
        } else {
            e
        };
        self.merge_branch(c, t, e)
    }

    /// Merge the value a local has after the two branches of a time-varying `if`.
    fn merge_branch(&mut self, c: ExprId, tv: Val, ev: Val) -> Result<Val> {
        match (tv, ev) {
            (Val::Rec(tf), Val::Rec(ef)) => {
                let mut out = Vec::with_capacity(tf.len());
                for (n, t) in tf {
                    let m = match ef.iter().find(|(m, _)| *m == n) {
                        Some((_, e)) => self.merge_branch(c, t, e.clone())?,
                        None => t,
                    };
                    out.push((n, m));
                }
                Ok(Val::Rec(out))
            }
            (Val::Str(t), _) => Ok(Val::Str(t)),
            (t @ (Val::Inst(_) | Val::IA(..) | Val::Func(_)), _) => Ok(t),
            (t, e) => self.zip2(t, e, &mut |a, x, y| if x == y { x } else { a.if_(c, x, y) }),
        }
    }

    fn assign_local(
        &mut self,
        target: &'a ast::ComponentReference,
        v: Val,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<()> {
        let name = target.parts[0].ident.text.to_string();
        let frame = ctx.locals.iter().rposition(|f| f.contains_key(&name));
        if target.parts.len() == 1 && target.parts[0].subs.is_none() {
            let frame = frame.unwrap_or(ctx.locals.len() - 1);
            ctx.locals[frame].insert(name, v);
            return Ok(());
        }
        let frame = frame.ok_or_else(|| anyhow!("unknown variable '{name}'"))?;
        let cur = ctx.locals[frame][&name].clone();
        let new = self.assign_path(
            cur,
            target.parts[0].subs.as_deref(),
            &target.parts[1..],
            v,
            scope,
            ctx,
        )?;
        ctx.locals[frame].insert(name, new);
        Ok(())
    }

    /// `cur[subs].rest := v`, returning the updated value of `cur`.
    fn assign_path(
        &mut self,
        cur: Val,
        subs: Option<&'a [ast::Subscript]>,
        rest: &'a [ast::ComponentRefPart],
        v: Val,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        if rest.is_empty() {
            return match subs {
                None => Ok(v),
                Some(s) => self.assign_into(cur, s, v, scope, ctx),
            };
        }
        let cur = if matches!(cur, Val::Inst(_) | Val::IA(..)) {
            self.to_rec(cur)?
        } else {
            cur
        };
        let Val::Rec(mut fields) = cur else {
            bail!("field assignment on a non-record value")
        };
        let f = &*rest[0].ident.text;
        let pos = match fields.iter().position(|(n, _)| n == f) {
            Some(p) => p,
            None => {
                fields.push((f.to_string(), Val::S(Arena::ZERO)));
                fields.len() - 1
            }
        };
        let fv = fields[pos].1.clone();
        // `c[i].re := x` on an array of records stored field by field: the record subscript
        // addresses the leading dimensions of the field.
        let nv = match (subs, rest[0].subs.as_deref()) {
            (Some(s), None) if rest.len() == 1 => self.assign_into(fv, s, v, scope, ctx)?,
            (None, fsubs) => self.assign_path(fv, fsubs, &rest[1..], v, scope, ctx)?,
            _ => bail!("unsupported assignment target"),
        };
        fields[pos].1 = nv;
        Ok(Val::Rec(fields))
    }

    /// `cur[subs] := v` for arrays, and field by field for arrays of records.
    fn assign_into(
        &mut self,
        cur: Val,
        subs: &'a [ast::Subscript],
        v: Val,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        match cur {
            Val::A(dims, mut el) => {
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (_, idx) = select_indices(&dims, &sel);
                let (_, vals) = v.elems()?;
                for (k, i) in idx.into_iter().enumerate() {
                    el[i] = *vals.get(k).unwrap_or(&vals[0]);
                }
                Ok(Val::A(dims, el))
            }
            Val::Rec(fields) => {
                let vr = if Self::is_record_like(&v) {
                    self.to_rec(v)?
                } else {
                    let (re, im) = self.complex_parts(v)?;
                    Self::complex(re, im)
                };
                let mut out = Vec::with_capacity(fields.len());
                for (n, fv) in fields {
                    if matches!(fv, Val::Str(_)) {
                        out.push((n, fv));
                        continue;
                    }
                    let part = self.field(vr.clone(), &n).unwrap_or(Val::S(Arena::ZERO));
                    let nv = self.assign_into(fv, subs, part, scope, ctx)?;
                    out.push((n, nv));
                }
                Ok(Val::Rec(out))
            }
            other => bail!("indexed assignment to a {} value", other.kind()),
        }
    }

    /// Algorithm section in a model: symbolic execution gives one equation per assigned variable.
    fn translate_algorithm(
        &mut self,
        stmts: &'a [ast::Statement],
        scope: &Scope,
        initial: bool,
    ) -> Result<()> {
        // Collect assigned targets (top-level names) and when-statements.
        let mut ctx = Ctx::default();
        ctx.locals.push(FxHashMap::default());
        let mut plain = Vec::new();
        for s in stmts {
            if let ast::Statement::When(blocks) = s {
                let mut branches = Vec::new();
                for b in blocks {
                    let c = self.when_condition(&b.cond, scope, &mut ctx)?;
                    let mut acts = Vec::new();
                    let mut rest = Vec::new();
                    for st in &b.stmts {
                        match st {
                            ast::Statement::Reinit { variable, value } => {
                                let t = self.resolve_ref(variable, scope, &mut ctx)?.scalar()?;
                                let v = self.translate(value, scope, &mut ctx)?.scalar()?;
                                if let Node::Var(var) = self.model.arena.node(t) {
                                    acts.push(WhenAction::Reinit(var, v));
                                }
                            }
                            ast::Statement::FunctionCall { comp, args, .. }
                                if cref_name(comp) == "reinit" =>
                            {
                                let t = self.translate(&args[0], scope, &mut ctx)?.scalar()?;
                                let v = self.translate(&args[1], scope, &mut ctx)?.scalar()?;
                                if let Node::Var(var) = self.model.arena.node(t) {
                                    acts.push(WhenAction::Reinit(var, v));
                                }
                            }
                            ast::Statement::FunctionCall { outputs, .. } if outputs.is_empty() => {}
                            ast::Statement::Assert { .. } => {}
                            other => rest.push(other.clone()),
                        }
                    }
                    // The other statements execute in order (tuple assignments, ifs and
                    // loops included); each variable they assign gets its final value.
                    let mut targets: Vec<String> = Vec::new();
                    collect_targets(&rest, &mut targets);
                    let mut wctx = Ctx::default();
                    wctx.locals.push(FxHashMap::default());
                    let mut orig = Vec::new();
                    for t in &targets {
                        let cr: &'a ast::ComponentReference = Box::leak(Box::new(simple_cref(t)));
                        let v = self.resolve_ref(cr, scope, &mut wctx)?;
                        wctx.locals[0].insert(t.clone(), v.clone());
                        orig.push((t.clone(), v));
                    }
                    let leaked: &'a [ast::Statement] = Box::leak(rest.into_boxed_slice());
                    let mut returned = false;
                    self.exec(leaked, scope, &mut wctx, &mut returned)?;
                    for (t, ov) in orig {
                        let nv = wctx.locals[0][&t].clone();
                        let (mut oe, mut ne) = (Vec::new(), Vec::new());
                        flatten_scalars(&ov, &mut oe);
                        flatten_scalars(&nv, &mut ne);
                        for (i, &o) in oe.iter().enumerate() {
                            let Some(&n) = ne.get(i).or(ne.first()) else {
                                continue;
                            };
                            if n == o {
                                continue;
                            }
                            if let Node::Var(var) = self.model.arena.node(o) {
                                self.when_assigned.insert(var);
                                acts.push(WhenAction::Assign(var, n));
                            }
                        }
                    }
                    branches.push((c, acts));
                }
                let o = self.eq_origin(scope, "when-statement");
                self.model.whens.push(WhenClause {
                    branches,
                    origin: o,
                });
            } else {
                plain.push(s.clone());
            }
        }
        if plain.is_empty() {
            return Ok(());
        }
        // Targets assigned in the algorithm become locals initialised with their variable.
        let mut targets: Vec<String> = Vec::new();
        collect_targets(stmts, &mut targets);
        for t in &targets {
            let cr = ast::ComponentReference {
                local: false,
                parts: vec![ast::ComponentRefPart {
                    ident: rumoca_core::Token {
                        text: t.as_str().into(),
                        ..Default::default()
                    },
                    subs: None,
                    def_id: None,
                }],
                span: rumoca_core::Span::DUMMY,
                qualified_display_name: None,
            };
            let cr: &'a ast::ComponentReference = Box::leak(Box::new(cr));
            let v = self.resolve_ref(cr, scope, &mut ctx)?;
            ctx.locals[0].insert(t.clone(), v);
        }
        let start_vals = ctx.locals[0].clone();
        let leaked: &'a [ast::Statement] = Box::leak(plain.into_boxed_slice());
        let mut returned = false;
        // Execute without function scoping: references fall back to instance components.
        self.exec(leaked, scope, &mut ctx, &mut returned)?;
        let o = self.eq_origin(scope, "algorithm");
        for t in targets {
            let before = start_vals[&t].clone();
            let after = ctx.locals[0][&t].clone();
            let mut out = Vec::new();
            self.push_residuals(before, after, &mut out, o)?;
            if initial {
                self.model.initial_equations.extend(out);
            } else {
                self.model.equations.extend(out);
            }
        }
        Ok(())
    }
}

impl Val {
    fn normalize(self) -> Val {
        match self {
            Val::A(d, v) if d.is_empty() => Val::S(v[0]),
            other => other,
        }
    }
}

fn simple_cref(name: &str) -> ast::ComponentReference {
    ast::ComponentReference {
        local: false,
        parts: vec![ast::ComponentRefPart {
            ident: rumoca_core::Token {
                text: name.into(),
                ..Default::default()
            },
            subs: None,
            def_id: None,
        }],
        span: rumoca_core::Span::DUMMY,
        qualified_display_name: None,
    }
}

fn collect_targets(stmts: &[ast::Statement], out: &mut Vec<String>) {
    for s in stmts {
        match s {
            ast::Statement::Assignment { comp, .. } => {
                let n = comp.parts[0].ident.text.to_string();
                if !out.contains(&n) {
                    out.push(n);
                }
            }
            ast::Statement::FunctionCall { outputs, .. } => {
                for o in outputs {
                    if let E::ComponentReference(cr) = o {
                        let n = cr.parts[0].ident.text.to_string();
                        if !out.contains(&n) {
                            out.push(n);
                        }
                    }
                }
            }
            ast::Statement::If {
                cond_blocks,
                else_block,
            } => {
                for b in cond_blocks {
                    collect_targets(&b.stmts, out);
                }
                if let Some(e) = else_block {
                    collect_targets(e, out);
                }
            }
            ast::Statement::For { equations, .. } => collect_targets(equations, out),
            ast::Statement::While(b) => collect_targets(&b.stmts, out),
            _ => {}
        }
    }
}

/// Total time derivative of an expression (der() of a general expression).
pub fn time_derivative(a: &mut Arena, x: ExprId) -> ExprId {
    if let Node::Var(v) = a.node(x) {
        return a.der(v);
    }
    let mut memo = FxHashMap::default();
    let mut seed = |ar: &mut Arena, n: Node| -> Option<ExprId> {
        match n {
            Node::Var(v) => Some(ar.der(v)),
            Node::Time => Some(Arena::ONE),
            _ => None,
        }
    };
    a.forward_diff(x, &mut seed, &mut memo)
}

/// The modifier seen by element `k` of an array of `n` structured components: values not
/// marked `each` are sliced along their leading dimensions when they are evaluated.
fn select_mod_element<'a>(m: &Mod<'a>, k: usize, n: usize) -> Mod<'a> {
    let mut r = m.clone();
    mark_selection(&mut r, k, n);
    r
}

fn mark_selection(m: &mut Mod<'_>, k: usize, n: usize) {
    if m.each {
        // `each` belongs to this array of components; inside an element the value
        // distributes over the variable's own dimensions as usual.
        m.each = false;
        return;
    }
    if let Some(v) = m.value.as_mut() {
        v.sel.push((k, n));
    }
    for (_, sm) in m.subs.iter_mut() {
        mark_selection(sm, k, n);
    }
}

/// Element `k` of `n` along the leading dimensions of an array value; values whose leading
/// dimensions do not cover `n` elements are left as they are (applied to every element).
fn slice_leading(v: Val, k: usize, n: usize) -> Val {
    fn split(d: &[usize], len: usize, n: usize) -> Option<usize> {
        let mut p = 1;
        for (j, &x) in d.iter().enumerate() {
            p *= x;
            if p == n {
                return Some(j + 1);
            }
            if p > n {
                return None;
            }
        }
        let _ = len;
        None
    }
    match v {
        Val::A(d, x) if n > 0 && k < n => match split(&d, x.len(), n) {
            Some(j) => {
                let chunk = x.len() / n;
                let rest = d[j..].to_vec();
                let part = x[k * chunk..(k + 1) * chunk].to_vec();
                if rest.is_empty() {
                    Val::S(part[0])
                } else {
                    Val::A(rest, part)
                }
            }
            None => Val::A(d, x),
        },
        Val::IA(d, x) if n > 0 && k < n => match split(&d, x.len(), n) {
            Some(j) => {
                let chunk = x.len() / n;
                let rest = d[j..].to_vec();
                let part = x[k * chunk..(k + 1) * chunk].to_vec();
                if rest.is_empty() {
                    Val::Inst(part[0])
                } else {
                    Val::IA(rest, part)
                }
            }
            None => Val::IA(d, x),
        },
        Val::Tuple(mut t) if !t.is_empty() => {
            let first = t.remove(0);
            slice_leading(first, k, n)
        }
        Val::Rec(fields) => Val::Rec(
            fields
                .into_iter()
                .map(|(f, v)| {
                    (
                        f,
                        if matches!(v, Val::Str(_)) {
                            v
                        } else {
                            slice_leading(v, k, n)
                        },
                    )
                })
                .collect(),
        ),
        other => other,
    }
}

fn dot(prefix: &str) -> String {
    if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}.")
    }
}

fn index_str(dims: &[usize], mut k: usize) -> String {
    let mut idx = vec![0; dims.len()];
    for d in (0..dims.len()).rev() {
        idx[d] = k % dims[d] + 1;
        k /= dims[d];
    }
    idx.iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Apply a subscript selection to row-major dims. `None` = full range; `Some([MAX, ..])` = vector subscript.
fn select_indices(dims: &[usize], sel: &[Option<Vec<usize>>]) -> (Vec<usize>, Vec<usize>) {
    let mut per_dim: Vec<(bool, Vec<usize>)> = Vec::new();
    for (d, &n) in dims.iter().enumerate() {
        match sel.get(d) {
            None | Some(None) => per_dim.push((true, (0..n).collect())),
            Some(Some(v)) => {
                if v.first() == Some(&usize::MAX) {
                    per_dim.push((true, v[1..].to_vec()));
                } else {
                    per_dim.push((false, v.clone()));
                }
            }
        }
    }
    let new_dims: Vec<usize> = per_dim
        .iter()
        .filter(|(keep, _)| *keep)
        .map(|(_, v)| v.len())
        .collect();
    let mut out = vec![0usize];
    for (d, (_, v)) in per_dim.iter().enumerate() {
        let stride: usize = dims[d + 1..].iter().product();
        let mut next = Vec::with_capacity(out.len() * v.len());
        for &base in &out {
            for &i in v {
                next.push(base + i * stride);
            }
        }
        out = next;
    }
    (new_dims, out)
}

fn split_args(args: &[E]) -> (Vec<&E>, Vec<(&str, &E)>) {
    let mut pos = Vec::new();
    let mut named = Vec::new();
    for a in args {
        match a {
            E::NamedArgument { name, value, .. } => named.push((&*name.text, &**value)),
            other => pos.push(other),
        }
    }
    (pos, named)
}

pub fn cref_name(c: &ast::ComponentReference) -> String {
    c.parts
        .iter()
        .map(|p| p.ident.text.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn expr_string(e: &E) -> String {
    match e {
        E::Terminal {
            token,
            terminal_type: TerminalType::String,
            ..
        } => token.text.trim_matches('"').to_string(),
        E::Binary { lhs, rhs, .. } => format!("{}{}", expr_string(lhs), expr_string(rhs)),
        _ => String::new(),
    }
}

pub fn literal_number(e: &E) -> Option<f64> {
    match e {
        E::Terminal {
            token,
            terminal_type: TerminalType::UnsignedReal | TerminalType::UnsignedInteger,
            ..
        } => token.text.parse().ok(),
        E::Unary {
            op: OpUnary::Minus,
            rhs,
            ..
        } => literal_number(rhs).map(|v| -v),
        E::Parenthesized { inner, .. } => literal_number(inner),
        _ => None,
    }
}

fn builtin_enum_literal(enum_name: &str, lit: &str) -> Option<f64> {
    match enum_name {
        "StateSelect" => Some(match lit {
            "never" => 1.0,
            "avoid" => 2.0,
            "default" => 3.0,
            "prefer" => 4.0,
            "always" => 5.0,
            _ => return None,
        }),
        "AssertionLevel" => Some(match lit {
            "error" => 1.0,
            "warning" => 2.0,
            _ => return None,
        }),
        "Init" | "InitPID" => None,
        _ => None,
    }
}

/// Whether an equation (or one nested in it) is a connect equation.
/// A constant condition element needs no edge variable.
fn v_len_one(a: &Arena, e: ExprId) -> bool {
    matches!(a.node(e), Node::Const(_))
}

fn has_connect(eq: &ast::Equation) -> bool {
    match eq {
        ast::Equation::Connect { .. } => true,
        ast::Equation::For { equations, .. } => equations.iter().any(has_connect),
        _ => false,
    }
}

/// The scalar connectors of a connect argument: the elements of a connector array.
fn comp_elements(c: &Comp) -> Vec<Comp> {
    match c {
        Comp::Arr(_, el) => el.iter().flat_map(comp_elements).collect(),
        other => vec![other.clone()],
    }
}

/// The `__Daedalus_builtin = "name"` entry of a function's annotation.
fn builtin_annotation(ann: &[E]) -> Option<String> {
    ann.iter().find_map(|a| match a {
        E::Modification { target, value, .. }
            if target.parts.len() == 1 && &*target.parts[0].ident.text == "__Daedalus_builtin" =>
        {
            match &**value {
                E::Terminal { token, .. } => Some(token.text.trim_matches('"').to_string()),
                _ => None,
            }
        }
        _ => None,
    })
}

/// Scalars of a value in a fixed order (strings and instances have none).
fn flatten_scalars(v: &Val, out: &mut Vec<ExprId>) {
    match v {
        Val::S(e) => out.push(*e),
        Val::A(_, el) => out.extend_from_slice(el),
        Val::Rec(f) => {
            for (_, x) in f {
                flatten_scalars(x, out);
            }
        }
        Val::Tuple(t) => {
            for x in t {
                flatten_scalars(x, out);
            }
        }
        Val::Inst(_) | Val::IA(..) | Val::Str(_) | Val::SA(..) | Val::Func(_) => {}
    }
}

/// `v` with its scalars (in [`flatten_scalars`] order) taken from `it`.
fn rebuild_scalars(v: &Val, it: &mut impl Iterator<Item = ExprId>) -> Val {
    match v {
        Val::S(_) => Val::S(it.next().unwrap_or(Arena::ZERO)),
        Val::A(d, el) => Val::A(
            d.clone(),
            el.iter()
                .map(|_| it.next().unwrap_or(Arena::ZERO))
                .collect(),
        ),
        Val::Rec(f) => Val::Rec(
            f.iter()
                .map(|(n, x)| (n.clone(), rebuild_scalars(x, it)))
                .collect(),
        ),
        Val::Tuple(t) => Val::Tuple(t.iter().map(|x| rebuild_scalars(x, it)).collect()),
        other => other.clone(),
    }
}

/// A `break` that leaves this statement list's loop (not one of a nested loop).
/// Whether two values are the same expressions (the arena is hash-consed).
fn vals_identical(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::S(x), Val::S(y)) => x == y,
        (Val::A(d1, x), Val::A(d2, y)) => d1 == d2 && x == y,
        (Val::Inst(x), Val::Inst(y)) => x == y,
        (Val::IA(d1, x), Val::IA(d2, y)) => d1 == d2 && x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::SA(d1, x), Val::SA(d2, y)) => d1 == d2 && x == y,
        (Val::Rec(x), Val::Rec(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|((n, u), (m, v))| n == m && vals_identical(u, v))
        }
        (Val::Tuple(x), Val::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(u, v)| vals_identical(u, v))
        }
        (Val::Func(x), Val::Func(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn stmts_break(stmts: &[ast::Statement]) -> bool {
    stmts.iter().any(|s| match s {
        ast::Statement::Break { .. } => true,
        ast::Statement::If {
            cond_blocks,
            else_block,
        } => {
            cond_blocks.iter().any(|b| stmts_break(&b.stmts))
                || else_block.as_deref().is_some_and(stmts_break)
        }
        _ => false,
    })
}

/// `annotation(name = true)`.
fn annotation_flag(ann: &[E], name: &str) -> bool {
    ann.iter().any(|a| match a {
        E::Modification { target, value, .. }
            if target.parts.len() == 1 && &*target.parts[0].ident.text == name =>
        {
            matches!(&**value, E::Terminal { token, .. } if &*token.text == "true")
        }
        _ => false,
    })
}

/// Rough size of an algorithm: statements, loops counting extra.
fn stmt_weight(stmts: &[ast::Statement]) -> usize {
    stmts
        .iter()
        .map(|s| match s {
            ast::Statement::If {
                cond_blocks,
                else_block,
            } => {
                1 + cond_blocks
                    .iter()
                    .map(|b| stmt_weight(&b.stmts))
                    .sum::<usize>()
                    + else_block.as_deref().map_or(0, stmt_weight)
            }
            ast::Statement::For { equations, .. } => 6 + stmt_weight(equations),
            ast::Statement::While(b) => 12 + stmt_weight(&b.stmts),
            _ => 1,
        })
        .sum()
}

/// Shape of a value for the compiled-function cache.
fn val_shape(v: &Val) -> String {
    match v {
        Val::S(_) => "s".into(),
        Val::A(d, _) => format!("{d:?}"),
        Val::Rec(f) => format!(
            "{{{}}}",
            f.iter()
                .map(|(n, x)| format!("{n}:{}", val_shape(x)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Val::Tuple(t) => format!(
            "({})",
            t.iter().map(val_shape).collect::<Vec<_>>().join(",")
        ),
        other => other.kind().to_string(),
    }
}
