//! Class tree over all loaded source files, with Modelica name lookup.

use rumoca_ir_ast as ast;
use rustc_hash::FxHashMap;

pub type ClassId = u32;

pub struct ClassNode<'a> {
    pub name: String,
    pub path: String,
    pub def: Option<&'a ast::ClassDef>,
    pub parent: Option<ClassId>,
    pub children: FxHashMap<String, ClassId>,
    /// Declaration order of children (for browsing).
    pub order: Vec<ClassId>,
    /// Index of the source document that defines this class.
    pub source: usize,
}

pub struct ClassTree<'a> {
    pub nodes: Vec<ClassNode<'a>>,
    /// Memoised resolution of extends clauses: class -> base classes.
    bases_cache: std::cell::RefCell<FxHashMap<ClassId, Vec<ClassId>>>,
}

pub const ROOT: ClassId = 0;

impl<'a> ClassTree<'a> {
    pub fn build(docs: &[(&'a ast::StoredDefinition, usize)]) -> Self {
        let mut t = ClassTree {
            nodes: vec![ClassNode {
                name: String::new(),
                path: String::new(),
                def: None,
                parent: None,
                children: FxHashMap::default(),
                order: Vec::new(),
                source: usize::MAX,
            }],
            bases_cache: Default::default(),
        };
        // Insert documents with shorter `within` paths first so that package.mo
        // files establish their package before children land in it.
        let mut sorted: Vec<_> = docs.to_vec();
        sorted.sort_by_key(|(d, _)| d.within.as_ref().map(|w| w.name.len()).unwrap_or(0));
        for (doc, src) in sorted {
            let mut parent = ROOT;
            if let Some(w) = &doc.within {
                for tok in &w.name {
                    parent = t.ensure_child(parent, &tok.text, src);
                }
            }
            for (name, cd) in &doc.classes {
                t.insert(parent, name, cd, src);
            }
        }
        t
    }

    fn ensure_child(&mut self, parent: ClassId, name: &str, source: usize) -> ClassId {
        if let Some(&c) = self.nodes[parent as usize].children.get(name) {
            return c;
        }
        let id = self.nodes.len() as ClassId;
        let path = if parent == ROOT {
            name.to_string()
        } else {
            format!("{}.{}", self.nodes[parent as usize].path, name)
        };
        self.nodes.push(ClassNode {
            name: name.to_string(),
            path,
            def: None,
            parent: Some(parent),
            children: FxHashMap::default(),
            order: Vec::new(),
            source,
        });
        self.nodes[parent as usize]
            .children
            .insert(name.to_string(), id);
        self.nodes[parent as usize].order.push(id);
        id
    }

    fn insert(&mut self, parent: ClassId, name: &str, cd: &'a ast::ClassDef, source: usize) {
        let id = self.ensure_child(parent, name, source);
        self.nodes[id as usize].def = Some(cd);
        self.nodes[id as usize].source = source;
        for (n, c) in &cd.classes {
            self.insert(id, n, c, source);
        }
    }

    pub fn node(&self, id: ClassId) -> &ClassNode<'a> {
        &self.nodes[id as usize]
    }

    pub fn def(&self, id: ClassId) -> Option<&'a ast::ClassDef> {
        self.nodes[id as usize].def
    }

    /// Find a class by its full dotted path.
    pub fn find_path(&self, path: &str) -> Option<ClassId> {
        let mut cur = ROOT;
        for part in path.split('.') {
            cur = *self.nodes[cur as usize].children.get(part)?;
        }
        Some(cur)
    }

