use std::path::{Path, PathBuf};

use super::OrphanReport;
use super::identity::{extract_bundle_id, slug};

#[derive(Debug, Clone)]
pub enum IgnoreRule {
    App(String),
    Path(PathBuf),
}

impl PartialEq for IgnoreRule {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::App(a), Self::App(b)) => a.eq_ignore_ascii_case(b),
            (Self::Path(a), Self::Path(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for IgnoreRule {}

impl IgnoreRule {
    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim();
        if s.is_empty() {
            return None;
        }
        if s.contains('/') || s.starts_with('~') {
            Some(Self::Path(expand_tilde(Path::new(s))))
        } else {
            Some(Self::App(s.to_string()))
        }
    }

    pub fn as_stored(&self) -> String {
        match self {
            Self::App(id) => id.clone(),
            Self::Path(p) => p.to_string_lossy().into_owned(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::App(_) => "app",
            Self::Path(_) => "path",
        }
    }

    pub fn display(&self) -> String {
        self.as_stored()
    }

    pub fn matches(&self, app_id: &str, path: &Path) -> bool {
        match self {
            Self::App(id) => app_ids_match(id, app_id),
            Self::Path(p) => path == p || path.starts_with(p),
        }
    }
}

pub(crate) fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = crate::paths::default_home()
    {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn app_ids_match(rule: &str, app_id: &str) -> bool {
    if rule.eq_ignore_ascii_case(app_id) {
        return true;
    }
    let rs = slug(rule);
    let aslug = slug(app_id);
    if !rs.is_empty() && rs == aslug {
        return true;
    }
    if let (Some(a), Some(b)) = (extract_bundle_id(rule), extract_bundle_id(app_id))
        && a.eq_ignore_ascii_case(&b)
    {
        return true;
    }
    false
}

pub fn is_ignored(rules: &[String], app_id: &str, path: &Path) -> bool {
    rules
        .iter()
        .filter_map(|r| IgnoreRule::parse(r))
        .any(|r| r.matches(app_id, path))
}

/// Returns true if a new rule was inserted.
pub fn add_ignore(rules: &mut Vec<String>, raw: &str) -> Result<bool, &'static str> {
    let Some(rule) = IgnoreRule::parse(raw) else {
        return Err("empty ignore rule");
    };
    if let IgnoreRule::Path(p) = &rule
        && p.components().count() < 4
    {
        return Err("path too shallow to ignore");
    }
    if rules
        .iter()
        .any(|r| IgnoreRule::parse(r).as_ref() == Some(&rule))
    {
        return Ok(false);
    }
    rules.push(rule.as_stored());
    rules.sort();
    Ok(true)
}

pub fn remove_ignore(rules: &mut Vec<String>, raw: &str) -> bool {
    let Some(want) = IgnoreRule::parse(raw) else {
        return false;
    };
    let before = rules.len();
    rules.retain(|r| IgnoreRule::parse(r).as_ref() != Some(&want));
    before != rules.len()
}

/// Drop ignored leftover paths from a report. Returns how many paths were hidden.
pub fn apply_ignore(report: &mut OrphanReport, rules: &[String]) -> usize {
    if rules.is_empty() {
        return 0;
    }
    let before: usize = report.apps.iter().map(|a| a.paths.len()).sum();
    for app in &mut report.apps {
        app.paths.retain(|p| !is_ignored(rules, &app.id, &p.path));
        app.size = app.paths.iter().map(|p| p.size).sum();
    }
    report.apps.retain(|a| !a.paths.is_empty());
    let after: usize = report.apps.iter().map(|a| a.paths.len()).sum();
    before.saturating_sub(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orphans::{Confidence, OrphanApp, OrphanPath, OrphanPathKind};

    fn path_item(p: &str) -> OrphanPath {
        OrphanPath {
            path: PathBuf::from(p),
            kind: OrphanPathKind::Dir,
            size: 10,
            mtime: None,
            location: "Containers".into(),
        }
    }

    #[test]
    fn parse_app_vs_path() {
        assert!(matches!(
            IgnoreRule::parse("com.foo.bar"),
            Some(IgnoreRule::App(_))
        ));
        assert!(matches!(
            IgnoreRule::parse("/Users/x/Library/Containers/com.foo"),
            Some(IgnoreRule::Path(_))
        ));
        assert!(IgnoreRule::parse("  ").is_none());
    }

    #[test]
    fn app_rule_hides_related_paths() {
        let rules = vec!["com.foo.bar".into()];
        assert!(is_ignored(
            &rules,
            "com.foo.bar",
            Path::new("/Users/x/Library/Containers/com.foo.bar")
        ));
        assert!(is_ignored(
            &rules,
            "com.foo.bar",
            Path::new("/Users/x/Library/Caches/com.foo.bar")
        ));
        assert!(!is_ignored(
            &rules,
            "com.other",
            Path::new("/Users/x/Library/Containers/com.other")
        ));
    }

    #[test]
    fn path_rule_is_exact_or_prefix() {
        let dir = "/Users/x/Library/Containers/org.foo";
        let rules = vec![dir.to_string()];
        assert!(is_ignored(&rules, "org.foo", Path::new(dir)));
        assert!(is_ignored(
            &rules,
            "org.foo",
            Path::new("/Users/x/Library/Containers/org.foo/Data")
        ));
        assert!(!is_ignored(
            &rules,
            "org.foo",
            Path::new("/Users/x/Library/Containers/org.bar")
        ));
    }

    #[test]
    fn add_remove_dedup() {
        let mut rules = Vec::new();
        assert_eq!(add_ignore(&mut rules, "com.foo"), Ok(true));
        assert_eq!(add_ignore(&mut rules, "COM.FOO"), Ok(false));
        assert!(remove_ignore(&mut rules, "com.foo"));
        assert!(rules.is_empty());
        assert_eq!(
            add_ignore(&mut rules, "/tmp"),
            Err("path too shallow to ignore")
        );
    }

    #[test]
    fn apply_drops_ignored_apps() {
        let mut report = OrphanReport {
            installed_apps: 1,
            apps: vec![OrphanApp {
                id: "com.foo".into(),
                name: "foo".into(),
                confidence: Confidence::High,
                reason: String::new(),
                size: 20,
                paths: vec![
                    path_item("/Users/x/Library/Containers/com.foo"),
                    path_item("/Users/x/Library/Caches/com.foo"),
                ],
            }],
        };
        let hidden = apply_ignore(&mut report, &["com.foo".into()]);
        assert_eq!(hidden, 2);
        assert!(report.apps.is_empty());
    }
}
