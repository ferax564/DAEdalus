//! Bipartite matching (Hopcroft–Karp) and strongly connected components (Tarjan).
//! Both are iterative and linear-ish so they scale to 10^6-equation systems.

/// Incidence: for each equation, the list of unknown indices it contains.
pub struct Bipartite<'a> {
    pub n_eq: usize,
    pub n_var: usize,
    pub adj: &'a [Vec<u32>],
}

pub const NONE: u32 = u32::MAX;

/// Maximum matching. Returns (eq -> var, var -> eq).
pub fn hopcroft_karp(g: &Bipartite) -> (Vec<u32>, Vec<u32>) {
    let mut eq_match = vec![NONE; g.n_eq];
    let mut var_match = vec![NONE; g.n_var];
    // Greedy initialisation: cheap and gets most of the matching.
    for e in 0..g.n_eq {
        for &v in &g.adj[e] {
            if var_match[v as usize] == NONE {
                var_match[v as usize] = e as u32;
                eq_match[e] = v;
                break;
            }
        }
    }
    let mut dist = vec![0u32; g.n_eq];
    let mut queue = Vec::with_capacity(g.n_eq);
    loop {
        // BFS from free equations
        queue.clear();
        let inf = u32::MAX;
        for e in 0..g.n_eq {
            if eq_match[e] == NONE {
                dist[e] = 0;
                queue.push(e as u32);
            } else {
                dist[e] = inf;
            }
        }
        let mut found = false;
        let mut qi = 0;
        while qi < queue.len() {
            let e = queue[qi] as usize;
            qi += 1;
            for &v in &g.adj[e] {
                let m = var_match[v as usize];
                if m == NONE {
                    found = true;
                } else if dist[m as usize] == inf {
                    dist[m as usize] = dist[e] + 1;
                    queue.push(m);
                }
            }
        }
        if !found {
            break;
        }
        // DFS (iterative) for augmenting paths along layered graph
        let mut it = vec![0usize; g.n_eq];
        let mut augmented = false;
        for start in 0..g.n_eq {
            if eq_match[start] != NONE {
                continue;
            }
            // stack of equations
            let mut stack: Vec<usize> = vec![start];
            while let Some(&e) = stack.last() {
                if it[e] >= g.adj[e].len() {
                    dist[e] = u32::MAX;
                    stack.pop();
                    continue;
                }
                let v = g.adj[e][it[e]] as usize;
                it[e] += 1;
                let m = var_match[v];
                if m == NONE {
                    // augment along stack
                    let mut vv = v as u32;
                    for &se in stack.iter().rev() {
                        let prev = eq_match[se];
                        eq_match[se] = vv;
                        var_match[vv as usize] = se as u32;
                        vv = prev;
                    }
                    augmented = true;
                    break;
                } else if dist[m as usize] == dist[e] + 1 {
                    stack.push(m as usize);
                }
            }
        }
        if !augmented {
            break;
        }
    }
    (eq_match, var_match)
}

/// Try to find an augmenting path from equation `e` (simple DFS, used incrementally).
/// Variables for which `active` is false are ignored (Pantelides' differentiated variables).
/// Visited equations/variables are marked with `stamp`.
pub fn augment(
    e: usize,
    adj: &[Vec<u32>],
    eq_match: &mut [u32],
    var_match: &mut [u32],
    visited_eq: &mut [u32],
    visited_var: &mut [u32],
    stamp: u32,
    active: &dyn Fn(u32) -> bool,
) -> bool {
    augment_traced(
        e,
        adj,
        eq_match,
        var_match,
        visited_eq,
        visited_var,
        stamp,
        active,
        None,
    )
}