    /// Resolve the base classes of `id` (extends clauses), memoised.
    pub fn bases(&self, id: ClassId) -> Vec<ClassId> {
        if let Some(b) = self.bases_cache.borrow().get(&id) {
            return b.clone();
        }
        // Insert a placeholder to break cycles.
        self.bases_cache.borrow_mut().insert(id, Vec::new());
        let mut out = Vec::new();
        if let Some(def) = self.def(id) {
            for ext in &def.extends {
                let parts: Vec<&str> = ext.base_name.name.iter().map(|t| &*t.text).collect();
                // Extends names are looked up in the enclosing scope of the class
                // (excluding the class itself to avoid self-reference).
                let scope = self.node(id).parent.unwrap_or(ROOT);
                if let Some(b) = self.lookup(scope, &parts, Some(id)) {
                    out.push(b);
                }
            }
        }
        self.bases_cache.borrow_mut().insert(id, out.clone());
        out
    }

    /// Look up a nested class member `name` of class `id`, including inherited ones.
    pub fn member_class(&self, id: ClassId, name: &str) -> Option<ClassId> {
        self.member_class_depth(id, name, 0)
    }

    fn member_class_depth(&self, id: ClassId, name: &str, depth: usize) -> Option<ClassId> {
        if depth > 32 {
            return None;
        }
        if let Some(&c) = self.nodes[id as usize].children.get(name) {
            return Some(c);
        }
        for b in self.bases(id) {
            if let Some(c) = self.member_class_depth(b, name, depth + 1) {
                return Some(c);
            }
        }
        // A short class definition `package P = Q` exposes Q's members.
        None
    }

    /// Resolve a (possibly dotted) class name starting the lookup in `scope`,
    /// walking outwards through enclosing classes, their imports, and finally the root.
    /// `skip` is a class excluded from the first-identifier match (used for extends).
    pub fn lookup(&self, scope: ClassId, parts: &[&str], skip: Option<ClassId>) -> Option<ClassId> {
        let first = *parts.first()?;
        let mut cur = Some(scope);
        let mut found = None;
        while let Some(s) = cur {
            if let Some(c) = self.member_class(s, first)
                && Some(c) != skip
            {
                found = Some(c);
                break;
            }
            if let Some(c) = self.lookup_import(s, first) {
                found = Some(c);
                break;
            }
            // Encapsulated classes stop the lexical search (except at root).
            if self.def(s).is_some_and(|d| d.encapsulated) {
                if let Some(&c) = self.nodes[ROOT as usize].children.get(first) {
                    found = Some(c);
                }
                break;
            }
            cur = self.node(s).parent;
        }
        let mut c = found?;
        for p in &parts[1..] {
            c = self.member_class(c, p)?;
        }
        Some(c)
    }

    pub fn lookup_import(&self, scope: ClassId, name: &str) -> Option<ClassId> {
        let def = self.def(scope)?;
        for imp in &def.imports {
            match imp {
                ast::Import::Qualified { path, .. } => {
                    if path.name.last().is_some_and(|t| &*t.text == name) {
                        return self.find_tokens(&path.name);
                    }
                }
                ast::Import::Renamed { alias, path, .. } => {
                    if &*alias.text == name {
                        return self.find_tokens(&path.name);
                    }
                }
                ast::Import::Unqualified { path, .. } => {
                    if let Some(p) = self.find_tokens(&path.name)
                        && let Some(c) = self.member_class(p, name)
                    {
                        return Some(c);
                    }
                }
                ast::Import::Selective { path, names, .. } => {
                    if names.iter().any(|t| &*t.text == name)
                        && let Some(p) = self.find_tokens(&path.name)
                    {
                        return self.member_class(p, name);
                    }
                }
            }
        }
        None
    }

