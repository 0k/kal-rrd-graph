//! Strict YAML fields and validation for one graph, independent of navigation.

use std::path::PathBuf;

use serde::Deserialize;

use crate::pattern::FilePattern;

/// YAML decoding or graph validation failure.
///
/// This enum may grow; downstream matches must include a wildcard arm.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The YAML cannot be decoded, including unknown keys or wrong value types.
    #[error("invalid YAML in {path}: {source}")]
    Yaml {
        /// Diagnostic file path; empty for [`GraphFields::parse`].
        path: PathBuf,
        /// YAML parser's underlying diagnostic.
        source: serde_yaml::Error,
    },
    /// Decoded graph keys fail semantic validation.
    #[error("{at}: {message}")]
    Invalid {
        /// Host-supplied diagnostic path, such as `Root > Boiler`.
        at: String,
        /// Reason the graph definition was rejected.
        message: String,
    },
}

/// How the series of a graph are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum GraphType {
    /// Draw independent lines; the default graph type.
    #[default]
    Line,
    /// Stack series values on a common time grid.
    Stacked,
}

/// Line style of a series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Dash {
    /// Continuous line.
    Solid,
    /// Repeated dash segments.
    Dashed,
    /// Repeated dots.
    Dotted,
}

/// How the area of a `band` series is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BandPattern {
    /// Solid area; the default band pattern.
    #[default]
    Solid,
    /// Diagonal hatching in the series color.
    Hatch,
}

/// `band:` of a series entry: the area between two data sources.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct BandDef {
    /// Data source providing the lower bound.
    pub lower: String,
    /// Data source providing the upper bound.
    pub upper: String,
    /// Area pattern, defaulting to [`BandPattern::Solid`] during validation.
    pub pattern: Option<BandPattern>,
    /// Explicit opacity in `(0, 1]`; `None` leaves the frontend default.
    pub opacity: Option<f64>,
}

/// One entry of `series:`, as written in YAML.  Exactly one of `ds`,
/// `band` and `diff` says what it reads; the other keys only say how it
/// is drawn.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SeriesDef {
    /// One data source; mutually exclusive with `band` and `diff`.
    pub ds: Option<String>,
    /// Own series label; required for `band`/`diff`, otherwise defaults to `ds`.
    pub label: Option<String>,
    /// CSS color name or `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`.
    pub color: Option<String>,
    /// Line dash style; unset leaves the frontend default.
    pub dash: Option<Dash>,
    /// Positive finite line width in pixels.
    pub width: Option<f64>,
    /// Fill opacity in `(0, 1]` below a line; not valid on stacked graphs.
    pub fill: Option<f64>,
    /// Area between two data sources; mutually exclusive with `ds` and `diff`.
    pub band: Option<BandDef>,
    /// Exactly two data sources, in minuend/subtrahend order.
    pub diff: Option<Vec<String>>,
    /// Finite lower clamp for `diff`, in scaled units; invalid for other kinds.
    pub floor: Option<f64>,
}

/// What a validated series reads from each matched file.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum SeriesKind {
    /// One data source.
    Ds(String),
    /// `lower..upper` where both are known and `lower <= upper`.
    Band {
        /// Data source providing the lower bound.
        lower: String,
        /// Data source providing the upper bound.
        upper: String,
    },
    /// `minuend - subtrahend`, clamped below at `floor`.
    Diff {
        /// Data source from which the subtrahend is subtracted.
        minuend: String,
        /// Data source subtracted from the minuend.
        subtrahend: String,
        /// Optional lower clamp applied after scaling and subtraction.
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
    pub(crate) pattern: BandPattern,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) opacity: Option<f64>,
}

impl BandStyle {
    /// Validated area pattern, including the default when YAML omitted it.
    pub fn pattern(&self) -> BandPattern {
        self.pattern
    }
    /// Explicit opacity, or `None` when the frontend chooses it.
    pub fn opacity(&self) -> Option<f64> {
        self.opacity
    }
}

/// Pure presentation of a series, passed through to the frontend.  Every
/// field is omitted from JSON when unset.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct SeriesStyle {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dash: Option<Dash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) fill: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) band: Option<BandStyle>,
    /// `Some(false)` for series drawn without a line (bands).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) line: Option<bool>,
}