/// [`augment`] that also records the equations and variables it visits (in `trail`), so a
/// caller can read the visited set of a failed search without scanning every equation.
#[allow(clippy::too_many_arguments)]
pub fn augment_traced(
    e: usize,
    adj: &[Vec<u32>],
    eq_match: &mut [u32],
    var_match: &mut [u32],
    visited_eq: &mut [u32],
    visited_var: &mut [u32],
    stamp: u32,
    active: &dyn Fn(u32) -> bool,
    mut trail: Option<(&mut Vec<usize>, &mut Vec<u32>)>,
) -> bool {
    if let Some((te, _)) = trail.as_mut() {
        te.push(e);
    }
    // Iterative DFS with explicit stack: (eq, next edge index)
    let mut stack: Vec<(usize, usize)> = vec![(e, 0)];
    visited_eq[e] = stamp;
    loop {
        let Some(&(cur, idx)) = stack.last() else {
            return false;
        };
        if idx == 0
            && let Some(&v) = adj[cur]
                .iter()
                .find(|&&v| var_match[v as usize] == NONE && active(v))
        {
            let mut vv = v;
            for &(se, _) in stack.iter().rev() {
                let prev = eq_match[se];
                eq_match[se] = vv;
                var_match[vv as usize] = se as u32;
                vv = prev;
            }
            visited_var[v as usize] = stamp;
            return true;
        }
        if idx >= adj[cur].len() {
            stack.pop();
            continue;
        }
        stack.last_mut().unwrap().1 += 1;
        let v = adj[cur][idx] as usize;
        if visited_var[v] == stamp || !active(v as u32) {
            continue;
        }
        visited_var[v] = stamp;
        if let Some((_, tv)) = trail.as_mut() {
            tv.push(v as u32);
        }
        let m = var_match[v];
        if m != NONE && visited_eq[m as usize] != stamp {
            visited_eq[m as usize] = stamp;
            if let Some((te, _)) = trail.as_mut() {
                te.push(m as usize);
            }
            stack.push((m as usize, 0));
        }
    }
}

/// Tarjan SCC over equations of a matched system. Equation `e` depends on equation
/// `match(v)` for every unknown `v` in `adj[e]`. Returns components in evaluation
/// order (dependencies first).
pub fn tarjan_blt(adj: &[Vec<u32>], eq_of_var: &[u32]) -> Vec<Vec<u32>> {
    let n = adj.len();
    let mut index = vec![u32::MAX; n];
    let mut low = vec![0u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut counter = 0u32;
    let mut comps = Vec::new();
    let mut call: Vec<(u32, usize)> = Vec::new();
    for root in 0..n {
        if index[root] != u32::MAX {
            continue;
        }
        call.push((root as u32, 0));
        while let Some(&mut (v, ref mut ei)) = call.last_mut() {
            let vu = v as usize;
            if *ei == 0 && index[vu] == u32::MAX {
                index[vu] = counter;
                low[vu] = counter;
                counter += 1;
                stack.push(v);
                on_stack[vu] = true;
            }
            let mut pushed = false;
            while *ei < adj[vu].len() {
                let var = adj[vu][*ei] as usize;
                *ei += 1;
                let w = eq_of_var[var];
                if w == NONE || w == v {
                    continue;
                }
                let wu = w as usize;
                if index[wu] == u32::MAX {
                    call.push((w, 0));
                    pushed = true;
                    break;
                } else if on_stack[wu] {
                    low[vu] = low[vu].min(index[wu]);
                }
            }
            if pushed {
                continue;
            }
            // finished v
            call.pop();
            if let Some(&(p, _)) = call.last() {
                let pu = p as usize;
                low[pu] = low[pu].min(low[vu]);
            }
            if low[vu] == index[vu] {
                let mut comp = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w as usize] = false;
                    comp.push(w);
                    if w == v {
                        break;
                    }
                }
                comps.push(comp);
            }
        }
    }
    comps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_and_blt() {
        // e0: x0 ; e1: x0, x1 ; e2: x1, x2 ; e3: x2, x3 ; e4: x3, x2
        let adj = vec![vec![0], vec![0, 1], vec![1, 2], vec![2, 3], vec![3, 2]];
        let g = Bipartite {
            n_eq: 5,
            n_var: 4,
            adj: &adj,
        };
        let (em, vm) = hopcroft_karp(&g);
        assert_eq!(em.iter().filter(|&&v| v != NONE).count(), 4);
        let _ = vm;
        let adj2 = vec![vec![0], vec![0, 1], vec![1, 2, 3], vec![2, 3]];
        let g2 = Bipartite {
            n_eq: 4,
            n_var: 4,
            adj: &adj2,
        };
        let (_em, vm) = hopcroft_karp(&g2);
        let comps = tarjan_blt(&adj2, &vm);
        assert_eq!(comps.len(), 3);
        assert_eq!(comps[0], vec![0]);
        assert_eq!(comps[2].len(), 2);
    }
}
