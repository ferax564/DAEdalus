//! User overrides of `stateSelect`, applied to the flat model before alias elimination
//! and index reduction, so a different set of states can be tried without editing the
//! model (`daedalus simulate --state body.v=prefer`, the workbench's Structure view).
//!
//! An override names a variable exactly or by a glob pattern (`*` any run of
//! characters, `?` one character): `arm.*.phi=avoid`. Later overrides win over earlier
//! ones. Only continuous Real variables are affected.

use crate::flat::{StateSelect, VarKind, VarType, Variable};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// `pattern = tier`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateOverride {
    pub pattern: String,
    pub tier: StateSelect,
}

/// A variable whose `stateSelect` an override changed.
#[derive(Clone, Debug, Serialize)]
pub struct AppliedOverride {
    pub var: String,
    pub pattern: String,
    /// The model's own value.
    pub model: StateSelect,
    pub tier: StateSelect,
}

/// Parse a tier name (`prefer`, `StateSelect.prefer`; case-insensitive).
pub fn parse_tier(s: &str) -> Option<StateSelect> {
    let s = s.trim();
    let s = s.strip_prefix("StateSelect.").unwrap_or(s);
    Some(match s.to_ascii_lowercase().as_str() {
        "never" => StateSelect::Never,
        "avoid" => StateSelect::Avoid,
        "default" => StateSelect::Default,
        "prefer" => StateSelect::Prefer,
        "always" => StateSelect::Always,
        _ => return None,
    })
}

/// The Modelica name of a tier (`prefer`).
pub fn tier_name(t: StateSelect) -> &'static str {
    match t {
        StateSelect::Never => "never",
        StateSelect::Avoid => "avoid",
        StateSelect::Default => "default",
        StateSelect::Prefer => "prefer",
        StateSelect::Always => "always",
    }
}

impl StateOverride {
    /// Parse `NAME=tier` (the CLI's `--state` argument).
    pub fn parse(s: &str) -> Result<StateOverride> {
        let Some((name, tier)) = s.rsplit_once('=') else {
            bail!("expected NAME=prefer|avoid|always|never|default, got '{s}'");
        };
        let Some(tier) = parse_tier(tier) else {
            bail!("unknown stateSelect '{tier}' (always | prefer | default | avoid | never)");
        };
        let pattern = name.trim().to_string();
        if pattern.is_empty() {
            bail!("empty variable name in '{s}'");
        }
        Ok(StateOverride { pattern, tier })
    }

    pub fn matches(&self, name: &str) -> bool {
        glob_match(self.pattern.as_bytes(), name.as_bytes())
    }
}

/// Glob match with `*` (any run, dots included) and `?` (one character).
pub fn glob_match(p: &[u8], s: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some((pi, si));
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

/// Set the `stateSelect` of the matching continuous Real variables. Returns what changed;
/// an override that matches no such variable adds a warning.
pub fn apply(
    vars: &mut [Variable],
    overrides: &[StateOverride],
    warnings: &mut Vec<String>,
) -> Vec<AppliedOverride> {
    let mut applied: Vec<AppliedOverride> = Vec::new();
    if overrides.is_empty() {
        return applied;
    }
    let mut hit = vec![false; overrides.len()];
    for v in vars.iter_mut() {
        if v.kind != VarKind::Continuous || v.ty != VarType::Real {
            continue;
        }
        let Some(k) = (0..overrides.len())
            .rev()
            .find(|&k| overrides[k].matches(&v.name))
        else {
            continue;
        };
        hit[k] = true;
        let o = &overrides[k];
        applied.push(AppliedOverride {
            var: v.name.clone(),
            pattern: o.pattern.clone(),
            model: v.state_select,
            tier: o.tier,
        });
        v.state_select = o.tier;
    }
    for (o, h) in overrides.iter().zip(hit) {
        if !h {
            warnings.push(format!(
                "state override '{}={}' matches no continuous Real variable",
                o.pattern,
                tier_name(o.tier)
            ));
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match(b"a.b", b"a.b"));
        assert!(!glob_match(b"a.b", b"a.bc"));
        assert!(glob_match(b"a.*", b"a.b.c"));
        assert!(glob_match(b"*.phi", b"inertia1.phi"));
        assert!(!glob_match(b"*.phi", b"inertia1.phi2"));
        assert!(glob_match(b"x[?]", b"x[2]"));
        assert!(glob_match(b"*", b""));
    }

    #[test]
    fn parse() {
        let o = StateOverride::parse("body.v=StateSelect.prefer").unwrap();
        assert_eq!(o.pattern, "body.v");
        assert_eq!(o.tier, StateSelect::Prefer);
        assert!(StateOverride::parse("x=maybe").is_err());
        assert!(StateOverride::parse("x").is_err());
    }
}
