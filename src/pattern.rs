//! `files:` patterns: a relative path whose components may contain `*`.
//!
//! Each `*` matches any run of characters inside ONE path component and
//! is a numbered capture (`{1}`, `{2}`, … in label templates), in the
//! order the stars appear.  `containers/*/*/memory.rrd` matches
//! `containers/myc/odoo/memory.rrd` with captures `["myc", "odoo"]`.
//!
//! Nothing else is special: no `?`, no `**`, no braces.  That keeps the
//! capture numbering obvious to whoever writes the label template.

use std::path::{Path, PathBuf};

use regex::Regex;

/// Why a relative file pattern could not be parsed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PatternError {
    #[error("pattern is empty")]
    Empty,
    #[error("{pattern:?} must be relative to `rrd-dir` (no leading `/`)")]
    Absolute { pattern: String },
    #[error("{pattern:?} has an empty, `.` or `..` path component")]
    InvalidComponent { pattern: String },
    #[error(transparent)]
    Regex(#[from] regex::Error),
}

#[derive(Debug, Clone)]
enum Component {
    Literal(String),
    Wild(Regex),
}

impl Component {
    fn captures(&self, name: &str) -> Option<Vec<String>> {
        match self {
            Self::Literal(literal) => (literal == name).then(Vec::new),
            Self::Wild(re) if !name.starts_with('.') => re.captures(name).map(|m| {
                m.iter()
                    .skip(1)
                    .map(|c| c.map_or("", |c| c.as_str()).to_string())
                    .collect()
            }),
            Self::Wild(_) => None,
        }
    }
}

/// How a relative event path relates to a file pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatternMatch {
    Full,
    Prefix,
    None,
}

#[derive(Debug, Clone)]
pub struct FilePattern {
    source: String,
    components: Vec<Component>,
    captures: usize,
}

/// One file matched by a [`FilePattern`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FileMatch {
    pub path: PathBuf,
    pub captures: Vec<String>,
}

