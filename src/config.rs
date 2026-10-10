//! Strict YAML fields and validation for one graph, independent of navigation.

use std::path::PathBuf;

use serde::Deserialize;

use crate::pattern::FilePattern;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid YAML in {path}: {source}")]
    Yaml {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("{at}: {message}")]
    Invalid { at: String, message: String },
}

/// How the series of a graph are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GraphType {
    #[default]
    Line,
    Stacked,
}

/// One data source of a graph, as written in `series:`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SeriesDef {
    pub ds: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Graph {
    /// Opaque identifier assigned by the host application.
    pub id: String,
    pub title: String,
    /// Effective, resolved RRD directory supplied by the host.
    pub rrd_dir: PathBuf,
    pub files: FilePattern,
    pub series: Vec<SeriesDef>,
    /// Label template using `{N}` for the N-th `*` of `files`.
    pub label: Option<String>,
    pub unit: Option<String>,
    pub graph_type: GraphType,
    pub stack_toggle: bool,
    pub top: Option<usize>,
    pub min_max: bool,
    pub scale: f64,
    pub valid_max: Option<f64>,
    /// Seconds an unknown point may repeat the last known value.
    pub hold: Option<i64>,
}

/// Unvalidated YAML keys for one graph. Hosts may flatten these into their
/// own node descriptions, then supply an id, title and resolved directory.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GraphFields {
    pub files: Option<String>,
    pub ds: Option<String>,
    pub series: Option<Vec<SeriesDef>>,
    pub label: Option<String>,
    pub unit: Option<String>,
    #[serde(rename = "type")]
    pub graph_type: Option<GraphType>,
    pub stack_toggle: Option<bool>,
    pub top: Option<usize>,
    pub min_max: Option<bool>,
    pub scale: Option<f64>,
    pub valid_max: Option<f64>,
    pub hold: Option<serde_yaml::Value>,
}

impl GraphFields {
    /// Parse a standalone graph's YAML keys; no groups or navigation are needed.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        serde_yaml::from_str(text).map_err(|source| ConfigError::Yaml {
            path: PathBuf::new(),
            source,
        })
    }

    /// Set keys other than `files`, in the original validation order. Hosts use
    /// this to reject graph settings on their non-graph navigation nodes.
    pub fn graph_only_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        let flags = [
            ("ds", self.ds.is_some()),
            ("series", self.series.is_some()),
            ("label", self.label.is_some()),
            ("unit", self.unit.is_some()),
            ("type", self.graph_type.is_some()),
            ("stack-toggle", self.stack_toggle.is_some()),
            ("top", self.top.is_some()),
            ("min-max", self.min_max.is_some()),
            ("scale", self.scale.is_some()),
            ("valid-max", self.valid_max.is_some()),
            ("hold", self.hold.is_some()),
        ];
        for (key, set) in flags {
            if set {
                keys.push(key);
            }
        }
        keys
    }

    /// Validate graph keys using the host's diagnostic path and resolved RRD
    /// directory. Missing data is not a configuration error.
    pub fn build(
        self,
        id: String,
        title: String,
        at: &str,
        dir: Option<PathBuf>,
    ) -> Result<Graph, ConfigError> {
        let raw = self;
        let Some(rrd_dir) = dir else {
            return Err(invalid(
                at,
                "no `rrd-dir` set on this graph nor on any of its parents",
            ));
        };
        let files = raw
            .files
            .ok_or_else(|| invalid(at, "a graph needs `files`"))?;
        let files = FilePattern::parse(&files).map_err(|e| invalid(at, format!("`files`: {e}")))?;
        let series = match (raw.ds, raw.series) {
            (Some(_), Some(_)) => return Err(invalid(at, "set either `ds` or `series`, not both")),
            (None, None) => return Err(invalid(at, "a graph needs `ds` or `series`")),
            (Some(ds), None) => vec![SeriesDef { ds, label: None }],
            (None, Some(list)) => {
                if list.is_empty() {
                    return Err(invalid(at, "`series` must not be empty"));
                }
                list
            }
        };
        if let Some(template) = &raw.label {
            check_label(template, files.capture_count()).map_err(|e| invalid(at, e))?;
        }
        if raw.top == Some(0) {
            return Err(invalid(at, "`top` must be at least 1"));
        }
        let scale = raw.scale.unwrap_or(1.0);
        if !scale.is_finite() || scale == 0.0 {
            return Err(invalid(at, "`scale` must be a finite, non-zero number"));
        }
        if let Some(v) = raw.valid_max {
            if !v.is_finite() {
                return Err(invalid(at, "`valid-max` must be a finite number"));
            }
        }
        let hold = raw
            .hold
            .as_ref()
            .map(parse_duration)
            .transpose()
            .map_err(|e| invalid(at, format!("`hold`: {e}")))?;
        Ok(Graph {
            id,
            title,
            rrd_dir,
            files,
            series,
            label: raw.label,
            unit: raw.unit,
            graph_type: raw.graph_type.unwrap_or_default(),
            stack_toggle: raw.stack_toggle.unwrap_or(false),
            top: raw.top,
            min_max: raw.min_max.unwrap_or(false),
            scale,
            valid_max: raw.valid_max,
            hold,
        })
    }
}

