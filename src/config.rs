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

/// Line style of a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Dash {
    Solid,
    Dashed,
    Dotted,
}

/// How the area of a `band` series is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BandPattern {
    #[default]
    Solid,
    Hatch,
}

/// `band:` of a series entry: the area between two data sources.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BandDef {
    pub lower: String,
    pub upper: String,
    pub pattern: Option<BandPattern>,
    pub opacity: Option<f64>,
}

/// One entry of `series:`, as written in YAML.  Exactly one of `ds`,
/// `band` and `diff` says what it reads; the other keys only say how it
/// is drawn.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SeriesDef {
    pub ds: Option<String>,
    pub label: Option<String>,
    pub color: Option<String>,
    pub dash: Option<Dash>,
    pub width: Option<f64>,
    pub fill: Option<f64>,
    pub band: Option<BandDef>,
    pub diff: Option<Vec<String>>,
    pub floor: Option<f64>,
}

/// What a validated series reads from each matched file.
#[derive(Debug, Clone, PartialEq)]
pub enum SeriesKind {
    /// One data source.
    Ds(String),
    /// `lower..upper` where both are known and `lower <= upper`.
    Band { lower: String, upper: String },
    /// `minuend - subtrahend`, clamped below at `floor`.
    Diff {
        minuend: String,
        subtrahend: String,
        floor: Option<f64>,
    },
}

impl SeriesKind {
    /// Data sources this series reads, in a stable order.
    pub fn inputs(&self) -> Vec<&str> {
        match self {
            SeriesKind::Ds(ds) => vec![ds],
            SeriesKind::Band { lower, upper } => vec![lower, upper],
            SeriesKind::Diff {
                minuend,
                subtrahend,
                ..
            } => vec![minuend, subtrahend],
        }
    }
}

/// Band drawing passed to the frontend.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct BandStyle {
    pub pattern: BandPattern,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
}

/// Pure presentation of a series, passed through to the frontend.  Every
/// field is omitted from JSON when unset.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct SeriesStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dash: Option<Dash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub band: Option<BandStyle>,
    /// `Some(false)` for series drawn without a line (bands).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<bool>,
}

/// A validated series.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub kind: SeriesKind,
    pub label: Option<String>,
    pub style: SeriesStyle,
}

impl Series {
    /// Own name of the series: its label, or its data source.
    pub fn own_label(&self) -> &str {
        match (&self.label, &self.kind) {
            (Some(label), _) => label,
            (None, SeriesKind::Ds(ds)) => ds,
            (None, SeriesKind::Band { lower, .. }) => lower,
            (None, SeriesKind::Diff { minuend, .. }) => minuend,
        }
    }
}

