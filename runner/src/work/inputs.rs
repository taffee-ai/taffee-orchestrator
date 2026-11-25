//! Pinned inputs as the orchestrator reads them itself, independently of any worker. Kind
//! functions in checks and the coverage computation read from here (WM-CHK-02, WM-CHK-06).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub trait InputStore: Send + Sync {
    /// Every file at this pinned reference, or None if the reference is not available.
    fn files(&self, repo: &str) -> Option<Vec<String>>;
    fn line_count(&self, repo: &str, file: &str) -> Option<usize>;
    fn line(&self, repo: &str, file: &str, n: usize) -> Option<String>;
    fn declared(&self, repo: &str, symbol: &str) -> Option<bool>;
}

/// A repository written out as a fixture file (docs/spec/work-manifest/fixtures/).
#[derive(Debug, Clone, Deserialize)]
pub struct Fixture {
    pub r#ref: String,
    #[serde(default)]
    pub declarations: BTreeMap<String, String>,
    pub files: BTreeMap<String, FixtureFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureFile {
    pub lines: usize,
    #[serde(default)]
    pub text: BTreeMap<String, String>,
}

impl Fixture {
    pub fn load(path: &Path) -> Result<Fixture> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Ok(serde_json::from_value(super::kind::yaml_to_json(&text)?)?)
    }
}

enum Source {
    Fixture(Fixture),
    Dir(PathBuf),
}

/// Pinned references mapped to fixtures or checked-out directories.
#[derive(Default)]
pub struct Store {
    repos: BTreeMap<String, Source>,
}

impl Store {
    pub fn add_fixture(&mut self, f: Fixture) {
        self.repos.insert(f.r#ref.clone(), Source::Fixture(f));
    }

    pub fn add_dir(&mut self, pinned: &str, dir: PathBuf) {
        self.repos.insert(pinned.to_string(), Source::Dir(dir));
    }

    /// `ref → path`: a `.yaml` path is a fixture, anything else a checked-out directory.
    pub fn from_map(map: &BTreeMap<String, PathBuf>) -> Result<Store> {
        let mut s = Store::default();
        for (r, p) in map {
            if matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml")) {
                let mut f = Fixture::load(p)?;
                f.r#ref = r.clone();
                s.add_fixture(f);
            } else {
                s.add_dir(r, p.clone());
            }
        }
        Ok(s)
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

fn read_lines(dir: &Path, file: &str) -> Option<Vec<String>> {
    if file.contains("..") {
        return None;
    }
    let text = std::fs::read_to_string(dir.join(file)).ok()?;
    Some(text.lines().map(String::from).collect())
}

pub fn mentions(line: &str, symbol: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut start = 0;
    while let Some(i) = line[start..].find(symbol) {
        let at = start + i;
        let before = line[..at].chars().next_back();
        let after = line[at + symbol.len()..].chars().next();
        if !before.is_some_and(is_word) && !after.is_some_and(is_word) {
            return true;
        }
        start = at + symbol.len();
    }
    false
}

const DECLARERS: &[&str] = &["class", "interface", "type", "enum", "function", "struct", "trait", "fn", "def", "record"];

fn declares(line: &str, symbol: &str) -> bool {
    let words: Vec<&str> = line.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).filter(|w| !w.is_empty()).collect();
    words.windows(2).any(|w| DECLARERS.contains(&w[0]) && w[1] == symbol)
}

impl InputStore for Store {
    fn files(&self, repo: &str) -> Option<Vec<String>> {
        match self.repos.get(repo)? {
            Source::Fixture(f) => Some(f.files.keys().cloned().collect()),
            Source::Dir(d) => {
                let mut out = Vec::new();
                walk(d, d, &mut out);
                out.sort();
                Some(out)
            }
        }
    }

    fn line_count(&self, repo: &str, file: &str) -> Option<usize> {
        match self.repos.get(repo)? {
            Source::Fixture(f) => f.files.get(file).map(|x| x.lines),
            Source::Dir(d) => read_lines(d, file).map(|l| l.len()),
        }
    }

    fn line(&self, repo: &str, file: &str, n: usize) -> Option<String> {
        match self.repos.get(repo)? {
            Source::Fixture(f) => f.files.get(file).and_then(|x| x.text.get(&n.to_string()).cloned()),
            Source::Dir(d) => read_lines(d, file).and_then(|l| l.get(n.checked_sub(1)?).cloned()),
        }
    }

    fn declared(&self, repo: &str, symbol: &str) -> Option<bool> {
        match self.repos.get(repo)? {
            Source::Fixture(f) => Some(f.declarations.contains_key(symbol)),
            Source::Dir(_) => {
                let files = self.files(repo)?;
                Some(files.iter().any(|file| {
                    let Source::Dir(d) = &self.repos[repo] else { return false };
                    read_lines(d, file).is_some_and(|ls| ls.iter().any(|l| declares(l, symbol)))
                }))
            }
        }
    }
}

/// Files of `repo` matching any of `globs` (`**` crosses directories, `*` does not).
pub fn files_under(store: &dyn InputStore, repo: &str, globs: &[String]) -> Option<Vec<String>> {
    let mut b = globset::GlobSetBuilder::new();
    for g in globs {
        b.add(globset::GlobBuilder::new(g).literal_separator(true).build().ok()?);
    }
    let set = b.build().ok()?;
    let mut files: Vec<String> = store.files(repo)?.into_iter().filter(|f| set.is_match(f)).collect();
    files.sort();
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_mentions() {
        assert!(mentions("const x = new PaymentService(db);", "PaymentService"));
        assert!(mentions("describe('PaymentService', () => {", "PaymentService"));
        assert!(!mentions("PaymentServiceImpl", "PaymentService"));
        assert!(!mentions("MyPaymentService", "PaymentService"));
    }

    #[test]
    fn declarations() {
        assert!(declares("export class PaymentService {", "PaymentService"));
        assert!(!declares("new PaymentService()", "PaymentService"));
    }
}
