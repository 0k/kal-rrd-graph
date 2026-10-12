//! Turning one configured [`Graph`] into the series the frontend draws.
//!
//! For a time window, every file matched by the graph's `files` pattern
//! is read through librrd (the `rrd` crate), the requested data sources
//! are extracted, and all series are put on ONE common time grid so the
//! frontend can stack and compare them point by point.
//!
//! Missing data is reported, never fatal: a pattern that matches no
//! file, a file without the requested data source, or an unreadable
//! file each add a human-readable note to the result and the rest of
//! the graph is still drawn.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use rrd::ConsolidationFn;
use serde::Serialize;

use crate::config::{label_tokens, Graph, GraphType, LabelToken, SeriesKind, SeriesStyle, YAxis};

/// Time window requested by the frontend (UNIX seconds, `end` > `start`).
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub start: i64,
    pub end: i64,
    /// Requested seconds per point (at least 1).
    pub resolution: i64,
}

impl Window {
    /// Seconds per point the caller asked for (at least 1).
    pub fn from_points(start: i64, end: i64, points: usize) -> Self {
        let span = end.saturating_sub(start).max(1);
        let points = i64::try_from(points.max(1)).unwrap_or(i64::MAX);
        Self::from_step(start, end, span / points + i64::from(span % points != 0))
    }

    /// Preserve a previous response's lattice when fetching a tail.
    pub fn from_step(start: i64, end: i64, step: i64) -> Self {
        Self {
            start,
            end,
            resolution: step.max(1),
        }
    }

    fn lookback(self, hold: Option<i64>) -> Self {
        Self {
            start: self.start.saturating_sub(hold.unwrap_or(0).max(0)),
            ..self
        }
    }

    pub(crate) fn in_range(self, hold: Option<i64>) -> bool {
        self.end > self.start && grid(self, self.resolution.max(1), hold).is_some()
    }
}

