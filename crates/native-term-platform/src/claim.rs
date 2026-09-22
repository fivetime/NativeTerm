//! Which tab belongs to which NativeTerm session (see "Claiming tabs" in
//! `docs/ARCHITECTURE.md`). Pure logic over what UIA reports; no OS calls.
//!
//! A tab is claimed by the first rule that matches:
//! 1. its name is a session label;
//! 2. it is the selected tab and one of its panes' titles is a label;
//! 3. it carried a claim in the previous scan of the window, found by
//!    aligning the previous full tab list with the current one.

use std::collections::{HashMap, HashSet};

use crate::{Claim, Rect, TabView, WindowId};

/// A pane of the selected tab (`TermControl`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    /// Live terminal title (HelpText); the label for a NativeTerm pane.
    pub title: String,
    /// Profile name (Name).
    pub profile: String,
}

/// One window's tabs as read through UIA.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowTabs {
    /// Every tab name in strip order, including tabs scrolled away.
    pub names: Vec<String>,
    /// Rectangles of realized tabs, aligned with `names`.
    pub rects: Vec<Option<Rect>>,
    pub selected: Option<usize>,
    /// Panes of the selected tab; empty if unknown.
    pub panes: Vec<Pane>,
}

/// Title prefixes an elevated console adds ("Administrator: ").
const ADMIN_PREFIXES: &[&str] = &[
    "Administrator: ",
    "管理员: ",
    "系統管理員: ",
    "管理者: ",
    "관리자: ",
    "Administrador: ",
    "Administrateur : ",
    "Amministratore: ",
    "Администратор: ",
];

pub fn strip_admin_prefix(title: &str) -> &str {
    ADMIN_PREFIXES.iter().find_map(|p| title.strip_prefix(p)).unwrap_or(title)
}

/// Remembers each window's previous tab list and claims.
#[derive(Default)]
pub struct Claimer {
    windows: HashMap<WindowId, Vec<(String, Option<Claim>)>>,
}

impl Claimer {
    pub fn new() -> Claimer {
        Claimer::default()
    }

    /// Claim the tabs of window `key` for the open sessions' `labels`.
    pub fn claim(&mut self, key: WindowId, tabs: &WindowTabs, labels: &HashSet<String>) -> Vec<TabView> {
        let names = &tabs.names;
        let previous = self.windows.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let mut claims: Vec<Option<Claim>> =
            carry(previous, names).into_iter().map(|c| c.filter(|c| labels.contains(&c.label))).collect();

        // rule 1: the exact name is the strongest evidence
        for (k, name) in names.iter().enumerate() {
            let label = strip_admin_prefix(name);
            if !labels.contains(label) {
                continue;
            }
            for (j, other) in claims.iter_mut().enumerate() {
                if j != k && other.as_ref().is_some_and(|c| c.label == label) {
                    *other = None;
                }
            }
            let mixed = claims[k].as_ref().filter(|c| c.label == label).is_some_and(|c| c.mixed);
            claims[k] = Some(Claim { label: label.to_string(), mixed });
        }

        if let Some(s) = tabs.selected.filter(|&s| s < names.len()) {
            // rule 2: a session pane in the selected tab, whatever has focus
            if claims[s].is_none() {
                let taken: HashSet<&str> = claims.iter().flatten().map(|c| c.label.as_str()).collect();
                let found = tabs
                    .panes
                    .iter()
                    .map(|p| strip_admin_prefix(&p.title))
                    .find(|t| labels.contains(*t) && !taken.contains(t));
                if let Some(label) = found {
                    claims[s] = Some(Claim { label: label.to_string(), mixed: false });
                }
            }
            if !tabs.panes.is_empty() {
                if let Some(c) = claims[s].as_mut() {
                    c.mixed = tabs.panes.len() > 1;
                }
            }
        }

        let views = names
            .iter()
            .enumerate()
            .map(|(k, name)| TabView {
                index: k,
                name: name.clone(),
                selected: tabs.selected == Some(k),
                rect: tabs.rects.get(k).copied().flatten(),
                claim: claims[k].clone(),
            })
            .collect();
        self.windows.insert(key, names.iter().cloned().zip(claims).collect());
        views
    }

    /// Forget windows that are gone. Call only after a complete scan.
    pub fn retain(&mut self, keys: &[WindowId]) {
        self.windows.retain(|k, _| keys.contains(k));
    }
}