impl SeriesStyle {
    /// Explicit CSS color, or `None` for a frontend-generated color.
    pub fn color(&self) -> Option<&str> {
        self.color.as_deref()
    }
    /// Explicit line dash style, or `None` for the frontend default.
    pub fn dash(&self) -> Option<Dash> {
        self.dash
    }
    /// Explicit positive line width in pixels.
    pub fn width(&self) -> Option<f64> {
        self.width
    }
    /// Explicit opacity of the area below a line.
    pub fn fill(&self) -> Option<f64> {
        self.fill
    }
    /// Band presentation, present only for a band series.
    pub fn band(&self) -> Option<&BandStyle> {
        self.band.as_ref()
    }
    /// `Some(false)` suppresses the line for bands; `None` leaves it enabled.
    pub fn line(&self) -> Option<bool> {
        self.line
    }
}

/// A series validated by [`GraphFields::build`].
///
/// Its inputs and presentation are read-only. Change the unvalidated
/// [`SeriesDef`] and rebuild the graph to modify it.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub(crate) kind: SeriesKind,
    pub(crate) label: Option<String>,
    pub(crate) style: SeriesStyle,
}

impl Series {
    /// Validated data-source operation for this series.
    pub fn kind(&self) -> &SeriesKind {
        &self.kind
    }
    /// Explicit own label, before combining it with the per-file label.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    /// Read-only drawing style passed through to the frontend.
    pub fn style(&self) -> &SeriesStyle {
        &self.style
    }
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

/// `y-axis:` of a graph. Serialized in `api/graph` as `y_axis`.
///
/// Deserialize from YAML or use [`YAxis::default`] for automatic axes.
/// [`GraphFields::build`] validates finite bounds, positive intervals and
/// an integer subdivision of at least two minor intervals per major interval.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct YAxis {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) interval: Option<f64>,
    #[serde(
        rename(deserialize = "minor-interval"),
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) minor_interval: Option<f64>,
}

impl YAxis {
    /// Explicit lower bound in scaled units, or `None` for automatic bounds.
    pub fn min(&self) -> Option<f64> {
        self.min
    }
    /// Explicit upper bound in scaled units, or `None` for automatic bounds.
    pub fn max(&self) -> Option<f64> {
        self.max
    }
    /// Major tick spacing in scaled units, or `None` for automatic ticks.
    pub fn interval(&self) -> Option<f64> {
        self.interval
    }
    /// Minor tick spacing in scaled units; requires a major interval.
    pub fn minor_interval(&self) -> Option<f64> {
        self.minor_interval
    }
}

/// Immutable graph definition validated by [`GraphFields::build`].
///
/// The host supplies identity, title and the resolved RRD directory. Graph
/// settings come from YAML keys rather than a navigation model. Validation
/// does not require the directory, files or data sources to exist.
#[derive(Debug, Clone)]
pub struct Graph {
    /// Opaque identifier assigned by the host application.
    pub(crate) id: String,
    pub(crate) title: String,
    /// Effective, resolved RRD directory supplied by the host.
    pub(crate) rrd_dir: PathBuf,
    pub(crate) files: FilePattern,
    pub(crate) series: Vec<Series>,
    /// Label template using `{N}` for the N-th `*` of `files`.
    pub(crate) label: Option<String>,
    pub(crate) unit: Option<String>,
    pub(crate) graph_type: GraphType,
    pub(crate) stack_toggle: bool,
    pub(crate) top: Option<usize>,
    pub(crate) min_max: bool,
    pub(crate) scale: f64,
    pub(crate) valid_max: Option<f64>,
    /// Seconds an unknown point may repeat the last known value.
    pub(crate) hold: Option<i64>,
    pub(crate) y_axis: Option<YAxis>,
}

impl Graph {
    /// Opaque identifier assigned by the host.
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Display title assigned by the host.
    pub fn title(&self) -> &str {
        &self.title
    }
    /// Resolved RRD directory supplied by the host; no tilde expansion is done.
    pub fn rrd_dir(&self) -> &std::path::Path {
        &self.rrd_dir
    }
    /// Validated relative file pattern and its numbered captures.
    pub fn files(&self) -> &FilePattern {
        &self.files
    }
    /// Validated series in drawing order, with later entries on top.
    pub fn series(&self) -> &[Series] {
        &self.series
    }
    /// Per-file label template; `None` joins captures with `/`.
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    /// Display unit supplied by YAML, or `None` for an unspecified unit.
    pub fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }
    /// Initial graph layout, defaulting to [`GraphType::Line`].
    pub fn graph_type(&self) -> GraphType {
        self.graph_type
    }
    /// Whether the host should offer a stack/unstack control.
    pub fn stack_toggle(&self) -> bool {
        self.stack_toggle
    }
    /// Number of highest-mean series to keep; `None` keeps all series.
    pub fn top(&self) -> Option<usize> {
        self.top
    }
    /// Whether to fetch MIN/MAX archive bounds around data-source averages.
    pub fn min_max(&self) -> bool {
        self.min_max
    }
    /// Nonzero finite multiplier applied to raw values; defaults to `1.0`.
    pub fn scale(&self) -> f64 {
        self.scale
    }
    /// Raw values above this threshold become unknown before scaling.
    pub fn valid_max(&self) -> Option<f64> {
        self.valid_max
    }
    /// Maximum seconds an unknown point may repeat the previous known value.
    pub fn hold(&self) -> Option<i64> {
        self.hold
    }
    /// Optional axis settings in scaled units.
    pub fn y_axis(&self) -> Option<&YAxis> {
        self.y_axis.as_ref()
    }
}