fn grid(window: Window, step: i64, hold: Option<i64>) -> Option<(i64, i64, usize, usize)> {
    window.end.checked_sub(window.start)?;
    window.start.checked_sub(1)?;
    let last = window.end.checked_add(step)?;
    UNIX_EPOCH.checked_add(Duration::from_secs(last.max(0) as u64))?;
    let original_g0 = window.start.div_euclid(step).checked_mul(step)?;
    let read_start = window.start.checked_sub(hold.unwrap_or(0).max(0))?;
    let g0 = read_start.div_euclid(step).checked_mul(step)?;
    let span = window.end.checked_sub(g0)?.checked_add(step - 1)?;
    let len = usize::try_from((span / step).max(0)).ok()?;
    let trim = usize::try_from(original_g0.checked_sub(g0)? / step).ok()?;
    Some((original_g0.checked_add(step)?, g0, len, trim))
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeriesData {
    pub label: String,
    pub avg: Vec<Option<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<Vec<Option<f64>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Vec<Option<f64>>>,
    /// Drawing keys from the YAML; absent from JSON when unset.
    #[serde(flatten)]
    pub style: SeriesStyle,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphData {
    pub id: String,
    pub title: String,
    pub unit: Option<String>,
    #[serde(rename = "type")]
    pub graph_type: GraphType,
    /// Time of the first point; point `i` is at `start + i * step`.
    /// Each point summarises the `step` seconds that END at its time.
    pub start: i64,
    pub step: i64,
    pub len: usize,
    pub series: Vec<SeriesData>,
    /// Most recent update time over the graph's files.
    pub last_update: Option<i64>,
    /// Why some (or all) of the expected data is missing.
    pub notes: Vec<String>,
    /// The graph's `y-axis`, omitted when not configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_axis: Option<YAxis>,
}

/// One series read from one file, before alignment on the common grid.
#[derive(Debug, Clone)]
pub struct RawSeries {
    pub label: String,
    pub step: i64,
    /// `(time, avg, min, max)`; time is the END of the RRD interval.
    pub samples: Vec<(i64, f64, f64, f64)>,
}

/// Read and assemble `graph` over `window`.
pub fn build(graph: &Graph, window: Window, keep: Option<&BTreeSet<String>>) -> GraphData {
    let mut notes = Vec::new();
    let mut raws = Vec::new();
    let mut last_update = None;

    if !window.in_range(graph.hold) {
        return GraphData {
            id: graph.id.clone(),
            title: graph.title.clone(),
            unit: graph.unit.clone(),
            graph_type: graph.graph_type,
            start: window.start,
            step: window.resolution.max(1),
            len: 0,
            series: Vec::new(),
            last_update: None,
            notes: vec!["`start` or `end` is out of range".into()],
            y_axis: graph.y_axis,
        };
    }

    let matches = match graph.files.expand(&graph.rrd_dir) {
        Ok(m) => m,
        Err(e) => {
            notes.push(format!(
                "cannot list {}: {e}",
                graph.rrd_dir.join(graph.files.as_str()).display()
            ));
            Vec::new()
        }
    };
    if matches.is_empty() && notes.is_empty() {
        notes.push(format!(
            "no file matches {} in {}",
            graph.files.as_str(),
            graph.rrd_dir.display()
        ));
    }

    let resolution = window.resolution;
    let read_window = window.lookback(graph.hold);
    // Every data source any series reads, once, in first-use order.
    let mut needed: Vec<&str> = Vec::new();
    for ds in graph.series.iter().flat_map(|s| s.kind.inputs()) {
        if !needed.contains(&ds) {
            needed.push(ds);
        }
    }
    let mut files = Vec::new();
    for m in &matches {
        let rel = m
            .path
            .strip_prefix(&graph.rrd_dir)
            .unwrap_or(&m.path)
            .display()
            .to_string();
        if let Some(t) = last_update_of(&m.path) {
            last_update = Some(last_update.map_or(t, |l: i64| l.max(t)));
        }
        let file_label = file_label(graph.label.as_deref(), &m.captures);
        match read_file(
            &m.path,
            &needed,
            graph.min_max,
            read_window,
            resolution,
            &mut notes,
            &rel,
        ) {
            Ok(per_ds) => {
                // Data source -> index of its aligned column in `raws`.
                let mut columns = BTreeMap::new();
                for (ds, read) in needed.iter().zip(per_ds) {
                    if let Some((step, samples)) = read {
                        columns.insert(*ds, raws.len());
                        raws.push(RawSeries {
                            label: ds.to_string(),
                            step,
                            samples,
                        });
                    }
                }
                let mut noted = BTreeSet::new();
                for def in &graph.series {
                    let own = matches!(def.kind, SeriesKind::Ds(_));
                    for ds in def.kind.inputs() {
                        // A `ds` series reports its own absence, as it
                        // always has; derived series only add new names.
                        if !columns.contains_key(ds) && (noted.insert(ds) || own) {
                            notes.push(format!("{rel}: no data source `{ds}`"));
                        }
                    }
                }
                files.push((file_label, columns));
            }
            Err(e) => notes.push(format!("{rel}: {e}")),
        }
    }

    let cleanup = Cleanup {
        scale: graph.scale,
        min_max: graph.min_max,
        valid_max: graph.valid_max,
        hold: graph.hold,
    };
    let aligned = align(&raws, window, resolution, &cleanup);
    let mut drawn = Vec::new();
    for (file_label, columns) in &files {
        for def in &graph.series {
            let Some(inputs) = def
                .kind
                .inputs()
                .iter()
                .map(|ds| columns.get(ds).and_then(|&i| aligned.series.get(i)))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let label = series_label(file_label, def.own_label(), graph.series.len());
            let style = def.style.clone();
            drawn.push(match &def.kind {
                SeriesKind::Ds(_) => SeriesData {
                    label,
                    style,
                    ..inputs[0].clone()
                },
                SeriesKind::Band { .. } => {
                    let (avg, min, max) = band_values(&inputs[0].avg, &inputs[1].avg);
                    SeriesData {
                        label,
                        avg,
                        min: Some(min),
                        max: Some(max),
                        style,
                    }
                }
                SeriesKind::Diff { floor, .. } => SeriesData {
                    label,
                    avg: diff_values(&inputs[0].avg, &inputs[1].avg, *floor),
                    min: None,
                    max: None,
                    style,
                },
            });
        }
    }
    let series = match (graph.top, keep) {
        (Some(_), Some(keep)) => keep_labels(drawn, keep),
        (Some(n), None) => keep_top(drawn, n),
        (None, _) => drawn,
    };
    GraphData {
        id: graph.id.clone(),
        title: graph.title.clone(),
        unit: graph.unit.clone(),
        graph_type: graph.graph_type,
        start: aligned.start,
        step: aligned.step,
        len: aligned.len,
        series,
        last_update,
        notes,
        y_axis: graph.y_axis,
    }
}

/// `(avg, min, max)` of a band: the area `lower..upper` at points where
/// both are known and `lower <= upper`, unknown elsewhere.  `avg` is the
/// upper bound so the frontend draws the band (it skips unknown averages)
/// while the series itself has no line.
pub fn band_values(lower: &[Option<f64>], upper: &[Option<f64>]) -> BandValues {
    let points: Vec<_> = lower
        .iter()
        .zip(upper)
        .map(|pair| match pair {
            (Some(lo), Some(hi)) if lo <= hi => Some((*lo, *hi)),
            _ => None,
        })
        .collect();
    let hi: Vec<_> = points.iter().map(|p| p.map(|(_, hi)| hi)).collect();
    let lo = points.iter().map(|p| p.map(|(lo, _)| lo)).collect();
    (hi.clone(), lo, hi)
}

pub type BandValues = (Vec<Option<f64>>, Vec<Option<f64>>, Vec<Option<f64>>);

/// `a - b` where both are known, clamped below at `floor`.
pub fn diff_values(a: &[Option<f64>], b: &[Option<f64>], floor: Option<f64>) -> Vec<Option<f64>> {
    a.iter()
        .zip(b)
        .map(|pair| match pair {
            (Some(a), Some(b)) => Some(floor.map_or(a - b, |f| (a - b).max(f))),
            _ => None,
        })
        .collect()
}

fn last_update_of(path: &Path) -> Option<i64> {
    let info = rrd::ops::info::info(path).ok()?;
    info.get("last_update")?
        .clone()
        .into_count()
        .map(|t| t as i64)
}

type DsSamples = Option<(i64, Vec<(i64, f64, f64, f64)>)>;

/// Fetch every requested data source of one file.  Returns, per
/// `names` entry, `None` when the file has no such data source.
fn read_file(
    path: &Path,
    names: &[&str],
    min_max: bool,
    window: Window,
    resolution: i64,
    notes: &mut Vec<String>,
    rel: &str,
) -> Result<Vec<DsSamples>, String> {
    let avg = fetch(path, ConsolidationFn::Avg, window, resolution)?;
    let info = min_max
        .then(|| rrd::ops::info::info(path))
        .transpose()
        .map_err(|e| e.to_string())?;
    let band = |cf, name: &str, notes: &mut Vec<String>| {
        // librrd can fall back to AVERAGE when the requested CF has no
        // archive at the requested resolution. Require an actual CF archive.
        let finest = info.as_ref().and_then(|info| {
            let base = info.get("step")?.clone().into_count()?;
            info.iter()
                .filter_map(|(key, value)| {
                    if !key.starts_with("rra[")
                        || !key.ends_with(".cf")
                        || value.clone().into_string().as_deref() != Some(name)
                    {
                        return None;
                    }
                    let key = format!("{}pdp_per_row", key.strip_suffix("cf")?);
                    let step = info.get(&key)?.clone().into_count()?.checked_mul(base)?;
                    i64::try_from(step).ok()
                })
                .min()
        });
        let fetched = finest.and_then(|step| fetch(path, cf, window, resolution.max(step)).ok());
        match fetched {
            Some(f)
                if f.rows
                    .iter()
                    .any(|(_, row)| row.iter().any(|v| v.is_finite())) =>
            {
                Some(f)
            }
            _ => {
                notes.push(format!("{rel}: no {name} archive"));
                None
            }
        }
    };
    let (min, max) = if min_max {
        (
            band(ConsolidationFn::Min, "MIN", notes),
            band(ConsolidationFn::Max, "MAX", notes),
        )
    } else {
        (None, None)
    };
    Ok(names
        .iter()
        .map(|name| {
            let col = avg.names.iter().position(|n| n == name)?;
            let other = |f: &Option<Fetched>, t: i64| -> f64 {
                f.as_ref()
                    .and_then(|f| {
                        let col = f.names.iter().position(|n| n == name)?;
                        let i = f.rows.partition_point(|(end, _)| *end < t);
                        let (end, values) = f.rows.get(i)?;
                        (i128::from(*end) - i128::from(f.step) < i128::from(t))
                            .then(|| values.get(col).copied())
                            .flatten()
                    })
                    .unwrap_or(f64::NAN)
            };
            let samples = avg
                .rows
                .iter()
                .map(|(t, values)| (*t, values[col], other(&min, *t), other(&max, *t)))
                .collect();
            Some((avg.step, samples))
        })
        .collect())
}

struct Fetched {
    names: Vec<String>,
    step: i64,
    rows: Vec<(i64, Vec<f64>)>,
}

fn fetch(
    path: &Path,
    cf: ConsolidationFn,
    window: Window,
    resolution: i64,
) -> Result<Fetched, String> {
    let at = |t: i64| UNIX_EPOCH + Duration::from_secs(t.max(0) as u64);
    let data = rrd::ops::fetch::fetch(
        path,
        cf,
        at(window.start),
        at(window.end),
        Duration::from_secs(resolution as u64),
    )
    .map_err(|e| e.to_string())?;
    let secs = |t: std::time::SystemTime| {
        t.duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };
    Ok(Fetched {
        names: data.ds_names().to_vec(),
        step: data.step().as_secs() as i64,
        rows: data
            .rows()
            .iter()
            .map(|row| (secs(row.timestamp()), row.as_slice().to_vec()))
            .collect(),
    })
}

/// Render the per-file part of a label: the template, or by default the
/// captures joined with `/`.
fn file_label(template: Option<&str>, captures: &[String]) -> String {
    match template {
        None => captures.join("/"),
        Some(t) => label_tokens(t)
            .unwrap_or_else(|_| vec![LabelToken::Text(t)])
            .into_iter()
            .map(|tok| match tok {
                LabelToken::Text(s) => s.to_string(),
                LabelToken::Capture(n) => n
                    .checked_sub(1)
                    .and_then(|i| captures.get(i))
                    .cloned()
                    .unwrap_or_default(),
            })
            .collect(),
    }
}

fn series_label(file_label: &str, own: &str, defs: usize) -> String {
    if file_label.is_empty() {
        own.to_string()
    } else if defs == 1 {
        file_label.to_string()
    } else {
        format!("{file_label} {own}")
    }
}

/// Series put on a common grid.
#[derive(Debug, PartialEq)]
pub struct Aligned {
    pub start: i64,
    pub step: i64,
    pub len: usize,
    pub series: Vec<SeriesData>,
}

/// Per-graph value cleanup applied while aligning (see the `scale`,
/// `min-max`, `valid-max` and `hold` config keys).
#[derive(Debug, Clone, Copy)]
pub struct Cleanup {
    pub scale: f64,
    pub min_max: bool,
    /// Raw values above this are unknown (e.g. a counter reset read by
    /// librrd as a wrap).
    pub valid_max: Option<f64>,
    /// Seconds an unknown point may repeat the last known value.
    pub hold: Option<i64>,
}

/// Put `raws` on one grid whose step is the coarsest of the requested
/// resolution and of every file's own step.  Bucket `i` covers
/// `(g0 + i*step, g0 + (i+1)*step]` and is reported at its end time.
pub fn align(raws: &[RawSeries], window: Window, resolution: i64, cleanup: &Cleanup) -> Aligned {
    let Cleanup {
        scale,
        min_max,
        valid_max,
        hold,
    } = *cleanup;
    let valid = |v: f64| !v.is_nan() && valid_max.is_none_or(|m| v <= m);
    let step = raws
        .iter()
        .map(|r| r.step)
        .fold(resolution, i64::max)
        .max(1);
    let Some((start, g0, len, trim)) = grid(window, step, hold) else {
        return Aligned {
            start: window.start,
            step,
            len: 0,
            series: Vec::new(),
        };
    };
    let series = raws
        .iter()
        .map(|raw| {
            let mut sum = vec![0.0; len];
            let mut count = vec![0usize; len];
            let mut min = vec![f64::INFINITY; len];
            let mut max = vec![f64::NEG_INFINITY; len];
            for &(t, a, lo, hi) in &raw.samples {
                let Some(offset) = t.checked_sub(1).and_then(|t| t.checked_sub(g0)) else {
                    continue;
                };
                let idx = offset.div_euclid(step);
                if idx < 0 || idx as usize >= len {
                    continue;
                }
                let i = idx as usize;
                if valid(a) {
                    sum[i] += a * scale;
                    count[i] += 1;
                }
                if valid(lo) {
                    min[i] = min[i].min(lo * scale);
                }
                if valid(hi) {
                    max[i] = max[i].max(hi * scale);
                }
            }
            let avg = (0..len)
                .map(|i| (count[i] > 0).then(|| sum[i] / count[i] as f64))
                .collect();
            let finite = |v: Vec<f64>| v.into_iter().map(|x| x.is_finite().then_some(x)).collect();
            let fill = |v: Vec<Option<f64>>| {
                let filled = match hold {
                    Some(h) => hold_values(v, step, h),
                    None => v,
                };
                filled.into_iter().skip(trim).collect()
            };
            SeriesData {
                label: raw.label.clone(),
                avg: fill(avg),
                min: min_max.then(|| fill(finite(min))),
                max: min_max.then(|| fill(finite(max))),
                style: SeriesStyle::default(),
            }
        })
        .collect();
    Aligned {
        start,
        step,
        len: len.saturating_sub(trim),
        series,
    }
}

/// Repeat the last known value over following unknown points whose
/// time is at most `hold` seconds after it.
fn hold_values(mut values: Vec<Option<f64>>, step: i64, hold: i64) -> Vec<Option<f64>> {
    let mut last: Option<(usize, f64)> = None;
    for (i, v) in values.iter_mut().enumerate() {
        match (*v, last) {
            (Some(x), _) => last = Some((i, x)),
            (None, Some((j, x))) if (i - j) as i128 * i128::from(step) <= i128::from(hold) => {
                *v = Some(x)
            }
            (None, _) => {}
        }
    }
    values
}

fn mean(values: &[Option<f64>]) -> f64 {
    let (sum, n) = values
        .iter()
        .flatten()
        .fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        f64::NEG_INFINITY
    } else {
        sum / n as f64
    }
}

/// Keep the `n` series with the largest mean over the window (largest
/// first) and sum the others, point by point, into one `other (k)`
/// series.
pub fn keep_top(mut series: Vec<SeriesData>, n: usize) -> Vec<SeriesData> {
    if series.len() <= n {
        return series;
    }
    series.sort_by(|a, b| {
        mean(&b.avg)
            .partial_cmp(&mean(&a.avg))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.label.cmp(&b.label))
    });
    let rest = series.split_off(n);
    append_other(series, rest)
}

