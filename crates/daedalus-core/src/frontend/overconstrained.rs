//! Overconstrained connection graphs (MLS §9.4): `Connections.branch/root/potentialRoot`,
//! `isRoot` and `rooted`, as used by MultiBody frames and quasi-static references.
//!
//! Flattening records the graph ([`OcLog`]) on a first run. [`OcAnswers::compute`] builds a
//! spanning forest with one root per component; a second run then answers `isRoot`/`rooted`
//! and replaces the potential equalities of loop-closing connections by the type's
//! `equalityConstraint`. Nodes are overconstrained record instances, named by their full
//! instance path, which is the same in both runs.

use rustc_hash::{FxHashMap, FxHashSet};

/// The connection graph as seen while flattening.
#[derive(Default, Debug, Clone)]
pub struct OcLog {
    /// `Connections.branch(a, b)`: edges that must be part of the spanning tree.
    pub branches: Vec<(String, String)>,
    /// `Connections.root(a)`: definite roots.
    pub roots: Vec<String>,
    /// `Connections.potentialRoot(a, priority)`.
    pub potential: Vec<(String, i64)>,
    /// `connect` between connectors with overconstrained records: breakable edges.
    pub connects: Vec<(String, String)>,
    /// Some `Connections.*` operator or overconstrained connection was seen.
    pub used: bool,
}

/// Spanning-forest answers used by the second flattening run.
#[derive(Default, Debug, Clone)]
pub struct OcAnswers {
    pub roots: FxHashSet<String>,
    /// Connect edges that close a loop (or join two rooted trees), normalised (min, max).
    pub broken: FxHashSet<(String, String)>,
    pub depth: FxHashMap<String, usize>,
    branch_partner: FxHashMap<String, Vec<String>>,
}

pub fn edge_key(a: &str, b: &str) -> (String, String) {
    if a <= b { (a.to_string(), b.to_string()) } else { (b.to_string(), a.to_string()) }
}

impl OcAnswers {
    pub fn compute<'l>(log: &'l OcLog) -> OcAnswers {
        let mut ids: FxHashMap<&'l str, usize> = FxHashMap::default();
        let mut names: Vec<&'l str> = Vec::new();
        let mut intern = |s: &'l str| -> usize {
            *ids.entry(s).or_insert_with(|| {
                names.push(s);
                names.len() - 1
            })
        };
        let branches: Vec<(usize, usize)> =
            log.branches.iter().map(|(a, b)| (intern(a), intern(b))).collect();
        let connects: Vec<(usize, usize)> =
            log.connects.iter().map(|(a, b)| (intern(a), intern(b))).collect();
        let roots: Vec<usize> = log.roots.iter().map(|a| intern(a)).collect();
        let potential: Vec<(usize, i64)> = log.potential.iter().map(|(a, p)| (intern(a), *p)).collect();
        let n = names.len();
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        let mut has_root = vec![false; n];
        for &r in &roots {
            has_root[r] = true;
        }
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut out = OcAnswers::default();
        // Branches are never broken.
        for &(a, b) in &branches {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra] = rb;
                has_root[rb] |= has_root[ra];
            }
            adj[a].push(b);
            adj[b].push(a);
            out.branch_partner.entry(names[a].to_string()).or_default().push(names[b].to_string());
            out.branch_partner.entry(names[b].to_string()).or_default().push(names[a].to_string());
        }
        // Connections join trees unless they would close a loop or join two rooted trees.
        for &(a, b) in &connects {
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra == rb || (has_root[ra] && has_root[rb]) {
                out.broken.insert(edge_key(names[a], names[b]));
                continue;
            }
            parent[ra] = rb;
            has_root[rb] |= has_root[ra];
            adj[a].push(b);
            adj[b].push(a);
        }
        let mut root_nodes: Vec<usize> = roots.clone();
        // One potential root (lowest priority value, first declared) per unrooted tree.
        let mut best: FxHashMap<usize, (i64, usize)> = FxHashMap::default();
        for (k, &(a, p)) in potential.iter().enumerate() {
            let r = find(&mut parent, a);
            if has_root[r] {
                continue;
            }
            let e = best.entry(r).or_insert((p, k));
            if p < e.0 {
                *e = (p, k);
            }
        }
        let mut chosen: Vec<(usize, usize)> = best.into_values().map(|(_, k)| (k, potential[k].0)).collect();
        chosen.sort_unstable();
        root_nodes.extend(chosen.into_iter().map(|(_, a)| a));
        // Depth from the roots along tree edges.
        let mut depth = vec![usize::MAX; n];
        let mut queue = std::collections::VecDeque::new();
        for &r in &root_nodes {
            if depth[r] == usize::MAX {
                depth[r] = 0;
                queue.push_back(r);
            }
            out.roots.insert(names[r].to_string());
        }
        while let Some(x) = queue.pop_front() {
            for &y in &adj[x] {
                if depth[y] == usize::MAX {
                    depth[y] = depth[x] + 1;
                    queue.push_back(y);
                }
            }
        }
        for (i, &d) in depth.iter().enumerate() {
            if d != usize::MAX {
                out.depth.insert(names[i].to_string(), d);
            }
        }
        out
    }

    pub fn is_root(&self, node: &str) -> bool {
        self.roots.contains(node)
    }

    /// `Connections.rooted(a)`: `a` is closer to the root than its branch partner.
    pub fn rooted(&self, node: &str) -> bool {
        let d = self.depth.get(node).copied().unwrap_or(usize::MAX);
        match self.branch_partner.get(node) {
            Some(ps) => ps.iter().all(|p| d < self.depth.get(p).copied().unwrap_or(usize::MAX)),
            None => self.is_root(node),
        }
    }

    pub fn is_broken(&self, a: &str, b: &str) -> bool {
        self.broken.contains(&edge_key(a, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bar_loop_breaks_one_connection() {
        // world -- j1 -- b1 -- j2 -- b2 -- j3 -- world (a loop through two joints)
        let s = |x: &str| x.to_string();
        let log = OcLog {
            branches: vec![(s("j1.a"), s("j1.b")), (s("j2.a"), s("j2.b")), (s("j3.a"), s("j3.b"))],
            roots: vec![s("world")],
            potential: vec![(s("b1"), 0), (s("b2"), 0)],
            connects: vec![
                (s("world"), s("j1.a")),
                (s("j1.b"), s("b1")),
                (s("b1"), s("j2.a")),
                (s("j2.b"), s("b2")),
                (s("b2"), s("j3.a")),
                (s("j3.b"), s("world")),
            ],
            used: true,
        };
        let a = OcAnswers::compute(&log);
        assert!(a.is_root("world") && !a.is_root("b1") && !a.is_root("b2"));
        assert_eq!(a.broken.len(), 1);
        assert!(a.is_broken("j3.b", "world"));
        assert!(a.rooted("j1.a") && !a.rooted("j1.b"));
    }

    #[test]
    fn free_body_takes_potential_root() {
        let s = |x: &str| x.to_string();
        let log = OcLog { potential: vec![(s("body.frame_a"), 0)], used: true, ..Default::default() };
        let a = OcAnswers::compute(&log);
        assert!(a.is_root("body.frame_a"));
    }
}