    /// Import whose target is a component (constant) rather than a class:
    /// returns (package class, constant name).
    pub fn lookup_import_constant(&self, scope: ClassId, name: &str) -> Option<(ClassId, String)> {
        let mut cur = Some(scope);
        while let Some(s) = cur {
            if let Some(def) = self.def(s) {
                for imp in &def.imports {
                    match imp {
                        ast::Import::Qualified { path, .. } => {
                            if path.name.last().is_some_and(|t| &*t.text == name)
                                && path.name.len() >= 2
                            {
                                let pkg = self.find_tokens(&path.name[..path.name.len() - 1])?;
                                return Some((pkg, name.to_string()));
                            }
                        }
                        ast::Import::Renamed { alias, path, .. } => {
                            if &*alias.text == name && path.name.len() >= 2 {
                                let pkg = self.find_tokens(&path.name[..path.name.len() - 1])?;
                                let last = path.name.last()?.text.to_string();
                                return Some((pkg, last));
                            }
                        }
                        ast::Import::Unqualified { path, .. } => {
                            if let Some(p) = self.find_tokens(&path.name)
                                && self.has_component(p, name)
                            {
                                return Some((p, name.to_string()));
                            }
                        }
                        ast::Import::Selective { path, names, .. } => {
                            if names.iter().any(|t| &*t.text == name)
                                && let Some(p) = self.find_tokens(&path.name)
                            {
                                return Some((p, name.to_string()));
                            }
                        }
                    }
                }
            }
            cur = self.node(s).parent;
        }
        None
    }

    pub fn has_component(&self, id: ClassId, name: &str) -> bool {
        if self
            .def(id)
            .is_some_and(|d| d.components.contains_key(name))
        {
            return true;
        }
        self.bases(id)
            .into_iter()
            .any(|b| self.has_component(b, name))
    }

    /// Find the declaration of component `name` in class `id` or its bases.
    pub fn find_component(&self, id: ClassId, name: &str) -> Option<(ClassId, &'a ast::Component)> {
        if let Some(d) = self.def(id)
            && let Some(c) = d.components.get(name)
        {
            return Some((id, c));
        }
        for b in self.bases(id) {
            if let Some(r) = self.find_component(b, name) {
                return Some(r);
            }
        }
        None
    }

    fn find_tokens(&self, toks: &[rumoca_core::Token]) -> Option<ClassId> {
        let mut cur = ROOT;
        for t in toks {
            cur = self.member_class(cur, &t.text)?;
        }
        Some(cur)
    }

    /// The "restriction" (model, connector, ...) of a class, following short-class
    /// definitions.
    pub fn class_type(&self, id: ClassId) -> rumoca_core::ClassType {
        self.def(id)
            .map(|d| d.class_type.clone())
            .unwrap_or(rumoca_core::ClassType::Package)
    }

    /// If `id` (transitively) extends a builtin scalar type, return its name.
    pub fn builtin_base(&self, id: ClassId) -> Option<&'static str> {
        self.builtin_base_depth(id, 0)
    }

    fn builtin_base_depth(&self, id: ClassId, depth: usize) -> Option<&'static str> {
        if depth > 32 {
            return None;
        }
        let def = self.def(id)?;
        if !def.enum_literals.is_empty() {
            return Some("enumeration");
        }
        for ext in &def.extends {
            if ext.base_name.name.len() == 1
                && let Some(b) = builtin_name(&ext.base_name.name[0].text)
            {
                return Some(b);
            }
        }
        for b in self.bases(id) {
            if let Some(r) = self.builtin_base_depth(b, depth + 1) {
                return Some(r);
            }
        }
        None
    }

    /// Walk all classes depth-first (for browsing / search).
    pub fn walk(&self, f: &mut dyn FnMut(ClassId)) {
        let mut stack: Vec<ClassId> = self.nodes[ROOT as usize]
            .order
            .iter()
            .rev()
            .copied()
            .collect();
        while let Some(c) = stack.pop() {
            f(c);
            stack.extend(self.nodes[c as usize].order.iter().rev().copied());
        }
    }
}

pub fn builtin_name(s: &str) -> Option<&'static str> {
    match s {
        "Real" => Some("Real"),
        "Integer" => Some("Integer"),
        "Boolean" => Some("Boolean"),
        "String" => Some("String"),
        "StateSelect" => Some("StateSelect"),
        "AssertionLevel" => Some("AssertionLevel"),
        "ExternalObject" => Some("ExternalObject"),
        "Clock" => Some("Clock"),
        _ => None,
    }
}