fn invalid(at: &str, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        at: at.to_string(),
        message: message.into(),
    }
}

/// Seconds, or a number followed by `s`, `m`, `h`, `d`, `w` or `y` (365 days).
pub fn parse_duration(value: &serde_yaml::Value) -> Result<i64, String> {
    let bad = || format!("expected seconds or a duration like `30m`, `2h`, `1d`, got {value:?}");
    let seconds = match value {
        serde_yaml::Value::Number(n) => n.as_i64().ok_or_else(bad)?,
        serde_yaml::Value::String(s) => {
            let s = s.trim();
            let (digits, unit) =
                s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
            let n: i64 = digits.parse().map_err(|_| bad())?;
            let factor = match unit {
                "" | "s" => 1,
                "m" => 60,
                "h" => 3600,
                "d" => 86_400,
                "w" => 604_800,
                "y" => 31_536_000,
                _ => return Err(bad()),
            };
            n.checked_mul(factor).ok_or_else(bad)?
        }
        _ => return Err(bad()),
    };
    if seconds <= 0 {
        return Err(bad());
    }
    Ok(seconds)
}

fn check_label(template: &str, captures: usize) -> Result<(), String> {
    for token in label_tokens(template)? {
        if let LabelToken::Capture(n) = token {
            if n == 0 || n > captures {
                return Err(format!("`label` uses {{{n}}} but `files` has {captures} `*` (captures are numbered from 1)"));
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub(crate) enum LabelToken<'a> {
    Text(&'a str),
    Capture(usize),
}

/// Split a label template into literal text and `{N}` references.
pub(crate) fn label_tokens(template: &str) -> Result<Vec<LabelToken<'_>>, String> {
    let mut tokens = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            tokens.push(LabelToken::Text(&rest[..open]));
        }
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| format!("unclosed `{{` in label {template:?}"))?;
        let n: usize = after[..close].parse().map_err(|_| {
            format!(
                "label {template:?}: `{{{}}}` is not a capture number",
                &after[..close]
            )
        })?;
        tokens.push(LabelToken::Capture(n));
        rest = &after[close + 1..];
    }
    if !rest.is_empty() {
        tokens.push(LabelToken::Text(rest));
    }
    Ok(tokens)
}

/// Expand a leading `~` against `$HOME`.
pub fn expand_tilde(s: &str) -> PathBuf {
    let Some(rest) = s.strip_prefix('~') else {
        return PathBuf::from(s);
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return PathBuf::from(s);
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(rest.trim_start_matches('/')),
        None => PathBuf::from(s),
    }
}