/// `y-axis:` of a graph.  Serialized in `api/graph` as `y_axis`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct YAxis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval: Option<f64>,
    #[serde(
        rename(deserialize = "minor-interval"),
        skip_serializing_if = "Option::is_none"
    )]
    pub minor_interval: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Graph {
    /// Opaque identifier assigned by the host application.
    pub id: String,
    pub title: String,
    /// Effective, resolved RRD directory supplied by the host.
    pub rrd_dir: PathBuf,
    pub files: FilePattern,
    pub series: Vec<Series>,
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
    pub y_axis: Option<YAxis>,
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
    pub y_axis: Option<YAxis>,
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
            ("y-axis", self.y_axis.is_some()),
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
            (Some(ds), None) => vec![SeriesDef {
                ds: Some(ds),
                ..SeriesDef::default()
            }],
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
        let graph_type = raw.graph_type.unwrap_or_default();
        let stack_toggle = raw.stack_toggle.unwrap_or(false);
        let stackable = graph_type != GraphType::Stacked && !stack_toggle;
        let series = series
            .into_iter()
            .enumerate()
            .map(|(i, def)| {
                build_series(def, stackable, raw.top.is_some())
                    .map_err(|e| invalid(at, format!("`series` #{}: {e}", i + 1)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(axis) = &raw.y_axis {
            check_y_axis(axis).map_err(|e| invalid(at, format!("`y-axis`: {e}")))?;
        }
        Ok(Graph {
            id,
            title,
            rrd_dir,
            files,
            series,
            label: raw.label,
            unit: raw.unit,
            graph_type,
            stack_toggle,
            top: raw.top,
            min_max: raw.min_max.unwrap_or(false),
            scale,
            valid_max: raw.valid_max,
            hold,
            y_axis: raw.y_axis,
        })
    }
}

/// Validate one `series:` entry.  `stackable` is false when the graph
/// may be drawn stacked; `top` is true when series may be summed.
fn build_series(def: SeriesDef, stackable: bool, top: bool) -> Result<Series, String> {
    let label = def.label;
    let kind = match (def.ds, def.band.clone(), def.diff) {
        (Some(ds), None, None) => SeriesKind::Ds(ds),
        (None, Some(band), None) => SeriesKind::Band {
            lower: band.lower,
            upper: band.upper,
        },
        (None, None, Some(diff)) => {
            let [minuend, subtrahend]: [String; 2] = diff
                .try_into()
                .map_err(|_| "`diff` must list exactly two data sources".to_string())?;
            SeriesKind::Diff {
                minuend,
                subtrahend,
                floor: def.floor,
            }
        }
        _ => return Err("set exactly one of `ds`, `band` or `diff`".into()),
    };
    let derived = match &kind {
        SeriesKind::Ds(_) => None,
        SeriesKind::Band { .. } => Some("band"),
        SeriesKind::Diff { .. } => Some("diff"),
    };
    if let Some(name) = derived {
        if label.is_none() {
            return Err(format!("a `{name}` series needs a `label`"));
        }
    }
    match (&kind, def.floor) {
        (SeriesKind::Diff { .. }, Some(f)) if !f.is_finite() => {
            return Err("`floor` must be a finite number".into())
        }
        (SeriesKind::Diff { .. }, _) | (_, None) => {}
        (_, Some(_)) => return Err("`floor` is only valid on a `diff` series".into()),
    }
    if let Some(color) = &def.color {
        if !is_color(color) {
            return Err(format!("`color` must be `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or a CSS color name, got {color:?}"));
        }
    }
    if let Some(w) = def.width {
        if !w.is_finite() || w <= 0.0 {
            return Err("`width` must be a finite number above 0".into());
        }
    }
    let unit_interval = |v: f64| v.is_finite() && v > 0.0 && v <= 1.0;
    if def.fill.is_some_and(|f| !unit_interval(f)) {
        return Err("`fill` must be in (0, 1]".into());
    }
    let band = match def.band {
        Some(band) => {
            if band.opacity.is_some_and(|o| !unit_interval(o)) {
                return Err("band `opacity` must be in (0, 1]".into());
            }
            if def.dash.is_some() || def.width.is_some() || def.fill.is_some() {
                return Err(
                    "a `band` series draws no line: `dash`, `width` and `fill` are not allowed"
                        .into(),
                );
            }
            Some(BandStyle {
                pattern: band.pattern.unwrap_or_default(),
                opacity: band.opacity,
            })
        }
        None => None,
    };
    if !stackable && (def.fill.is_some() || derived.is_some()) {
        return Err(
            "`fill`, `band` and `diff` cannot be combined with `type: stacked` or `stack-toggle`"
                .into(),
        );
    }
    if top && derived.is_some() {
        return Err("`band` and `diff` cannot be combined with `top`".into());
    }
    Ok(Series {
        kind,
        label,
        style: SeriesStyle {
            color: def.color,
            dash: def.dash,
            width: def.width,
            fill: def.fill,
            line: band.map(|_| false),
            band,
        },
    })
}

fn check_y_axis(axis: &YAxis) -> Result<(), String> {
    for (name, value) in [
        ("min", axis.min),
        ("max", axis.max),
        ("interval", axis.interval),
        ("minor-interval", axis.minor_interval),
    ] {
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(format!("`{name}` must be a finite number"));
        }
    }
    if let (Some(min), Some(max)) = (axis.min, axis.max) {
        if min >= max {
            return Err("`min` must be below `max`".into());
        }
    }
    if axis.interval.is_some_and(|v| v <= 0.0) {
        return Err("`interval` must be a finite number above 0".into());
    }
    if let Some(minor) = axis.minor_interval {
        let Some(interval) = axis.interval else {
            return Err("`minor-interval` needs `interval`".into());
        };
        let count = interval / minor;
        let whole = count.round();
        // Same tolerance as `@0k/time-chart`, which re-validates it.
        if minor <= 0.0
            || whole < 2.0
            || (count - whole).abs() > f64::EPSILON * count.max(1.0) * 4.0
        {
            return Err(
                "`minor-interval` must divide `interval` into an integer count of at least 2"
                    .into(),
            );
        }
    }
    Ok(())
}

/// CSS color names (CSS Color Module Level 4), plus `transparent`.
const CSS_COLOR_NAMES: &[&str] = &[
    "aliceblue",
    "antiquewhite",
    "aqua",
    "aquamarine",
    "azure",
    "beige",
    "bisque",
    "black",
    "blanchedalmond",
    "blue",
    "blueviolet",
    "brown",
    "burlywood",
    "cadetblue",
    "chartreuse",
    "chocolate",
    "coral",
    "cornflowerblue",
    "cornsilk",
    "crimson",
    "cyan",
    "darkblue",
    "darkcyan",
    "darkgoldenrod",
    "darkgray",
    "darkgreen",
    "darkgrey",
    "darkkhaki",
    "darkmagenta",
    "darkolivegreen",
    "darkorange",
    "darkorchid",
    "darkred",
    "darksalmon",
    "darkseagreen",
    "darkslateblue",
    "darkslategray",
    "darkslategrey",
    "darkturquoise",
    "darkviolet",
    "deeppink",
    "deepskyblue",
    "dimgray",
    "dimgrey",
    "dodgerblue",
    "firebrick",
    "floralwhite",
    "forestgreen",
    "fuchsia",
    "gainsboro",
    "ghostwhite",
    "gold",
    "goldenrod",
    "gray",
    "green",
    "greenyellow",
    "grey",
    "honeydew",
    "hotpink",
    "indianred",
    "indigo",
    "ivory",
    "khaki",
    "lavender",
    "lavenderblush",
    "lawngreen",
    "lemonchiffon",
    "lightblue",
    "lightcoral",
    "lightcyan",
    "lightgoldenrodyellow",
    "lightgray",
    "lightgreen",
    "lightgrey",
    "lightpink",
    "lightsalmon",
    "lightseagreen",
    "lightskyblue",
    "lightslategray",
    "lightslategrey",
    "lightsteelblue",
    "lightyellow",
    "lime",
    "limegreen",
    "linen",
    "magenta",
    "maroon",
    "mediumaquamarine",
    "mediumblue",
    "mediumorchid",
    "mediumpurple",
    "mediumseagreen",
    "mediumslateblue",
    "mediumspringgreen",
    "mediumturquoise",
    "mediumvioletred",
    "midnightblue",
    "mintcream",
    "mistyrose",
    "moccasin",
    "navajowhite",
    "navy",
    "oldlace",
    "olive",
    "olivedrab",
    "orange",
    "orangered",
    "orchid",
    "palegoldenrod",
    "palegreen",
    "paleturquoise",
    "palevioletred",
    "papayawhip",
    "peachpuff",
    "peru",
    "pink",
    "plum",
    "powderblue",
    "purple",
    "rebeccapurple",
    "red",
    "rosybrown",
    "royalblue",
    "saddlebrown",
    "salmon",
    "sandybrown",
    "seagreen",
    "seashell",
    "sienna",
    "silver",
    "skyblue",
    "slateblue",
    "slategray",
    "slategrey",
    "snow",
    "springgreen",
    "steelblue",
    "tan",
    "teal",
    "thistle",
    "tomato",
    "transparent",
    "turquoise",
    "violet",
    "wheat",
    "white",
    "whitesmoke",
    "yellow",
    "yellowgreen",
];

fn is_color(s: &str) -> bool {
    match s.strip_prefix('#') {
        Some(hex) => {
            matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => CSS_COLOR_NAMES.contains(&s),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlabeled_derived_series_have_a_total_fallback() {
        for kind in [
            SeriesKind::Band {
                lower: "a".into(),
                upper: "b".into(),
            },
            SeriesKind::Diff {
                minuend: "a".into(),
                subtrahend: "b".into(),
                floor: None,
            },
        ] {
            let series = Series {
                kind,
                label: None,
                style: SeriesStyle::default(),
            };
            assert_eq!(series.own_label(), "a");
        }
    }
}