/// Carry claims from the previous full tab list to the current one:
/// order-preserving name matches first (LCS), then unique names that moved,
/// then in-place changes when the length is unchanged (rename, a split
/// tab's focus moving to another pane).
pub fn carry(prev: &[(String, Option<Claim>)], cur: &[String]) -> Vec<Option<Claim>> {
    let (n, m) = (prev.len(), cur.len());
    let mut out: Vec<Option<Claim>> = vec![None; m];
    let mut used_prev = vec![false; n];
    let mut done_cur = vec![false; m];
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if prev[i].0 == cur[j] { dp[i + 1][j + 1] + 1 } else { dp[i + 1][j].max(dp[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if prev[i].0 == cur[j] {
            out[j] = prev[i].1.clone();
            used_prev[i] = true;
            done_cur[j] = true;
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    for j in 0..m {
        if done_cur[j] {
            continue;
        }
        if let Some(i) = (0..n).find(|&i| !used_prev[i] && prev[i].0 == cur[j]) {
            out[j] = prev[i].1.clone();
            used_prev[i] = true;
            done_cur[j] = true;
        }
    }
    if n == m {
        for j in 0..m {
            if !done_cur[j] && !used_prev[j] {
                out[j] = prev[j].1.clone();
                used_prev[j] = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_without_rectangles_are_claimed_too() {
        let mut c = Claimer::new();
        let tabs = WindowTabs {
            names: vec!["pwsh".into(), "web01".into()],
            rects: vec![None, None],
            selected: Some(1),
            panes: Vec::new(),
        };
        let v = c.claim(WindowId(1), &tabs, &["web01".to_string()].into_iter().collect());
        assert_eq!(v[1].claim.as_ref().map(|c| c.label.as_str()), Some("web01"));
        assert!(v.iter().all(|t| t.rect.is_none()));
        assert!(v[1].selected);
    }

    fn labels(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn window(names: &[&str], selected: Option<usize>, panes: &[&str]) -> WindowTabs {
        WindowTabs {
            names: names.iter().map(|s| s.to_string()).collect(),
            rects: vec![None; names.len()],
            selected,
            panes: panes.iter().map(|t| Pane { title: t.to_string(), profile: String::new() }).collect(),
        }
    }

    fn claimed(views: &[TabView]) -> Vec<Option<(&str, bool)>> {
        views.iter().map(|v| v.claim.as_ref().map(|c| (c.label.as_str(), c.mixed))).collect()
    }

    #[test]
    fn rule1_by_name_and_admin_prefix() {
        let mut c = Claimer::new();
        let l = labels(&["web01", "db01"]);
        let v = c.claim(WindowId(1), &window(&["pwsh", "web01", "管理员: db01"], Some(0), &["pwsh"]), &l);
        assert_eq!(claimed(&v), vec![None, Some(("web01", false)), Some(("db01", false))]);
    }

    #[test]
    fn rule2_split_tab_with_foreign_focus() {
        let mut c = Claimer::new();
        let l = labels(&["web01"]);
        let v = c.claim(WindowId(1), &window(&["pwsh", "命令提示符"], Some(1), &["web01", "命令提示符"]), &l);
        assert_eq!(claimed(&v), vec![None, Some(("web01", true))]);
        // selecting another tab keeps the claim and the mixed flag
        let v = c.claim(WindowId(1), &window(&["pwsh", "命令提示符"], Some(0), &["pwsh"]), &l);
        assert_eq!(claimed(&v), vec![None, Some(("web01", true))]);
    }

    #[test]
    fn rule2_does_not_claim_a_label_twice() {
        let mut c = Claimer::new();
        let l = labels(&["web01"]);
        let v = c.claim(WindowId(1), &window(&["web01", "cmd"], Some(1), &["web01"]), &l);
        assert_eq!(claimed(&v), vec![Some(("web01", false)), None]);
    }

    #[test]
    fn carried_through_rename_and_moves() {
        let mut c = Claimer::new();
        let l = labels(&["a", "b"]);
        c.claim(WindowId(1), &window(&["a", "x", "b"], None, &[]), &l);
        // renamed in place
        let v = c.claim(WindowId(1), &window(&["a", "x", "my b"], None, &[]), &l);
        assert_eq!(claimed(&v), vec![Some(("a", false)), None, Some(("b", false))]);
        // dragged to the front, a tab opened at the end
        let v = c.claim(WindowId(1), &window(&["my b", "a", "x", "y"], None, &[]), &l);
        assert_eq!(claimed(&v), vec![Some(("b", false)), Some(("a", false)), None, None]);
        // the renamed one closed
        let v = c.claim(WindowId(1), &window(&["a", "x", "y"], None, &[]), &l);
        assert_eq!(claimed(&v), vec![Some(("a", false)), None, None]);
    }

    #[test]
    fn claims_end_with_the_session_or_the_window() {
        let mut c = Claimer::new();
        c.claim(WindowId(1), &window(&["a", "renamed"], Some(1), &["b"]), &labels(&["a", "b"]));
        let v = c.claim(WindowId(1), &window(&["a", "renamed"], None, &[]), &labels(&["a"]));
        assert_eq!(claimed(&v), vec![Some(("a", false)), None]);
        c.retain(&[]);
        let v = c.claim(WindowId(1), &window(&["x", "renamed"], None, &[]), &labels(&["a", "b"]));
        assert_eq!(claimed(&v), vec![None, None]);
    }

    #[test]
    fn exact_name_wins_over_a_carried_claim() {
        let mut c = Claimer::new();
        let l = labels(&["a"]);
        c.claim(WindowId(1), &window(&["a", "x"], None, &[]), &l);
        // the claimed tab changed its title, another tab now carries the label
        let v = c.claim(WindowId(1), &window(&["y", "a"], None, &[]), &l);
        assert_eq!(claimed(&v), vec![None, Some(("a", false))]);
    }

    #[test]
    fn many_tabs_scrolled() {
        let mut c = Claimer::new();
        let names: Vec<String> = (0..64).map(|i| format!("nt-{i}")).collect();
        let l: HashSet<String> = names.iter().cloned().collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        c.claim(WindowId(1), &window(&refs, Some(10), &["nt-10", "cmd"]), &l);
        let mut renamed = refs.clone();
        renamed[10] = "cmd";
        let v = c.claim(WindowId(1), &window(&renamed, Some(63), &["nt-63"]), &l);
        assert_eq!(v.iter().filter(|t| t.claim.is_some()).count(), 64);
        assert_eq!(v[10].claim, Some(Claim { label: "nt-10".into(), mixed: true }));
    }
}