impl FilePattern {
    pub fn parse(source: &str) -> Result<FilePattern, PatternError> {
        if source.is_empty() {
            return Err(PatternError::Empty);
        }
        if source.starts_with('/') {
            return Err(PatternError::Absolute {
                pattern: source.into(),
            });
        }
        let mut components = Vec::new();
        let mut captures = 0;
        for part in source.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(PatternError::InvalidComponent {
                    pattern: source.into(),
                });
            }
            if part.contains('*') {
                let stars = part.matches('*').count();
                captures += stars;
                let body = part
                    .split('*')
                    .map(regex::escape)
                    .collect::<Vec<_>>()
                    .join("(.*)");
                let re = Regex::new(&format!("^{body}$"))?;
                components.push(Component::Wild(re));
            } else {
                components.push(Component::Literal(part.to_string()));
            }
        }
        Ok(FilePattern {
            source: source.to_string(),
            components,
            captures,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Number of `*` in the pattern.
    pub fn capture_count(&self) -> usize {
        self.captures
    }

    /// All regular files under `root` matching the pattern, sorted by
    /// path.  Hidden entries (leading `.`) never match a `*`.
    pub(crate) fn expand(&self, root: &Path) -> std::io::Result<Vec<FileMatch>> {
        let mut out = Vec::new();
        self.walk(root, 0, Vec::new(), &mut out, &mut |_| {}, false)?;
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }

    /// Existing directories visited by expansion, including the root.
    /// Missing paths are normal; other traversal errors are debug-logged.
    pub(crate) fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Err(error) = self.walk(
            root,
            0,
            Vec::new(),
            &mut Vec::new(),
            &mut |dir| dirs.push(dir.to_path_buf()),
            true,
        ) {
            log::debug!(
                "cannot enumerate pattern directories in {}: {error}",
                root.display()
            );
        }
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Match a whole relative path or an intermediate directory, without IO.
    pub(crate) fn match_rel(&self, rel: &Path) -> PatternMatch {
        let mut depth = 0;
        for part in rel.components() {
            let std::path::Component::Normal(name) = part else {
                return PatternMatch::None;
            };
            let Some(name) = name.to_str() else {
                return PatternMatch::None;
            };
            let Some(component) = self.components.get(depth) else {
                return PatternMatch::None;
            };
            if component.captures(name).is_none() {
                return PatternMatch::None;
            }
            depth += 1;
        }
        if depth == self.components.len() {
            PatternMatch::Full
        } else {
            PatternMatch::Prefix
        }
    }

    fn walk(
        &self,
        dir: &Path,
        depth: usize,
        captures: Vec<String>,
        out: &mut Vec<FileMatch>,
        on_dir: &mut impl FnMut(&Path),
        skip_errors: bool,
    ) -> std::io::Result<()> {
        match std::fs::metadata(dir) {
            Ok(metadata) if metadata.is_dir() => on_dir(dir),
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        let last = depth + 1 == self.components.len();
        let mut visit = |path: PathBuf, captures: Vec<String>| -> std::io::Result<()> {
            if last {
                if path.is_file() {
                    out.push(FileMatch { path, captures });
                }
                Ok(())
            } else if path.is_dir() {
                let result = self.walk(&path, depth + 1, captures, out, on_dir, skip_errors);
                if skip_errors {
                    if let Err(error) = result {
                        log::debug!("cannot traverse {}: {error}", path.display());
                    }
                    Ok(())
                } else {
                    result
                }
            } else {
                Ok(())
            }
        };
        match &self.components[depth] {
            Component::Literal(name) => visit(dir.join(name), captures),
            component @ Component::Wild(_) => {
                let entries = match std::fs::read_dir(dir) {
                    Ok(entries) => entries,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(e) => return Err(e),
                };
                for entry in entries {
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(error) if skip_errors => {
                            log::debug!("cannot read entry in {}: {error}", dir.display());
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if let Some(matched) = component.captures(name) {
                        let mut caps = captures.clone();
                        caps.extend(matched);
                        visit(entry.path(), caps)?;
                    }
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for p in paths {
            let p = dir.path().join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"").unwrap();
        }
        dir
    }

    fn expand(pattern: &str, dir: &tempfile::TempDir) -> Vec<(String, Vec<String>)> {
        FilePattern::parse(pattern)
            .unwrap()
            .expand(dir.path())
            .unwrap()
            .into_iter()
            .map(|m| {
                (
                    m.path
                        .strip_prefix(dir.path())
                        .unwrap()
                        .display()
                        .to_string(),
                    m.captures,
                )
            })
            .collect()
    }

    #[test]
    fn dirs_include_only_existing_matched_intermediates() {
        let dir = tree(&[
            "containers/a/x/memory.rrd",
            "containers/b/y/other",
            "containers/.hidden/z/memory.rrd",
            "elsewhere/file",
        ]);
        let p = FilePattern::parse("containers/*/*/memory.rrd").unwrap();
        let expected: Vec<_> = [
            "",
            "containers",
            "containers/a",
            "containers/a/x",
            "containers/b",
            "containers/b/y",
        ]
        .iter()
        .map(|p| dir.path().join(p))
        .collect();
        assert_eq!(p.dirs(dir.path()), expected);
        assert_eq!(
            FilePattern::parse("missing/*/x.rrd")
                .unwrap()
                .dirs(dir.path()),
            vec![dir.path().to_path_buf()]
        );
        assert!(p.dirs(&dir.path().join("absent")).is_empty());
        assert_eq!(
            FilePattern::parse("*.rrd").unwrap().dirs(dir.path()),
            vec![dir.path().to_path_buf()]
        );
    }

    #[test]
    fn relative_matching_distinguishes_full_prefix_and_nonmatches() {
        let p = FilePattern::parse("containers/*/*/memory.rrd").unwrap();
        for rel in [
            "containers/a/x/memory.rrd",
            "containers/@/curl:latest/memory.rrd",
        ] {
            assert_eq!(p.match_rel(Path::new(rel)), PatternMatch::Full);
        }
        for rel in ["", "containers", "containers/a", "containers/a/x"] {
            assert_eq!(p.match_rel(Path::new(rel)), PatternMatch::Prefix);
        }
        for rel in [
            "elsewhere",
            "containers/.hidden",
            "containers/a/.hidden/memory.rrd",
            "containers/a/x/other",
            "containers/a/x/memory.rrd/extra",
            "/containers/a/x/memory.rrd",
            "../containers",
        ] {
            assert_eq!(p.match_rel(Path::new(rel)), PatternMatch::None, "{rel}");
        }
        let literal = FilePattern::parse(".hidden/file.rrd").unwrap();
        assert_eq!(
            literal.match_rel(Path::new(".hidden/file.rrd")),
            PatternMatch::Full
        );
        assert_eq!(
            FilePattern::parse("disk-*/memory.rrd")
                .unwrap()
                .match_rel(Path::new("disk-a/memory.rrd")),
            PatternMatch::Full
        );
    }

    #[test]
    fn stars_capture_one_component_each_in_order() {
        let dir = tree(&[
            "containers/myc/odoo/memory.rrd",
            "containers/myc/odoo/network.rrd",
            "containers/@/curl:latest/memory.rrd",
        ]);
        assert_eq!(
            expand("containers/*/*/memory.rrd", &dir),
            vec![
                (
                    "containers/@/curl:latest/memory.rrd".into(),
                    vec!["@".into(), "curl:latest".into()]
                ),
                (
                    "containers/myc/odoo/memory.rrd".into(),
                    vec!["myc".into(), "odoo".into()]
                ),
            ]
        );
    }

    #[test]
    fn star_inside_a_component_captures_only_the_wild_part() {
        let dir = tree(&[
            "disk-2/docker/images/size.rrd",
            "disk-2/docker/images_r/size.rrd",
        ]);
        assert_eq!(
            expand("disk-2/docker/*_r/size.rrd", &dir),
            vec![(
                "disk-2/docker/images_r/size.rrd".into(),
                vec!["images".into()]
            )]
        );
    }

    #[test]
    fn literal_pattern_matches_only_an_existing_file() {
        let dir = tree(&["load_avg.rrd"]);
        assert_eq!(expand("load_avg.rrd", &dir).len(), 1);
        assert!(expand("disk.rrd", &dir).is_empty());
        assert!(expand("missing/*/x.rrd", &dir).is_empty());
    }

    #[test]
    fn directories_and_hidden_entries_do_not_match() {
        let dir = tree(&["a/x.rrd/inner", ".hidden.rrd", "b.rrd"]);
        assert_eq!(
            expand("*.rrd", &dir),
            vec![("b.rrd".into(), vec!["b".into()])]
        );
    }

    #[test]
    fn absolute_and_parent_paths_are_rejected() {
        assert!(FilePattern::parse("/etc/x.rrd").is_err());
        assert!(FilePattern::parse("../x.rrd").is_err());
        assert!(FilePattern::parse("a//x.rrd").is_err());
    }
}
