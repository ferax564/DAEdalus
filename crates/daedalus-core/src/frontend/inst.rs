//! Instantiation and flattening: class tree + top model name -> [`FlatModel`].
//!
//! Pass 1 builds the instance tree (components, array dimensions, conditional
//! components, modifiers). Pass 2 translates bindings and equations of every
//! instance into the flat expression arena, inlining functions by symbolic
//! execution. Connection sets are resolved last.

use super::tree::{ClassId, ClassTree, ROOT, builtin_name};
use crate::expr::{Arena, Cmp, ExprId, Fn1, Fn2, Node, Table, VarId};
use crate::flat::*;
use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use rumoca_core::{ArrayConstructor, Causality, ClassType, OpBinary, OpUnary, Variability};
use rumoca_ir_ast as ast;
use rumoca_ir_ast::{Expression as E, TerminalType};
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

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
    r
}

#[derive(Clone, Debug)]
pub enum Comp {
    Var(VarId),
    Inst(InstId),
    /// Array of components (shared: cloning a component reference is O(1)).
    Arr(Vec<usize>, std::rc::Rc<Vec<Comp>>),
    Removed,
}

pub struct Instance<'a> {
    pub prefix: String,
    pub class: ClassId,
    pub env: Rc<Env>,
    pub comps: IndexMap<String, Comp>,
    /// Components declared `inner` (by name) for outer resolution.
    pub inners: FxHashSet<String>,
    pub parent: Option<InstId>,
    pub is_connector: bool,
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
    /// Record value: ordered fields.
    Rec(Vec<(String, Val)>),
    /// Several function outputs (tuple).
    Tuple(Vec<Val>),
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
            Val::Rec(_) => "record",
            Val::Tuple(_) => "tuple",
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
    if dims.is_empty() { Val::S(v[0]) } else { Val::A(dims, v) }
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
}

pub struct Flattener<'a, 't> {
    pub tree: &'t ClassTree<'a>,
    pub model: FlatModel,
    pub insts: Vec<Instance<'a>>,
    pending: Vec<PendingVar<'a>>,
    env_cache: FxHashMap<ClassId, Rc<Env>>,
    const_cache: FxHashMap<(ClassId, String), Val>,
    const_in_progress: FxHashSet<(ClassId, String)>,
    /// Connection points union-find.
    conn_parent: FxHashMap<ConnPoint, ConnPoint>,
    conn_flow: FxHashMap<VarId, bool>,
    /// All flow variables of connectors that are inside connectors of some instance.
    connector_flows: Vec<VarId>,
    /// Translated parameter bindings (lazy; for structural evaluation).
    param_values: FxHashMap<VarId, f64>,
    binding_done: FxHashSet<VarId>,
    binding_mods: FxHashMap<VarId, usize>,
    sample_count: usize,
    when_assigned: FxHashSet<VarId>,
    depth: usize,
    pub var_by_name: FxHashMap<String, VarId>,
}

const MAX_DEPTH: usize = 200;