/// Unvalidated YAML keys for one graph. Hosts may flatten these into their
/// own node descriptions, then supply an id, title and resolved directory.
///
/// Fields remain constructible for hosts that deserialize their own nodes.
/// Call [`GraphFields::build`] before use. When using `serde(flatten)`, the
/// host struct must also use `serde(deny_unknown_fields)` to reject unknown keys.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct GraphFields {
    /// Required path pattern relative to the host's resolved RRD directory.
    pub files: Option<String>,
    /// Single data source shorthand; mutually exclusive with `series`.
    pub ds: Option<String>,
    /// Nonempty list of series definitions; mutually exclusive with `ds`.
    pub series: Option<Vec<SeriesDef>>,
    /// Per-file label template using one-based `{N}` wildcard captures.
    pub label: Option<String>,
    /// Optional display unit passed through to graph data.
    pub unit: Option<String>,
    /// YAML `type`, defaulting to [`GraphType::Line`].
    #[serde(rename = "type")]
    pub graph_type: Option<GraphType>,
    /// YAML `stack-toggle`, defaulting to `false`.
    pub stack_toggle: Option<bool>,
    /// Positive count of highest-mean series to keep; unset keeps all.
    pub top: Option<usize>,
    /// YAML `min-max`, defaulting to `false`.
    pub min_max: Option<bool>,
    /// Nonzero finite value multiplier, defaulting to `1.0`.
    pub scale: Option<f64>,
    /// YAML `valid-max`: optional finite ceiling on raw values.
    pub valid_max: Option<f64>,
    /// Positive seconds or duration text parsed by [`parse_duration`].
    pub hold: Option<serde_yaml::Value>,
    /// YAML `y-axis`: optional bounds and major/minor tick intervals.
    pub y_axis: Option<YAxis>,
}

impl GraphFields {
    /// Parse a standalone graph's YAML keys; no groups or navigation are needed.
    ///
    /// This decodes input only; call [`Self::build`] for semantic validation.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Yaml`] for malformed YAML, unknown keys or
    /// incorrect value types. The diagnostic file path is empty.
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
    ///
    /// `at` identifies the host node in diagnostics. `dir` is already resolved
    /// by the host; this method does not read configuration or RRD files.
    /// Defaults are applied once and the returned [`Graph`] is read-only.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `dir` is absent, required keys
    /// are missing, file/label patterns are invalid, series operations or
    /// styles conflict, or numeric/duration/axis settings fail validation.
    /// The error retains `at` unchanged.
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
///
/// Accepts positive integer YAML numbers or trimmed strings. String amounts
/// contain decimal digits and an optional unit with no intervening spaces.
///
/// # Errors
///
/// Returns [`DurationError::Invalid`] for other YAML types, malformed or
/// nonpositive values, unsupported units, or multiplication beyond `i64`.
///
/// ```
/// use kal_rrd_graph::config::parse_duration;
/// assert_eq!(parse_duration(&serde_yaml::Value::String("2h".into()))?, 7200);
/// # Ok::<(), kal_rrd_graph::config::DurationError>(())
/// ```
pub fn parse_duration(value: &serde_yaml::Value) -> Result<i64, DurationError> {
    let bad = || DurationError::Invalid {
        value: value.clone(),
    };
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

/// A rejected YAML duration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DurationError {
    /// A value is malformed, nonpositive, or cannot fit in signed seconds.
    #[error("expected seconds or a duration like `30m`, `2h`, `1d`, got {value:?}")]
    Invalid {
        /// Original rejected YAML value, retained for diagnostics.
        value: serde_yaml::Value,
    },
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