/// Preserve a full window's chosen labels instead of ranking a short tail.
pub fn keep_labels(series: Vec<SeriesData>, keep: &BTreeSet<String>) -> Vec<SeriesData> {
    let (kept, rest) = series.into_iter().partition(|s| keep.contains(&s.label));
    append_other(kept, rest)
}

fn append_other(mut series: Vec<SeriesData>, rest: Vec<SeriesData>) -> Vec<SeriesData> {
    let Some(len) = rest.iter().map(|s| s.avg.len()).max() else {
        return series;
    };
    let other = (0..len)
        .map(|i| {
            let values: Vec<f64> = rest
                .iter()
                .filter_map(|s| s.avg.get(i).copied().flatten())
                .collect();
            (!values.is_empty()).then(|| values.iter().sum())
        })
        .collect();
    series.push(SeriesData {
        label: format!("other ({})", rest.len()),
        avg: other,
        min: None,
        max: None,
        style: SeriesStyle::default(),
    });
    series
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_labels_render_without_panicking() {
        let captures = vec!["one".into()];
        assert_eq!(file_label(Some("{"), &captures), "{");
        assert_eq!(file_label(Some("x{0}{2}"), &captures), "x");
    }

    #[test]
    fn unequal_series_lengths_sum_all_available_points() {
        let s = |label: &str, avg| SeriesData {
            label: label.into(),
            avg,
            min: None,
            max: None,
            style: SeriesStyle::default(),
        };
        for series in [
            vec![s("a", vec![Some(1.0)]), s("b", vec![Some(2.0), Some(3.0)])],
            vec![s("b", vec![Some(2.0), Some(3.0)]), s("a", vec![Some(1.0)])],
        ] {
            let expected = vec![s("other (2)", vec![Some(3.0), Some(3.0)])];
            assert_eq!(keep_top(series.clone(), 0), expected);
            assert_eq!(keep_labels(series, &BTreeSet::new()), expected);
        }
    }

    #[test]
    fn extreme_alignment_returns_an_empty_grid_without_panicking() {
        for window in [
            Window::from_points(i64::MIN, i64::MAX, 800),
            Window::from_step(i64::MAX - 7, i64::MAX, 2),
        ] {
            let result = align(&[], window, window.resolution, &plain(false));
            assert_eq!(result.len, 0);
            assert_eq!(result.series, Vec::new());
        }
    }

    fn raw(label: &str, step: i64, samples: &[(i64, f64)]) -> RawSeries {
        RawSeries {
            label: label.into(),
            step,
            samples: samples.iter().map(|&(t, v)| (t, v, v, v)).collect(),
        }
    }

    fn plain(min_max: bool) -> Cleanup {
        Cleanup {
            scale: 1.0,
            min_max,
            valid_max: None,
            hold: None,
        }
    }

    fn window(start: i64, end: i64, points: usize) -> Window {
        Window::from_points(start, end, points)
    }

    #[test]
    fn explicit_step_tail_matches_full_grid_and_hold_values() {
        let raws = [raw("x", 10, &[(10, 5.0), (60, 7.0)])];
        let cleanup = Cleanup {
            hold: Some(25),
            ..plain(true)
        };
        let full = align(&raws, Window::from_step(0, 80, 10), 10, &cleanup);
        let tail = align(
            &raws,
            Window::from_step(20, 80, full.step),
            full.step,
            &cleanup,
        );
        assert_eq!((tail.start, tail.step, tail.len), (30, 10, 6));
        assert_eq!(tail.series[0].avg, full.series[0].avg[2..]);
        assert_eq!(
            tail.series[0].min.as_ref().unwrap(),
            &full.series[0].min.as_ref().unwrap()[2..]
        );
        assert_eq!(
            tail.series[0].max.as_ref().unwrap(),
            &full.series[0].max.as_ref().unwrap()[2..]
        );
        assert_eq!(tail.series[0].avg[0], Some(5.0));
    }

    #[test]
    fn band_keeps_points_where_both_bounds_are_known_and_ordered() {
        let lower = [Some(1.0), Some(2.0), Some(3.0), None, Some(1.0)];
        let upper = [Some(2.0), Some(2.0), Some(2.5), Some(1.0), None];
        let known = vec![Some(2.0), Some(2.0), None, None, None];
        assert_eq!(
            band_values(&lower, &upper),
            (
                known.clone(),
                vec![Some(1.0), Some(2.0), None, None, None],
                known
            )
        );
    }

    #[test]
    fn diff_subtracts_known_points_and_clamps_at_floor() {
        let a = [Some(5.0), Some(1.0), None, Some(2.0)];
        let b = [Some(2.0), Some(3.0), Some(1.0), None];
        assert_eq!(
            diff_values(&a, &b, None),
            vec![Some(3.0), Some(-2.0), None, None]
        );
        assert_eq!(
            diff_values(&a, &b, Some(0.0)),
            vec![Some(3.0), Some(0.0), None, None]
        );
        assert_eq!(
            diff_values(&a, &b, Some(-1.0)),
            vec![Some(3.0), Some(-1.0), None, None]
        );
    }

    #[test]
    fn window_constructors_preserve_resolution_math() {
        assert_eq!(Window::from_points(0, 41, 2).resolution, 21);
        assert_eq!(Window::from_step(0, 41, 10).resolution, 10);
        assert_eq!(Window::from_step(0, 41, 0).resolution, 1);
    }

    #[test]
    fn coarser_file_step_sets_the_grid_step() {
        let a = align(
            &[
                raw("a", 60, &[(60, 1.0), (120, 3.0)]),
                raw("b", 120, &[(120, 5.0)]),
            ],
            window(0, 120, 120),
            1,
            &plain(false),
        );
        assert_eq!((a.start, a.step, a.len), (120, 120, 1));
        assert_eq!(a.series[0].avg, vec![Some(2.0)]);
        assert_eq!(a.series[1].avg, vec![Some(5.0)]);
    }

    #[test]
    fn requested_resolution_downsamples_by_averaging_and_extremes() {
        let samples: Vec<(i64, f64, f64, f64)> = (1..=4)
            .map(|i| (i * 10, i as f64, i as f64 - 1.0, i as f64 + 1.0))
            .collect();
        let a = align(
            &[RawSeries {
                label: "x".into(),
                step: 10,
                samples,
            }],
            window(0, 40, 2),
            20,
            &plain(true),
        );
        assert_eq!(a.series[0].avg, vec![Some(1.5), Some(3.5)]);
        assert_eq!(a.series[0].min, Some(vec![Some(0.0), Some(2.0)]));
        assert_eq!(a.series[0].max, Some(vec![Some(3.0), Some(5.0)]));
    }

    #[test]
    fn unknown_values_stay_gaps_and_scale_applies() {
        let a = align(
            &[raw("x", 10, &[(10, f64::NAN), (20, 2.0)])],
            window(0, 20, 2),
            10,
            &Cleanup {
                scale: 1024.0,
                ..plain(false)
            },
        );
        assert_eq!(a.series[0].avg, vec![None, Some(2048.0)]);
    }

    #[test]
    fn values_above_valid_max_become_unknown_before_averaging() {
        let a = align(
            &[raw("x", 10, &[(10, 1.0), (20, 1e17), (30, 3.0), (40, 4.0)])],
            window(0, 40, 2),
            20,
            &Cleanup {
                scale: 1.0,
                min_max: true,
                valid_max: Some(1e10),
                hold: None,
            },
        );
        assert_eq!(a.series[0].avg, vec![Some(1.0), Some(3.5)]);
        assert_eq!(a.series[0].max, Some(vec![Some(1.0), Some(4.0)]));
    }

    #[test]
    fn hold_carries_the_last_value_through_gaps_up_to_its_duration() {
        let a = align(
            &[raw("x", 10, &[(10, 5.0), (60, 7.0)])],
            window(0, 80, 8),
            10,
            &Cleanup {
                scale: 1.0,
                min_max: false,
                valid_max: None,
                hold: Some(25),
            },
        );
        assert_eq!(
            a.series[0].avg,
            // Points report at their END time: 10, 20, … 80.
            vec![
                Some(5.0),
                Some(5.0),
                Some(5.0),
                None,
                None,
                Some(7.0),
                Some(7.0),
                Some(7.0)
            ]
        );
    }

    #[test]
    fn top_keeps_largest_means_and_sums_the_rest() {
        let s = |label: &str, v: Vec<Option<f64>>| SeriesData {
            label: label.into(),
            avg: v,
            min: None,
            max: None,
            style: SeriesStyle::default(),
        };
        let out = keep_top(
            vec![
                s("small", vec![Some(1.0), Some(1.0)]),
                s("big", vec![Some(9.0), Some(9.0)]),
                s("tiny", vec![None, Some(0.5)]),
                s("mid", vec![Some(5.0), None]),
            ],
            2,
        );
        let labels: Vec<_> = out.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["big", "mid", "other (2)"]);
        assert_eq!(out[2].avg, vec![Some(1.0), Some(1.5)]);
    }

    #[test]
    fn labels_come_from_template_captures_or_default_join() {
        let caps = vec!["myc".to_string(), "odoo".to_string()];
        assert_eq!(file_label(None, &caps), "myc/odoo");
        assert_eq!(file_label(Some("{2} ({1})"), &caps), "odoo (myc)");
        assert_eq!(series_label("", "1 min", 3), "1 min");
        assert_eq!(series_label("web", "1 min", 3), "web 1 min");
        assert_eq!(series_label("web", "1 min", 1), "web");
    }

    #[test]
    fn kept_labels_do_not_rank_the_tail_and_share_other_summing() {
        let s = |label: &str, avg| SeriesData {
            label: label.into(),
            avg,
            min: None,
            max: None,
            style: SeriesStyle::default(),
        };
        let series = vec![
            s("alpha beta", vec![Some(1.0), None]),
            s("gamma", vec![Some(2.0), Some(3.0)]),
            s("large", vec![Some(900.0), None]),
            s("tiny", vec![Some(0.5), None]),
        ];
        let keep = BTreeSet::from(["alpha beta".into(), "gamma".into()]);
        assert_eq!(
            keep_labels(series.clone(), &keep),
            vec![
                series[0].clone(),
                series[1].clone(),
                s("other (2)", vec![Some(900.5), None]),
            ]
        );
        let all = series.iter().map(|s| s.label.clone()).collect();
        assert_eq!(keep_labels(series.clone(), &all), series);
        let missing = BTreeSet::from(["alpha beta".into(), "removed".into()]);
        assert_eq!(
            keep_labels(series.clone(), &missing),
            vec![
                series[0].clone(),
                s("other (3)", vec![Some(902.5), Some(3.0)])
            ]
        );
        assert_eq!(
            keep_labels(series, &BTreeSet::new()),
            vec![s("other (4)", vec![Some(903.5), Some(3.0)])]
        );
        assert_eq!(keep_labels(Vec::new(), &keep), Vec::new());
    }
}