impl<'a, 't> Flattener<'a, 't> {
    pub fn new(tree: &'t ClassTree<'a>) -> Self {
        Flattener {
            tree,
            model: FlatModel { arena: Arena::new(), ..Default::default() },
            insts: Vec::new(),
            pending: Vec::new(),
            env_cache: FxHashMap::default(),
            const_cache: FxHashMap::default(),
            const_in_progress: FxHashSet::default(),
            conn_parent: FxHashMap::default(),
            conn_flow: FxHashMap::default(),
            connector_flows: Vec::new(),
            param_values: FxHashMap::default(),
            binding_done: FxHashSet::default(),
            binding_mods: FxHashMap::default(),
            sample_count: 0,
            when_assigned: FxHashSet::default(),
            depth: 0,
            var_by_name: FxHashMap::default(),
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
        let e = Rc::new(Env { class, overrides: Rc::new(FxHashMap::default()), parent });
        self.env_cache.insert(class, e.clone());
        e
    }

    fn env_with(&mut self, class: ClassId, overrides: Rc<FxHashMap<String, ClassId>>) -> Rc<Env> {
        if overrides.is_empty() {
            return self.env_of(class);
        }
        let parent = self.tree.node(class).parent.map(|p| self.env_of(p));
        Rc::new(Env { class, overrides, parent })
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

    fn lookup_class_name(&self, env: &Env, name: &ast::Name) -> Option<ClassId> {
        let parts: Vec<&str> = name.name.iter().map(|t| &*t.text).collect();
        self.lookup_class(env, &parts)
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

    fn add_mod_expr(&self, m: &mut Mod<'a>, e: &'a E, each: bool, final_: bool, redeclare: bool, scope: &Scope) {
        match e {
            E::Modification { target, value, .. } => {
                let path: Vec<String> = target.parts.iter().map(|p| p.ident.text.to_string()).collect();
                if let E::ClassModification { target: cls, modifications, .. } = &**value
                    && (redeclare || (modifications.is_empty() && self.looks_like_class(cls, scope)))
                {
                    if path.len() == 1 {
                        m.redeclares.push((path[0].clone(), cls, scope.clone()));
                    }
                    return;
                }
                let sub = Mod {
                    value: Some(Scoped { expr: value, scope: scope.clone() }),
                    each,
                    final_,
                    ..Default::default()
                };
                m.set_path(&path, sub);
            }
            E::ClassModification { target, modifications, each_flags, final_flags, redeclare_flags, .. } => {
                let path: Vec<String> = target.parts.iter().map(|p| p.ident.text.to_string()).collect();
                let mut sub = self.mods_from_exprs(modifications, each_flags, final_flags, redeclare_flags, scope);
                sub.each = each;
                sub.final_ = final_;
                m.set_path(&path, sub);
            }
            E::NamedArgument { name, value, .. } => {
                let sub = Mod {
                    value: Some(Scoped { expr: value, scope: scope.clone() }),
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
        self.lookup_class(&scope.env, &parts).is_some_and(|c| {
            !matches!(self.tree.class_type(c), ClassType::Function)
        })
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
            m.value = Some(Scoped { expr: b, scope: scope.clone() });
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
        let n = self.insts.len();
        for i in 0..n {
            self.translate_instance(i as InstId)
                .with_context(|| format!("in component '{}'", self.inst_name(i as InstId)))?;
        }
        self.finish_connections();
        self.finish_discrete();
        let _ = root;
        Ok(())
    }

    fn inst_name(&self, i: InstId) -> String {
        let p = &self.insts[i as usize].prefix;
        if p.is_empty() { self.model.name.clone() } else { p.clone() }
    }

    fn read_experiment(&mut self, ann: &[E]) {
        for a in ann {
            if let E::ClassModification { target, modifications, .. } = a
                && target.parts.len() == 1
                && &*target.parts[0].ident.text == "experiment"
            {
                for m in modifications {
                    if let E::Modification { target, value, .. } = m {
                        let key = target.parts.iter().map(|p| p.ident.text.to_string()).collect::<Vec<_>>().join(".");
                        let num = literal_number(value);
                        match key.as_str() {
                            "StartTime" => self.model.experiment.start_time = num,
                            "StopTime" => self.model.experiment.stop_time = num,
                            "Tolerance" => self.model.experiment.tolerance = num,
                            "Interval" => self.model.experiment.interval = num,
                            "__Dymola_Algorithm" | "__Daedalus_Algorithm" => {
                                if let E::Terminal { token, .. } = &**value {
                                    self.model.experiment.algorithm = Some(token.text.trim_matches('"').to_string());
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
            Rc::new(Env { class: env.class, overrides: Rc::new(overrides), parent: env.parent.clone() })
        };
        let is_connector = matches!(self.tree.class_type(class), ClassType::Connector);
        let id = self.insts.len() as InstId;
        self.insts.push(Instance {
            prefix: prefix.clone(),
            class,
            env: env.clone(),
            comps: IndexMap::new(),
            inners: FxHashSet::default(),
            parent,
            is_connector,
            connector_comps: FxHashSet::default(),
            equations: Vec::new(),
            initial_equations: Vec::new(),
            algorithms: Vec::new(),
            initial_algorithms: Vec::new(),
        });
        let mut elems: Vec<(&'a ast::Component, Rc<Env>, Mod<'a>)> = Vec::new();
        self.collect(class, env.clone(), m, id, &mut elems, 0)?;
        for (comp, cenv, cm) in elems {
            if self.insts[id as usize].comps.contains_key(comp.name.as_str()) {
                continue;
            }
            self.instantiate_component(id, comp, cenv, cm)
                .with_context(|| format!("instantiating '{}{}'", dot(&prefix), comp.name))?;
        }
        self.depth -= 1;
        Ok(id)
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
        let scope = Scope { inst: Some(inst), env: env.clone() };
        for ext in &def.extends {
            let parts: Vec<&str> = ext.base_name.name.iter().map(|t| &*t.text).collect();
            if parts.len() == 1 && builtin_name(parts[0]).is_some() {
                continue;
            }
            // Extends names are resolved in the enclosing scope (with overrides).
            let lookup_env = match &env.parent {
                Some(p) => Rc::new(Env { class: p.class, overrides: env.overrides.clone(), parent: p.parent.clone() }),
                None => env.clone(),
            };
            let base = self
                .lookup_class(&lookup_env, &parts)
                .or_else(|| self.lookup_class(&env, &parts))
                .ok_or_else(|| anyhow!("base class '{}' not found", ext.base_name))?;
            let mut em = Mod::default();
            for x in &ext.modifications {
                self.add_mod_expr(&mut em, &x.expr, x.each, x.final_, x.redeclare, &scope);
            }
            let merged = merge(m, &em);
            // Redeclares in extends apply to the instance environment for inherited elements.
            let mut ov: FxHashMap<String, ClassId> = (*env.overrides).clone();
            for (name, cls, sc) in &merged.redeclares {
                let p: Vec<&str> = cls.parts.iter().map(|p| &*p.ident.text).collect();
                if let Some(c) = self.lookup_class(&sc.env, &p) {
                    ov.insert(name.clone(), c);
                }
            }
            let benv = self.env_with(base, Rc::new(ov));
            self.collect(base, benv.clone(), &merged, inst, out, depth + 1)?;
            // Make inherited overrides visible to the instance env as well.
            if !benv.overrides.is_empty() {
                let ienv = &self.insts[inst as usize].env;
                if ienv.overrides.len() < benv.overrides.len() {
                    let mut ov2 = (*ienv.overrides).clone();
                    for (k, v) in benv.overrides.iter() {
                        ov2.entry(k.clone()).or_insert(*v);
                    }
                    let ne = Rc::new(Env { class: ienv.class, overrides: Rc::new(ov2), parent: ienv.parent.clone() });
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
        let scope = Scope { inst: Some(inst), env: env.clone() };
        if let Some(cond) = &comp.condition {
            let v = self.eval_structural(cond, &scope)?;
            if v == 0.0 {
                self.insts[inst as usize].comps.insert(name, Comp::Removed);
                return Ok(());
            }
        }
        if comp.outer {
            let mut p = self.insts[inst as usize].parent;
            while let Some(pi) = p {
                if self.insts[pi as usize].inners.contains(&name)
                    && let Some(c) = self.insts[pi as usize].comps.get(&name).cloned()
                {
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
        let builtin = if tparts.len() == 1 { builtin_name(tparts[0]) } else { None };
        let tclass = if builtin.is_some() {
            None
        } else {
            Some(
                self.lookup_class(&env, &tparts)
                    .ok_or_else(|| anyhow!("type '{}' not found", comp.type_name))?,
            )
        };
        // Dimensions
        let mut dims = Vec::new();
        for s in &comp.shape_expr {
            match s {
                ast::Subscript::Expression(e) => {
                    let v = self.eval_structural(e, &scope)?;
                    dims.push(v.round().max(0.0) as usize);
                }
                ast::Subscript::Range { .. } | ast::Subscript::Empty => {
                    // Unknown size `[:]`: derive from the binding value if possible.
                    let d = if let Some(v) = &m.value {
                        let val = self.translate(v.expr, &v.scope.clone(), &mut Ctx::default())?;
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
        if let Some(tc) = tclass
            && let Some(td) = self.tree.def(tc)
        {
            let tenv = self.env_of(tc);
            for s in &td.array_subscripts {
                if let ast::Subscript::Expression(e) = s {
                    let v = self.eval_structural(e, &Scope { inst: None, env: tenv.clone() })?;
                    dims.push(v as usize);
                }
            }
        }
        let count: usize = dims.iter().product();
        let base = builtin.or_else(|| tclass.and_then(|c| self.tree.builtin_base(c)));
        let is_connector_type = tclass.is_some_and(|c| matches!(self.tree.class_type(c), ClassType::Connector));
        if is_connector_type {
            self.insts[inst as usize].connector_comps.insert(name.clone());
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
                    self.insts[inst as usize].comps.insert(name, Comp::Removed);
                    return Ok(());
                }
                _ => VarType::Enumeration,
            };
            let mut kind = match &comp.variability {
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
            if causality_input && at_top && matches!(kind, VarKind::Continuous | VarKind::Discrete) {
                kind = VarKind::Input;
            }
            let is_flow = matches!(comp.connection, ast::Connection::Flow(_));
            let desc: String = comp.description.iter().map(|t| t.text.trim_matches('"').to_string()).collect::<Vec<_>>().join("");
            let mut elems = Vec::with_capacity(count.max(1));
            let n = if dims.is_empty() { 1 } else { count };
            for k in 0..n {
                let vname = if dims.is_empty() { full.clone() } else { format!("{}[{}]", full, index_str(&dims, k)) };
                let mut v = Variable::new(vname.clone(), kind, ty);
                v.is_flow = is_flow;
                v.description = desc.clone();
                v.is_protected = comp.is_protected;
                v.is_output = causality_output && prefix.is_empty();
                let vid = self.model.add_var(v);
                self.var_by_name.insert(vname, vid);
                if is_flow && self.insts[inst as usize].is_connector {
                    self.connector_flows.push(vid);
                }
                if is_flow && self.insts[inst as usize].is_connector {
                    self.conn_flow.insert(vid, true);
                }
                let is_param = matches!(kind, VarKind::Parameter | VarKind::Constant);
                self.pending.push(PendingVar { var: vid, m: tm.clone(), index: k, count: n, is_param });
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
            let c = if dims.is_empty() { elems.pop().unwrap() } else { Comp::Arr(dims, std::rc::Rc::new(elems)) };
            self.insts[inst as usize].comps.insert(name, c);
            return Ok(());
        }
        let tc = tclass.unwrap();
        if matches!(self.tree.class_type(tc), ClassType::Function | ClassType::Package) {
            self.insts[inst as usize].comps.insert(name, Comp::Removed);
            return Ok(());
        }
        let tenv = {
            let te = self.env_of(tc);
            let ov = env.overrides.clone();
            if ov.is_empty() { te } else { Rc::new(Env { class: tc, overrides: ov, parent: te.parent.clone() }) }
        };
        // Record parameters (`parameter Rec r`) propagate variability to fields.
        let var_prefix = comp.variability.clone();
        let mut elems = Vec::new();
        let n = if dims.is_empty() { 1 } else { count };
        for k in 0..n {
            let cprefix = if dims.is_empty() { full.clone() } else { format!("{}[{}]", full, index_str(&dims, k)) };
            let cm = if n > 1 { select_mod_element(&m, k, n) } else { m.clone() };
            let child = self.instantiate(tc, cprefix, &cm, tenv.clone(), Some(inst))?;
            if !matches!(var_prefix, Variability::Empty) {
                self.propagate_variability(child, &var_prefix);
            }
            elems.push(Comp::Inst(child));
        }
        let c = if dims.is_empty() { elems.pop().unwrap() } else { Comp::Arr(dims, std::rc::Rc::new(elems)) };
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
            Comp::Removed => {}
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
        i.is_connector && i.parent.is_some_and(|p| self.insts[p as usize].prefix.is_empty())
    }

    // ------------------------------------------------------------------
    // Structural evaluation
    // ------------------------------------------------------------------

    /// Evaluate an expression that must be known at translation time.
    pub fn eval_structural(&mut self, e: &'a E, scope: &Scope) -> Result<f64> {
        let v = self.translate(e, scope, &mut Ctx::default())?;
        let x = v.scalar()?;
        self.eval_param_expr(x).ok_or_else(|| {
            anyhow!("expression '{}' must be a constant or parameter expression", self.model.show(x))
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
        let consts: FxHashMap<VarId, ExprId> = map.iter().map(|(&k, &v)| (k, arena.constant(v))).collect();
        let r = arena.substitute(e, &|n| if let Node::Var(v) = n { consts.get(&v).copied() } else { None });
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

    fn translate_pending(&mut self) -> Result<()> {
        for i in 0..self.pending.len() {
            let v = self.pending[i].var;
            if self.binding_done.contains(&v) {
                continue;
            }
            self.binding_done.insert(v);
            let name = self.model.vars[v as usize].name.clone();
            self.translate_attrs(i).with_context(|| format!("in declaration of '{name}'"))?;
        }
        Ok(())
    }

    fn element_of(&self, val: Val, index: usize, count: usize, each: bool) -> Result<ExprId> {
        match val {
            Val::S(e) => Ok(e),
            Val::A(_, v) => {
                if each || count == 1 {
                    if v.len() == 1 { Ok(v[0]) } else { bail!("array value for scalar attribute") }
                } else if v.len() == count {
                    Ok(v[index])
                } else if v.len() == 1 {
                    Ok(v[0])
                } else {
                    bail!("array value of size {} does not match {} elements", v.len(), count)
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
            let val = self.translate(s.expr, &s.scope.clone(), &mut ctx)?;
            let e = self.element_of(val, index, count, m.each)?;
            if is_param {
                self.model.vars[var as usize].binding = Some(e);
            } else {
                let lhs = self.model.arena.var(var);
                let o = self.model.origin(format!("declaration of {}", self.model.vars[var as usize].name));
                self.model.equations.push(Equation { lhs, rhs: e, origin: o });
            }
        }
        for (attr, sm) in &m.subs {
            let Some(s) = &sm.value else { continue };
            match attr.as_str() {
                "start" | "nominal" | "min" | "max" => {
                    let val = self.translate(s.expr, &s.scope.clone(), &mut ctx)?;
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
                    let val = self.translate(s.expr, &s.scope.clone(), &mut ctx)?;
                    let e = self.element_of(val, index, count, sm.each)?;
                    let f = self.eval_param_expr(e).unwrap_or(1.0);
                    self.model.vars[var as usize].fixed = Some(f != 0.0);
                }
                "unit" | "displayUnit" | "quantity" => {
                    if let E::Terminal { token, terminal_type: TerminalType::String, .. } = s.expr {
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
                    let val = self.translate(s.expr, &s.scope.clone(), &mut ctx)?;
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

    fn translate_instance(&mut self, id: InstId) -> Result<()> {
        let eqs = std::mem::take(&mut self.insts[id as usize].equations);
        for (eq, env) in &eqs {
            let scope = Scope { inst: Some(id), env: env.clone() };
            let mut ctx = Ctx::default();
            let mut out = Vec::new();
            self.translate_equation(eq, &scope, &mut ctx, &mut out, false)?;
            self.model.equations.extend(out);
        }
        let ieqs = std::mem::take(&mut self.insts[id as usize].initial_equations);
        for (eq, env) in &ieqs {
            let scope = Scope { inst: Some(id), env: env.clone() };
            let mut ctx = Ctx::default();
            let mut out = Vec::new();
            self.translate_equation(eq, &scope, &mut ctx, &mut out, true)?;
            self.model.initial_equations.extend(out);
        }
        let algs = std::mem::take(&mut self.insts[id as usize].algorithms);
        for (alg, env) in &algs {
            let scope = Scope { inst: Some(id), env: env.clone() };
            self.translate_algorithm(alg, &scope, false)?;
        }
        let ialgs = std::mem::take(&mut self.insts[id as usize].initial_algorithms);
        for (alg, env) in &ialgs {
            let scope = Scope { inst: Some(id), env: env.clone() };
            self.translate_algorithm(alg, &scope, true)?;
        }
        Ok(())
    }

    fn eq_origin(&mut self, scope: &Scope, what: &str) -> u32 {
        let name = scope.inst.map(|i| self.inst_name(i)).unwrap_or_default();
        self.model.origin(format!("{what} in {name}"))
    }

    fn push_residuals(&mut self, l: Val, r: Val, out: &mut Vec<Equation>, origin: u32) -> Result<()> {
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
            (Val::Inst(_), _) | (_, Val::Inst(_)) => {
                let lr = self.as_record(&l)?;
                let rr = self.as_record(&r)?;
                return self.push_residuals(lr, rr, out, origin);
            }
            _ => {}
        }
        let (ld, le) = l.elems()?;
        let (rd, re) = r.elems()?;
        if le.len() != re.len() {
            if re.len() == 1 {
                for &x in &le {
                    out.push(Equation { lhs: x, rhs: re[0], origin });
                }
                return Ok(());
            }
            bail!("equation size mismatch: {:?} vs {:?}", ld, rd);
        }
        for (a, b) in le.into_iter().zip(re) {
            out.push(Equation { lhs: a, rhs: b, origin });
        }
        Ok(())
    }

    fn as_record(&mut self, v: &Val) -> Result<Val> {
        match v {
            Val::Inst(i) => {
                let mut fields = Vec::new();
                let comps: Vec<(String, Comp)> =
                    self.insts[*i as usize].comps.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                for (n, c) in comps {
                    if let Some(val) = self.comp_value(&c) {
                        fields.push((n, val));
                    }
                }
                Ok(Val::Rec(fields))
            }
            Val::Rec(_) => Ok(v.clone()),
            _ => bail!("expected record value"),
        }
    }

    fn comp_value(&mut self, c: &Comp) -> Option<Val> {
        match c {
            Comp::Var(v) => Some(Val::S(self.var_ref(*v))),
            Comp::Inst(i) => self.as_record(&Val::Inst(*i)).ok(),
            Comp::Arr(..) => self.comp_to_val(c).ok(),
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
            ast::Equation::If { cond_blocks, else_block } => {
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
                        None => else_block.as_ref().map(|v| v.iter().collect()).unwrap_or_default(),
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
                        bail!("if-equation branches with non-parameter conditions must have the same number of equations");
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
                        out.push(Equation { lhs: res, rhs: Arena::ZERO, origin: o });
                    }
                }
            }
            ast::Equation::When(blocks) => {
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
                self.model.whens.push(WhenClause { branches, origin: o });
            }
            ast::Equation::FunctionCall { comp, args, .. } => {
                let name = cref_name(comp);
                match name.as_str() {
                    "assert" => {
                        if let Some(c) = args.first() {
                            let c = self.translate(c, scope, &mut Ctx { no_event: true, ..ctx.clone() })?.scalar()?;
                            let msg = args.get(1).map(|m| expr_string(m)).unwrap_or_default();
                            self.model.asserts.push((c, msg));
                        }
                    }
                    "terminate" => {
                        self.model.warnings.push("terminate() outside when-clause ignored".into());
                    }
                    _ => {
                        // Function call with no outputs in an equation section: ignore.
                    }
                }
            }
            ast::Equation::Assert { condition, message, .. } => {
                let c = self.translate(condition, scope, &mut Ctx { no_event: true, ..ctx.clone() })?.scalar()?;
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
                // when {c1, c2}: triggers when any element becomes true.
                let mut r = Arena::ZERO;
                for e in v {
                    r = self.model.arena.f2(Fn2::Or, r, e);
                }
                Ok(r)
            }
            other => bail!("invalid when condition ({})", other.kind()),
        }
    }

    fn when_actions(&mut self, e: &'a ast::Equation, scope: &Scope, ctx: &mut Ctx, acts: &mut Vec<WhenAction>) -> Result<()> {
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
                    let val = values.get(i).or(values.first()).copied().unwrap_or(Arena::ZERO);
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
                        acts.push(WhenAction::Terminate(args.first().map(expr_string).unwrap_or_default()));
                    }
                    "assert" => {
                        let c = self.translate(&args[0], scope, ctx)?.scalar()?;
                        acts.push(WhenAction::Assert(c, args.get(1).map(expr_string).unwrap_or_default()));
                    }
                    _ => {}
                }
            }
            ast::Equation::If { cond_blocks, else_block } => {
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
                for (_, a) in per_branch.iter().chain(std::iter::once(&(Arena::ZERO, else_acts.clone()))) {
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
                        let any = per_branch.iter().any(|(_, a)| find(a, reinit).is_some()) || find(&else_acts, reinit).is_some();
                        if !any {
                            continue;
                        }
                        let keep = if reinit { self.model.arena.var(v) } else { self.model.arena.pre(v) };
                        let mut r = find(&else_acts, reinit).unwrap_or(keep);
                        for (c, a) in per_branch.iter().rev() {
                            let x = find(a, reinit).unwrap_or(keep);
                            r = self.model.arena.if_(*c, x, r);
                        }
                        acts.push(if reinit { WhenAction::Reinit(v, r) } else { WhenAction::Assign(v, r) });
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
            ast::Equation::Assert { condition, message, .. } => {
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
            ctx.locals.last_mut().unwrap().insert(name.clone(), Val::S(v));
            let r = self.for_loop(&indices[1..], scope, ctx, body);
            ctx.locals.pop();
            r?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Connections
    // ------------------------------------------------------------------

    fn translate_connect(
        &mut self,
        a: &'a ast::ComponentReference,
        b: &'a ast::ComponentReference,
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<()> {
        let inst = scope.inst.ok_or_else(|| anyhow!("connect outside of a model"))?;
        let (ca, ia) = self.resolve_connector(a, inst, scope, ctx)?;
        let (cb, ib) = self.resolve_connector(b, inst, scope, ctx)?;
        let (Some(ca), Some(cb)) = (ca, cb) else {
            return Ok(()); // conditional connector removed
        };
        let mut pa = Vec::new();
        let mut pb = Vec::new();
        self.connector_leaves(&ca, &mut pa);
        self.connector_leaves(&cb, &mut pb);
        if pa.len() != pb.len() {
            bail!(
                "connect({}, {}): incompatible connectors ({} vs {} variables)",
                a,
                b,
                pa.len(),
                pb.len()
            );
        }
        for (va, vb) in pa.into_iter().zip(pb) {
            let flow = self.model.vars[va as usize].is_flow;
            self.conn_flow.insert(va, flow);
            self.conn_flow.insert(vb, flow);
            self.union(ConnPoint { var: va, inside: ia }, ConnPoint { var: vb, inside: ib });
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
                    Comp::Var(_) => bail!("cannot access '{}' in a scalar", p.ident.text),
                };
            }
            if let Some(subs) = &p.subs {
                cur = self.subscript_comp(cur, subs, scope, ctx)?;
            }
        }
        if matches!(cur, Comp::Removed) {
            return Ok((None, !outside));
        }
        Ok((Some(cur), !outside))
    }

    fn subscript_comp(&mut self, c: Comp, subs: &'a [ast::Subscript], scope: &Scope, ctx: &mut Ctx) -> Result<Comp> {
        let Comp::Arr(dims, el) = c else {
            bail!("subscript on a non-array component");
        };
        let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
        let (nd, idx) = select_indices(&dims, &sel);
        let v: Vec<Comp> = idx.into_iter().map(|i| el[i].clone()).collect();
        if nd.is_empty() { Ok(v.into_iter().next().unwrap()) } else { Ok(Comp::Arr(nd, std::rc::Rc::new(v))) }
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
            Comp::Removed => {}
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
        let mut inside_connected: FxHashSet<VarId> = FxHashSet::default();
        for (_, set) in &sets {
            if set.len() < 2 {
                continue;
            }
            for p in set {
                if p.inside {
                    inside_connected.insert(p.var);
                }
            }
            let flow = self.conn_flow.get(&set[0].var).copied().unwrap_or(false);
            if flow {
                let mut sum = Arena::ZERO;
                for p in set {
                    let v = self.model.arena.var(p.var);
                    sum = if p.inside { self.model.arena.add(sum, v) } else { self.model.arena.sub(sum, v) };
                }
                self.model.equations.push(Equation { lhs: sum, rhs: Arena::ZERO, origin });
            } else {
                let v0 = self.model.arena.var(set[0].var);
                for p in &set[1..] {
                    if p.var == set[0].var {
                        continue;
                    }
                    let v = self.model.arena.var(p.var);
                    self.model.equations.push(Equation { lhs: v0, rhs: v, origin });
                }
            }
        }
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
            self.model.equations.push(Equation { lhs: v, rhs: Arena::ZERO, origin: o2 });
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
            E::Terminal { terminal_type, token, .. } => match terminal_type {
                TerminalType::UnsignedReal | TerminalType::UnsignedInteger => {
                    let v: f64 = token.text.parse().map_err(|_| anyhow!("bad number '{}'", token.text))?;
                    Ok(Val::S(self.model.arena.constant(v)))
                }
                TerminalType::Bool => Ok(Val::S(if &*token.text == "true" { Arena::ONE } else { Arena::ZERO })),
                TerminalType::String => Ok(Val::Str(token.text.trim_matches('"').to_string())),
                TerminalType::End => {
                    let v = ctx.end_value.ok_or_else(|| anyhow!("'end' outside of a subscript"))?;
                    Ok(Val::S(self.model.arena.constant(v as f64)))
                }
                TerminalType::Empty => Ok(Val::S(Arena::ZERO)),
            },
            E::Parenthesized { inner, .. } => self.translate(inner, scope, ctx),
            E::Unary { op, rhs, .. } => {
                let v = self.translate(rhs, scope, ctx)?;
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
            E::If { branches, else_branch, .. } => {
                let mut r = self.translate(else_branch, scope, ctx)?;
                for (c, b) in branches.iter().rev() {
                    let cv = self.translate(c, scope, ctx)?.scalar()?;
                    // Parameter conditions are resolved statically when possible.
                    if let Some(x) = self.model.arena.as_const(cv) {
                        if x != 0.0 {
                            r = self.translate(b, scope, ctx)?;
                        }
                        continue;
                    }
                    let bv = self.translate(b, scope, ctx)?;
                    r = self.zip2(bv, r, &mut |a, x, y| a.if_(cv, x, y))?;
                }
                Ok(r)
            }
            E::Range { start, step, end, .. } => {
                let s = self.translate(start, scope, ctx)?.scalar()?;
                let en = self.translate(end, scope, ctx)?.scalar()?;
                let st = match step {
                    Some(x) => self.translate(x, scope, ctx)?.scalar()?,
                    None => Arena::ONE,
                };
                let (s, en, st) = (
                    self.eval_param_expr(s).ok_or_else(|| anyhow!("range start must be a parameter expression"))?,
                    self.eval_param_expr(en).ok_or_else(|| anyhow!("range end must be a parameter expression"))?,
                    self.eval_param_expr(st).ok_or_else(|| anyhow!("range step must be a parameter expression"))?,
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
            E::ArrayComprehension { expr, indices, .. } => {
                let mut out = Vec::new();
                let mut inner_dims = Vec::new();
                let mut outer = Vec::new();
                self.for_loop_collect(indices, scope, ctx, expr, &mut out, &mut inner_dims, &mut outer)?;
                let mut dims = outer;
                dims.extend(inner_dims);
                Ok(mk_arr(dims, out))
            }
            E::ArrayIndex { base, subscripts, .. } => {
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
            ctx.locals.last_mut().unwrap().insert(name.clone(), Val::S(v));
            let r = self.for_loop_collect(&indices[1..], scope, ctx, body, out, inner_dims, outer_dims);
            ctx.locals.pop();
            r?;
        }
        Ok(())
    }

    fn field(&mut self, b: Val, field: &str) -> Result<Val> {
        match b {
            Val::Rec(fields) => fields
                .into_iter()
                .find(|(n, _)| n == field)
                .map(|(_, v)| v)
                .ok_or_else(|| anyhow!("record has no field '{field}'")),
            Val::Inst(i) => {
                let c = self.insts[i as usize]
                    .comps
                    .get(field)
                    .cloned()
                    .ok_or_else(|| anyhow!("no element '{field}'"))?;
                self.comp_to_val(&c)
            }
            other => bail!("field access '.{field}' on {}", other.kind()),
        }
    }

    fn map1(&mut self, v: Val, f: &mut dyn FnMut(&mut Arena, ExprId) -> ExprId) -> Result<Val> {
        match v {
            Val::S(x) => Ok(Val::S(f(&mut self.model.arena, x))),
            Val::A(d, xs) => Ok(Val::A(d, xs.into_iter().map(|x| f(&mut self.model.arena, x)).collect())),
            Val::Tuple(t) if !t.is_empty() => self.map1(t[0].clone(), f),
            other => bail!("numeric operation on {}", other.kind()),
        }
    }

    fn zip2(&mut self, a: Val, b: Val, f: &mut dyn FnMut(&mut Arena, ExprId, ExprId) -> ExprId) -> Result<Val> {
        let (ad, ae) = a.elems()?;
        let (bd, be) = b.elems()?;
        if ae.len() == be.len() {
            let dims = if ad.len() >= bd.len() { ad } else { bd };
            let v = ae.into_iter().zip(be).map(|(x, y)| f(&mut self.model.arena, x, y)).collect();
            return Ok(mk_arr(dims, v));
        }
        if ae.len() == 1 && ad.is_empty() {
            let v = be.into_iter().map(|y| f(&mut self.model.arena, ae[0], y)).collect();
            return Ok(mk_arr(bd, v));
        }
        if be.len() == 1 && bd.is_empty() {
            let v = ae.into_iter().map(|x| f(&mut self.model.arena, x, be[0])).collect();
            return Ok(mk_arr(ad, v));
        }
        bail!("array size mismatch {:?} vs {:?}", ad, bd)
    }

    fn binary(&mut self, op: &OpBinary, l: Val, r: Val, ctx: &Ctx) -> Result<Val> {
        let no_event = ctx.no_event;
        match op {
            OpBinary::Add | OpBinary::AddElem => self.zip2(l, r, &mut |a, x, y| a.add(x, y)),
            OpBinary::Sub | OpBinary::SubElem => self.zip2(l, r, &mut |a, x, y| a.sub(x, y)),
            OpBinary::MulElem => self.zip2(l, r, &mut |a, x, y| a.mul(x, y)),
            OpBinary::DivElem | OpBinary::Div => self.zip2(l, r, &mut |a, x, y| a.div(x, y)),
            OpBinary::ExpElem | OpBinary::Exp => self.zip2(l, r, &mut |a, x, y| a.pow(x, y)),
            OpBinary::Mul => self.mul_val(l, r),
            OpBinary::And => self.zip2(l, r, &mut |a, x, y| a.f2(Fn2::And, x, y)),
            OpBinary::Or => self.zip2(l, r, &mut |a, x, y| a.f2(Fn2::Or, x, y)),
            OpBinary::Lt | OpBinary::Le | OpBinary::Gt | OpBinary::Ge | OpBinary::Eq | OpBinary::Neq => {
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
        if no_event || matches!(c, Cmp::Eq | Cmp::Ne) || (self.is_discrete_expr(x) && self.is_discrete_expr(y)) {
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
            (0, _) | (_, 0) => self.zip2(Val::A(ld, le).normalize(), Val::A(rd, re).normalize(), &mut |a, x, y| a.mul(x, y)),
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
                if vals.iter().all(|v| matches!(v, Val::Inst(_))) && !vals.is_empty() {
                    let v: Vec<InstId> = vals.iter().map(|x| if let Val::Inst(i) = x { *i } else { 0 }).collect();
                    return Ok(Val::IA(vec![v.len()], v));
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
                            anyhow!("subscript '{}' must be a constant or parameter expression", self.model.show(x))
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

    fn index_val(&mut self, b: Val, subs: &'a [ast::Subscript], scope: &Scope, ctx: &mut Ctx) -> Result<Val> {
        match b {
            Val::A(dims, el) => {
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (nd, idx) = select_indices(&dims, &sel);
                Ok(mk_arr(nd, idx.into_iter().map(|i| el[i]).collect()))
            }
            Val::IA(dims, el) => {
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (nd, idx) = select_indices(&dims, &sel);
                let v: Vec<InstId> = idx.into_iter().map(|i| el[i]).collect();
                if nd.is_empty() { Ok(Val::Inst(v[0])) } else { Ok(Val::IA(nd, v)) }
            }
            Val::S(e) if subs.iter().all(|s| matches!(s, ast::Subscript::Expression(_))) => Ok(Val::S(e)),
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
                        .map(|e| if let Comp::Var(x) = e { self.var_ref(*x) } else { Arena::ZERO })
                        .collect();
                    Ok(Val::A(dims.clone(), v))
                } else if el.iter().all(|e| matches!(e, Comp::Inst(_))) {
                    let v = el.iter().map(|e| if let Comp::Inst(x) = e { *x } else { 0 }).collect();
                    Ok(Val::IA(dims.clone(), v))
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

    fn resolve_ref(&mut self, cr: &'a ast::ComponentReference, scope: &Scope, ctx: &mut Ctx) -> Result<Val> {
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
            && let Some(c) = self.insts[inst as usize].comps.get(first).cloned()
        {
            let mut cur = c;
            for (i, p) in cr.parts.iter().enumerate() {
                if i > 0 {
                    cur = self.child_comp(cur, &p.ident.text).with_context(|| format!("in reference '{cr}'"))?;
                }
                if let Some(subs) = &p.subs {
                    cur = self.subscript_comp(cur, subs, scope, ctx)?;
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
            Comp::Var(_) => bail!("cannot access '.{name}' of a scalar"),
        }
    }

    fn lookup_constant(&mut self, cr: &'a ast::ComponentReference, scope: &Scope, ctx: &mut Ctx) -> Result<Option<Val>> {
        let parts: Vec<&str> = cr.parts.iter().map(|p| &*p.ident.text).collect();
        let env = if cr.local { self.env_of(ROOT) } else { scope.env.clone() };
        // 1) Unqualified constant visible in an enclosing class (or the function itself).
        if parts.len() == 1 {
            let mut e: Option<Rc<Env>> = Some(env.clone());
            while let Some(en) = e {
                if let Some((owner, comp)) = self.tree.find_component(en.class, parts[0])
                    && matches!(comp.variability, Variability::Constant(_) | Variability::Parameter(_))
                {
                    let oenv = Rc::new(Env { class: owner, overrides: en.overrides.clone(), parent: en.parent.clone() });
                    let v = self.class_constant(owner, comp, oenv)?;
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
        }
        // 2) Qualified: longest class prefix, then constant / enumeration literal.
        for k in (1..parts.len()).rev() {
            let Some(cls) = self.lookup_class(&env, &parts[..k]) else { continue };
            let name = parts[k];
            if k == parts.len() - 1 {
                if let Some(v) = self.enum_literal(cls, name) {
                    return Ok(Some(Val::S(self.model.arena.constant(v))));
                }
                if let Some(v) = builtin_enum_literal(&self.tree.node(cls).name, name) {
                    return Ok(Some(Val::S(self.model.arena.constant(v))));
                }
            }
            if let Some((owner, comp)) = self.tree.find_component(cls, name) {
                // Respect overrides from the current environment when the class is redeclared.
                let oenv = Rc::new(Env {
                    class: owner,
                    overrides: env.overrides.clone(),
                    parent: self.env_of(owner).parent.clone(),
                });
                let mut v = self.class_constant(owner, comp, oenv)?;
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
        Ok(None)
    }

    fn apply_subs(&mut self, v: Val, part: &'a ast::ComponentRefPart, scope: &Scope, ctx: &mut Ctx) -> Result<Val> {
        match &part.subs {
            Some(s) => self.index_val(v, s, scope, ctx),
            None => Ok(v),
        }
    }

    fn enum_literal(&self, cls: ClassId, name: &str) -> Option<f64> {
        let def = self.tree.def(cls)?;
        if let Some(i) = def.enum_literals.iter().position(|l| &*l.ident.text == name) {
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

    fn class_constant(&mut self, owner: ClassId, comp: &'a ast::Component, env: Rc<Env>) -> Result<Val> {
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
                val.unwrap_or_else(|| Err(anyhow!("constant '{}' has no value", comp.name)))
            }
        };
        self.const_in_progress.remove(&key);
        let v = r?;
        self.const_cache.insert(key, v.clone());
        Ok(v)
    }

    // ------------------------------------------------------------------
    // Function calls
    // ------------------------------------------------------------------

    fn call(&mut self, comp: &'a ast::ComponentReference, args: &'a [E], scope: &Scope, ctx: &mut Ctx) -> Result<Val> {
        let name = cref_name(comp);
        let (pos, named) = split_args(args);
        macro_rules! arg {
            ($i:expr) => {
                self.translate(pos.get($i).copied().ok_or_else(|| anyhow!("{}: missing argument {}", name, $i + 1))?, scope, ctx)?
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
        let short = name.rsplit('.').next().unwrap_or(&name).to_string();
        let is_builtin_ns = !name.contains('.') || name.starts_with("Modelica.Math.");
        if is_builtin_ns && let Some(f) = f1(&short) {
            let v = arg!(0);
            return self.map1(v, &mut |a, x| a.f1(f, x));
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
                let actual = named.iter().find(|(n, _)| *n == "actual").map(|(_, e)| *e).or(pos.first().copied());
                return self.translate(actual.ok_or_else(|| anyhow!("homotopy: missing actual"))?, scope, ctx);
            }
            "delay" => {
                self.model.warnings.push("delay() is approximated by its undelayed argument".into());
                return Ok(arg!(0));
            }
            "sample" => {
                let start = arg!(0).scalar()?;
                let interval = arg!(1).scalar()?;
                return Ok(Val::S(self.sample(start, interval)));
            }
            "semiLinear" => {
                let x = arg!(0).scalar()?;
                let k1 = arg!(1).scalar()?;
                let k2 = arg!(2).scalar()?;
                let a = &mut self.model.arena;
                let c = a.cmp(Cmp::Ge, x, Arena::ZERO);
                let p1 = a.mul(k1, x);
                let p2 = a.mul(k2, x);
                return Ok(Val::S(a.if_(c, p1, p2)));
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
                let (_, e) = arg!(0).elems()?;
                let a = &mut self.model.arena;
                let mut r = if name == "sum" { Arena::ZERO } else { Arena::ONE };
                for x in e {
                    r = if name == "sum" { a.add(r, x) } else { a.mul(r, x) };
                }
                return Ok(Val::S(r));
            }
            "size" => {
                let v = arg!(0);
                let dims = match &v {
                    Val::A(d, _) | Val::IA(d, _) => d.clone(),
                    _ => vec![],
                };
                if pos.len() > 1 {
                    let k = self.translate(pos[1], scope, ctx)?.scalar()?;
                    let k = self.eval_param_expr(k).unwrap_or(1.0) as usize;
                    let d = dims.get(k.saturating_sub(1)).copied().unwrap_or(1);
                    return Ok(Val::S(self.model.arena.constant(d as f64)));
                }
                let v: Vec<ExprId> = dims.iter().map(|&d| self.model.arena.constant(d as f64)).collect();
                return Ok(Val::A(vec![v.len()], v));
            }
            "ndims" => {
                let v = arg!(0);
                let n = match &v {
                    Val::A(d, _) | Val::IA(d, _) => d.len(),
                    _ => 0,
                };
                return Ok(Val::S(self.model.arena.constant(n as f64)));
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
                    dims.push(self.eval_param_expr(d).ok_or_else(|| anyhow!("{name}: size must be a parameter"))? as usize);
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
                    let t = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 };
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
            "cat" => {
                let k = arg!(0).scalar()?;
                let k = self.eval_param_expr(k).unwrap_or(1.0) as usize;
                let mut vals = Vec::new();
                for p in &pos[1..] {
                    vals.push(self.translate(p, scope, ctx)?);
                }
                return self.array_construct(vals, if k == 1 { ArrayConstructor::Vertical } else { ArrayConstructor::Horizontal })
                    .map(|v| match v {
                        Val::A(d, e) if k == 1 && d.len() == 2 && d[1] == 1 => Val::A(vec![d[0]], e),
                        other => other,
                    });
            }
            "Integer" | "Real" | "Boolean" => return Ok(arg!(0)),
            "String" => return Ok(Val::Str(String::new())),
            "assert" | "terminate" | "print" => return Ok(Val::S(Arena::ZERO)),
            "cardinality" => {
                self.model.warnings.push("cardinality() is not supported; assuming 1".into());
                return Ok(Val::S(Arena::ONE));
            }
            "getInstanceName" => return Ok(Val::Str(String::new())),
            _ => {}
        }
        // User-defined function or record constructor.
        let parts: Vec<&str> = comp.parts.iter().map(|p| &*p.ident.text).collect();
        let env = if comp.local { self.env_of(ROOT) } else { scope.env.clone() };
        let cls = self
            .lookup_class(&env, &parts)
            .ok_or_else(|| anyhow!("function '{}' not found", name))?;
        let mut argv = Vec::new();
        for p in &pos {
            argv.push(self.translate(p, scope, ctx)?);
        }
        let mut nargv = Vec::new();
        for (n, e) in &named {
            nargv.push((n.to_string(), self.translate(e, scope, ctx)?));
        }
        match self.tree.class_type(cls) {
            ClassType::Function => {
                let outs = self.inline_function(cls, argv, nargv, env.overrides.clone())?;
                if outs.len() == 1 { Ok(outs.into_iter().next().unwrap()) } else { Ok(Val::Tuple(outs)) }
            }
            ClassType::Record => self.record_constructor(cls, argv, nargv),
            _ => bail!("'{}' is not a function", name),
        }
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
            branches: vec![(cond, vec![WhenAction::Assign(id, upd), WhenAction::Assign(next_id, upd_next)])],
            origin: o,
        });
        self.when_assigned.insert(id);
        self.when_assigned.insert(next_id);
        let _ = next;
        cond
    }

    fn record_constructor(&mut self, cls: ClassId, argv: Vec<Val>, nargv: Vec<(String, Val)>) -> Result<Val> {
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
                let mut ctx = Ctx { in_function: true, ..Default::default() };
                ctx.locals.push(fields.iter().cloned().collect());
                self.translate(b, &Scope { inst: None, env }, &mut ctx)?
            } else {
                Val::S(Arena::ZERO)
            };
            fields.push((fname, val));
        }
        Ok(Val::Rec(fields))
    }

    fn function_elements(&self, cls: ClassId, out: &mut Vec<(String, &'a ast::Component, ClassId)>, depth: usize) {
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

    fn function_algorithms(&self, cls: ClassId, out: &mut Vec<(&'a [ast::Statement], ClassId)>, depth: usize) {
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

    fn function_external(&self, cls: ClassId, depth: usize) -> Option<(&'a ast::ExternalFunction, String)> {
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
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            bail!("function inlining depth exceeded (recursive function?)");
        }
        let fname = self.tree.node(cls).path.clone();
        let mut elems = Vec::new();
        self.function_elements(cls, &mut elems, 0);
        let env = self.env_with(cls, overrides);
        let scope = Scope { inst: None, env };
        let mut ctx = Ctx { in_function: true, ..Default::default() };
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
                ctx.locals.last_mut().unwrap().insert(n.clone(), v);
            }
        }
        // External builtin intrinsics.
        if let Some((ext, short)) = self.function_external(cls, 0) {
            let lang = ext.language.clone().unwrap_or_default();
            let ins: Vec<Val> = inputs.iter().map(|n| ctx.locals[0][n].clone()).collect();
            self.depth -= 1;
            if lang.trim_matches('"') == "builtin" {
                return self.intrinsic(&short, ins).with_context(|| format!("in builtin function '{fname}'"));
            }
            bail!("external function '{}' is not supported", fname);
        }
        // Outputs and protected locals: dims + bindings.
        for (n, c, owner) in &elems {
            if matches!(c.causality, Causality::Input(_)) {
                continue;
            }
            if matches!(c.causality, Causality::Output(_)) {
                outputs.push(n.clone());
            }
            let oenv = self.env_of(*owner);
            let oscope = Scope { inst: None, env: oenv };
            let mut dims = Vec::new();
            for s in &c.shape_expr {
                if let ast::Subscript::Expression(e) = s {
                    let d = self.translate(e, &oscope, &mut ctx)?.scalar()?;
                    dims.push(self.eval_param_expr(d).ok_or_else(|| anyhow!("function array size must be known"))? as usize);
                }
            }
            let init = if let Some(b) = &c.binding {
                self.translate(b, &oscope, &mut ctx)?
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
            let oenv = self.env_of(owner);
            let oscope = Scope { inst: None, env: oenv };
            self.exec(stmts, &oscope, &mut ctx, &mut returned)
                .with_context(|| format!("in function '{fname}'"))?;
        }
        self.depth -= 1;
        Ok(outputs.iter().map(|n| ctx.locals[0][n].clone()).collect())
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
                    data.push(self.eval_param_expr(v).ok_or_else(|| anyhow!("table data must be constant or parameter"))?);
                }
                let t = Table { rows: d[0], cols: d[1], data, extrapolation: u8::from(name == "interpolateExtrapolate") };
                let id = self.model.arena.add_table(t);
                let col0 = col.saturating_sub(1) as u32;
                Ok(vec![Val::S(self.model.arena.interp(id, col0, x))])
            }
            _ => bail!("unknown builtin function '{name}'"),
        }
    }

    fn exec(&mut self, stmts: &'a [ast::Statement], scope: &Scope, ctx: &mut Ctx, returned: &mut bool) -> Result<()> {
        for s in stmts {
            if *returned {
                return Ok(());
            }
            match s {
                ast::Statement::Empty => {}
                ast::Statement::Assignment { comp, value } => {
                    let v = self.translate(value, scope, ctx)?;
                    self.assign_local(comp, v, scope, ctx)?;
                }
                ast::Statement::FunctionCall { comp, args, outputs } => {
                    let v = self.call(comp, args, scope, ctx)?;
                    let vals = match v {
                        Val::Tuple(t) => t,
                        other => vec![other],
                    };
                    for (o, val) in outputs.iter().zip(vals) {
                        if let E::ComponentReference(cr) = o {
                            self.assign_local(cr, val, scope, ctx)?;
                        }
                    }
                }
                ast::Statement::If { cond_blocks, else_block } => {
                    self.exec_if(cond_blocks, else_block.as_deref(), scope, ctx, returned)?;
                }
                ast::Statement::For { indices, equations } => {
                    let idx = indices.first().ok_or_else(|| anyhow!("for without index"))?;
                    let range = self.translate(&idx.range, scope, ctx)?;
                    let (_, vals) = range.elems()?;
                    let name = idx.ident.text.to_string();
                    for v in vals {
                        ctx.locals.last_mut().unwrap().insert(name.clone(), Val::S(v));
                        self.exec(equations, scope, ctx, returned)?;
                        if *returned {
                            break;
                        }
                    }
                    ctx.locals.last_mut().unwrap().remove(&name);
                }
                ast::Statement::While(block) => {
                    let mut n = 0;
                    loop {
                        let c = self.translate(&block.cond, scope, ctx)?.scalar()?;
                        let cv = self.eval_param_expr(c).ok_or_else(|| {
                            anyhow!("while-loop condition must be evaluable at translation time")
                        })?;
                        if cv == 0.0 {
                            break;
                        }
                        self.exec(&block.stmts, scope, ctx, returned)?;
                        n += 1;
                        if n > 100_000 || *returned {
                            break;
                        }
                    }
                }
                ast::Statement::Return { .. } => *returned = true,
                ast::Statement::Break { .. } => return Ok(()),
                ast::Statement::Assert { .. } => {}
                ast::Statement::When(_) | ast::Statement::Reinit { .. } => {
                    bail!("when-statements are not supported inside functions")
                }
            }
        }
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
        let mut r1 = false;
        self.exec(&first.stmts, scope, ctx, &mut r1)?;
        let then_locals = std::mem::replace(&mut ctx.locals, base);
        let mut r2 = false;
        self.exec_if(rest, else_block, scope, ctx, &mut r2)?;
        if r1 || r2 {
            bail!("return inside a branch with a time-varying condition is not supported");
        }
        let else_locals = std::mem::take(&mut ctx.locals);
        let mut merged = else_locals.clone();
        for (fi, frame) in then_locals.iter().enumerate() {
            for (k, tv) in frame {
                let ev = else_locals.get(fi).and_then(|f| f.get(k)).cloned();
                let m = match ev {
                    Some(ev) => self.zip2(tv.clone(), ev, &mut |a, x, y| a.if_(c, x, y))?,
                    None => tv.clone(),
                };
                merged[fi].insert(k.clone(), m);
            }
        }
        ctx.locals = merged;
        Ok(())
    }

    fn assign_local(&mut self, target: &'a ast::ComponentReference, v: Val, scope: &Scope, ctx: &mut Ctx) -> Result<()> {
        let name = target.parts[0].ident.text.to_string();
        if target.parts.len() > 1 {
            // record field assignment r.f := v
            let frame = ctx.locals.iter().rposition(|f| f.contains_key(&name)).ok_or_else(|| anyhow!("unknown variable '{name}'"))?;
            let mut cur = ctx.locals[frame][&name].clone();
            if let Val::Rec(fields) = &mut cur {
                let f = &*target.parts[1].ident.text;
                if let Some(pos) = fields.iter().position(|(n, _)| n == f) {
                    fields[pos].1 = v;
                } else {
                    fields.push((f.to_string(), v));
                }
            }
            ctx.locals[frame].insert(name, cur);
            return Ok(());
        }
        let frame = ctx.locals.iter().rposition(|f| f.contains_key(&name)).unwrap_or(ctx.locals.len() - 1);
        match &target.parts[0].subs {
            None => {
                ctx.locals[frame].insert(name, v);
            }
            Some(subs) => {
                let cur = ctx.locals[frame].get(&name).cloned().ok_or_else(|| anyhow!("unknown array '{name}'"))?;
                let Val::A(dims, mut el) = cur else { bail!("indexed assignment to non-array '{name}'") };
                let sel = self.eval_subscripts(subs, &dims, scope, ctx)?;
                let (_, idx) = select_indices(&dims, &sel);
                let (_, vals) = v.elems()?;
                for (k, i) in idx.into_iter().enumerate() {
                    el[i] = *vals.get(k).unwrap_or(&vals[0]);
                }
                ctx.locals[frame].insert(name, Val::A(dims, el));
            }
        }
        Ok(())
    }

    /// Algorithm section in a model: symbolic execution gives one equation per assigned variable.
    fn translate_algorithm(&mut self, stmts: &'a [ast::Statement], scope: &Scope, initial: bool) -> Result<()> {
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
                    for st in &b.stmts {
                        match st {
                            ast::Statement::Assignment { comp, value } => {
                                let t = self.resolve_ref(comp, scope, &mut ctx)?;
                                let v = self.translate(value, scope, &mut ctx)?;
                                let (_, te) = t.elems()?;
                                let (_, ve) = v.elems()?;
                                for (i, x) in te.iter().enumerate() {
                                    if let Node::Var(var) = self.model.arena.node(*x) {
                                        self.when_assigned.insert(var);
                                        acts.push(WhenAction::Assign(var, *ve.get(i).unwrap_or(&ve[0])));
                                    }
                                }
                            }
                            ast::Statement::Reinit { variable, value } => {
                                let t = self.resolve_ref(variable, scope, &mut ctx)?.scalar()?;
                                let v = self.translate(value, scope, &mut ctx)?.scalar()?;
                                if let Node::Var(var) = self.model.arena.node(t) {
                                    acts.push(WhenAction::Reinit(var, v));
                                }
                            }
                            ast::Statement::FunctionCall { comp, args, .. } if cref_name(comp) == "reinit" => {
                                let t = self.translate(&args[0], scope, &mut ctx)?.scalar()?;
                                let v = self.translate(&args[1], scope, &mut ctx)?.scalar()?;
                                if let Node::Var(var) = self.model.arena.node(t) {
                                    acts.push(WhenAction::Reinit(var, v));
                                }
                            }
                            _ => {}
                        }
                    }
                    branches.push((c, acts));
                }
                let o = self.eq_origin(scope, "when-statement");
                self.model.whens.push(WhenClause { branches, origin: o });
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
                parts: vec![ast::ComponentRefPart { ident: rumoca_core::Token { text: t.as_str().into(), ..Default::default() }, subs: None, def_id: None }],
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
            ast::Statement::If { cond_blocks, else_block } => {
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

fn select_mod_element<'a>(m: &Mod<'a>, _k: usize, _n: usize) -> Mod<'a> {
    // Modifiers on arrays of structured components are applied to each element
    // (`each` semantics); array-valued distribution is handled at the leaves.
    m.clone()
}

fn dot(prefix: &str) -> String {
    if prefix.is_empty() { String::new() } else { format!("{prefix}.") }
}

fn index_str(dims: &[usize], mut k: usize) -> String {
    let mut idx = vec![0; dims.len()];
    for d in (0..dims.len()).rev() {
        idx[d] = k % dims[d] + 1;
        k /= dims[d];
    }
    idx.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
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
    let new_dims: Vec<usize> = per_dim.iter().filter(|(keep, _)| *keep).map(|(_, v)| v.len()).collect();
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
    c.parts.iter().map(|p| p.ident.text.to_string()).collect::<Vec<_>>().join(".")
}

fn expr_string(e: &E) -> String {
    match e {
        E::Terminal { token, terminal_type: TerminalType::String, .. } => token.text.trim_matches('"').to_string(),
        E::Binary { lhs, rhs, .. } => format!("{}{}", expr_string(lhs), expr_string(rhs)),
        _ => String::new(),
    }
}

pub fn literal_number(e: &E) -> Option<f64> {
    match e {
        E::Terminal { token, terminal_type: TerminalType::UnsignedReal | TerminalType::UnsignedInteger, .. } => {
            token.text.parse().ok()
        }
        E::Unary { op: OpUnary::Minus, rhs, .. } => literal_number(rhs).map(|v| -v),
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
