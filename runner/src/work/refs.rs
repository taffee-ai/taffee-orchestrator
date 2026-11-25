//! References: `name[@version][#sha256:hex]`. The name is everything before the last `@`.

use super::jcs::is_digest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub version: Option<String>,
    pub digest: Option<String>,
}

impl Ref {
    pub fn parse(s: &str) -> Result<Ref, String> {
        if s.is_empty() || s.chars().any(char::is_whitespace) {
            return Err(format!("'{s}' is not a reference"));
        }
        let (rest, digest) = match s.split_once('#') {
            Some((r, d)) => {
                if !is_digest(d) {
                    return Err(format!("'{s}': the part after '#' must be sha256:<64 hex>"));
                }
                (r, Some(d.to_string()))
            }
            None => (s, None),
        };
        let (name, version) = match rest.rsplit_once('@') {
            Some((n, v)) if !v.is_empty() => (n, Some(v.to_string())),
            Some(_) => return Err(format!("'{s}': empty version after '@'")),
            None => (rest, None),
        };
        if name.is_empty() || name.contains('@') {
            return Err(format!("'{s}': bad name"));
        }
        Ok(Ref { name: name.to_string(), version, digest })
    }

    /// Immutable: a full commit id as version, or a content digest.
    pub fn is_pinned(&self) -> bool {
        self.digest.is_some() || self.version.as_deref().is_some_and(is_commit)
    }
}

pub fn is_commit(v: &str) -> bool {
    (v.len() == 40 || v.len() == 64) && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.name)?;
        if let Some(v) = &self.version {
            write!(f, "@{v}")?;
        }
        if let Some(d) = &self.digest {
            write!(f, "#{d}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_pins() {
        let r = Ref::parse("github.com/acme/x@main").unwrap();
        assert_eq!((r.name.as_str(), r.version.as_deref(), r.is_pinned()), ("github.com/acme/x", Some("main"), false));
        let c = "4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e";
        assert!(Ref::parse(&format!("github.com/acme/x@{c}")).unwrap().is_pinned());
        let d = format!("sha256:{}", "a".repeat(64));
        let k = Ref::parse(&format!("code.impact_analysis@1#{d}")).unwrap();
        assert!(k.is_pinned());
        assert_eq!(k.to_string(), format!("code.impact_analysis@1#{d}"));
        assert!(Ref::parse("a b").is_err());
        assert!(Ref::parse("x@").is_err());
        assert!(Ref::parse("x#sha256:00").is_err());
    }
}
