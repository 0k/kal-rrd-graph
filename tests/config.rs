//! Standalone graph validation contracts extracted from a host application's config tests.
use kal_rrd_graph::{GraphFields, GraphType};
use std::path::PathBuf;

fn graph(yaml: &str) -> kal_rrd_graph::Graph {
    GraphFields::parse(yaml)
        .unwrap()
        .build(
            "one".into(),
            "One".into(),
            "Root > One",
            Some(PathBuf::from(".")),
        )
        .unwrap()
}

#[test]
fn graph_needs_exactly_one_of_ds_and_series() {
    for (yaml, expected) in [
        ("files: a.rrd", "Root > One: a graph needs `ds` or `series`"),
        (
            "files: a.rrd\nds: v\nseries: [{ds: w}]",
            "Root > One: set either `ds` or `series`, not both",
        ),
        (
            "files: a.rrd\nseries: []",
            "Root > One: `series` must not be empty",
        ),
    ] {
        assert_eq!(
            GraphFields::parse(yaml)
                .unwrap()
                .build(
                    "one".into(),
                    "One".into(),
                    "Root > One",
                    Some(PathBuf::from("."))
                )
                .unwrap_err()
                .to_string(),
            expected
        );
    }
}

#[test]
fn validation_names_the_offending_graph() {
    for (key, expected) in [
        ("top: 0", "`top` must be at least 1"),
        ("scale: 0", "`scale` must be a finite, non-zero number"),
        ("scale: .nan", "`scale` must be a finite, non-zero number"),
        ("valid-max: .nan", "`valid-max` must be a finite number"),
        (
            "label: '{2}'",
            "`label` uses {2} but `files` has 1 `*` (captures are numbered from 1)",
        ),
    ] {
        let fields = GraphFields::parse(&format!("files: 'c/*/memory.rrd'\nds: v\n{key}")).unwrap();
        assert_eq!(
            fields
                .build(
                    "one".into(),
                    "One".into(),
                    "Root > One",
                    Some(PathBuf::from("."))
                )
                .unwrap_err()
                .to_string(),
            format!("Root > One: {expected}")
        );
    }
}

#[test]
fn graph_defaults_and_stack_toggle_match_yaml() {
    let a = graph("files: a.rrd\nds: v");
    assert_eq!(
        (
            a.graph_type,
            a.stack_toggle,
            a.min_max,
            a.scale,
            a.hold,
            a.valid_max,
            a.top
        ),
        (GraphType::Line, false, false, 1.0, None, None, None)
    );
    for (kind, expected) in [("line", GraphType::Line), ("stacked", GraphType::Stacked)] {
        let b = graph(&format!(
            "files: a.rrd\nds: v\ntype: {kind}\nstack-toggle: true"
        ));
        assert_eq!((b.graph_type, b.stack_toggle), (expected, true));
    }
}

#[test]
fn hold_accepts_seconds_or_a_duration_with_unit() {
    assert_eq!(graph("files: a.rrd\nds: v\nhold: 90").hold, Some(90));
    let b = graph("files: a.rrd\nds: v\nhold: 2h\nvalid-max: 1e10");
    assert_eq!((b.hold, b.valid_max), (Some(7200), Some(1e10)));
}

#[test]
fn unknown_keys_and_series_keys_are_rejected() {
    for yaml in [
        "files: a.rrd\nds: v\ncolour: red",
        "files: a.rrd\nseries: [{ds: v, colour: red}]",
        "files: a.rrd\nds: v\nchildren: []",
    ] {
        assert!(GraphFields::parse(yaml).is_err(), "{yaml}");
    }
}

fn build_error(yaml: &str) -> String {
    match GraphFields::parse(yaml) {
        Err(e) => e.to_string(),
        Ok(fields) => fields
            .build(
                "one".into(),
                "One".into(),
                "Root > One",
                Some(PathBuf::from(".")),
            )
            .unwrap_err()
            .to_string(),
    }
}

const FRISQUET: &str = indoc::indoc! {r##"
    files: zones/*/temperature.rrd
    label: "{1}"
    unit: "°C"
    y-axis: {interval: 1, minor-interval: 0.5}
    series:
      - {ds: conf, label: confort, color: "#ffff00", fill: 0.1}
      - {ds: target, label: target, color: orange, dash: dashed, width: 1}
      - {ds: sel, label: selected, color: red, dash: dotted, width: 3}
      - label: chauffe
        band: {lower: current, upper: sel, pattern: hatch}
        color: red
      - label: heating
        diff: [sel, current]
        floor: 0
        color: red
"##};

#[test]
fn drawing_keys_build_validated_series_and_y_axis() {
    use kal_rrd_graph::config::{BandPattern, Dash, SeriesKind, YAxis};
    let g = graph(FRISQUET);
    assert_eq!(
        g.y_axis,
        Some(YAxis {
            min: None,
            max: None,
            interval: Some(1.0),
            minor_interval: Some(0.5)
        })
    );
    let kinds: Vec<_> = g.series.iter().map(|s| s.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            SeriesKind::Ds("conf".into()),
            SeriesKind::Ds("target".into()),
            SeriesKind::Ds("sel".into()),
            SeriesKind::Band {
                lower: "current".into(),
                upper: "sel".into()
            },
            SeriesKind::Diff {
                minuend: "sel".into(),
                subtrahend: "current".into(),
                floor: Some(0.0)
            },
        ]
    );
    let styles: Vec<_> = g
        .series
        .iter()
        .map(|s| {
            (
                s.style.color.as_deref(),
                s.style.dash,
                s.style.width,
                s.style.fill,
                s.style.band.map(|b| b.pattern),
                s.style.line,
            )
        })
        .collect();
    assert_eq!(
        styles,
        vec![
            (Some("#ffff00"), None, None, Some(0.1), None, None),
            (
                Some("orange"),
                Some(Dash::Dashed),
                Some(1.0),
                None,
                None,
                None
            ),
            (Some("red"), Some(Dash::Dotted), Some(3.0), None, None, None),
            (
                Some("red"),
                None,
                None,
                None,
                Some(BandPattern::Hatch),
                Some(false)
            ),
            (Some("red"), None, None, None, None, None),
        ]
    );
    assert_eq!(graph("files: a.rrd\nds: v").y_axis, None);
}

#[test]
fn invalid_drawing_keys_name_the_graph_and_series() {
    let base = "files: a.rrd\n";
    for (yaml, expected) in [
        ("series: [{ds: v, width: 0}]", "`series` #1: `width` must be a finite number above 0"),
        ("series: [{ds: v, fill: 0}]", "`series` #1: `fill` must be in (0, 1]"),
        ("series: [{ds: v, fill: 1.5}]", "`series` #1: `fill` must be in (0, 1]"),
        ("series: [{ds: v, color: 'reddish'}]", "`series` #1: `color` must be `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or a CSS color name, got \"reddish\""),
        ("series: [{ds: v, color: '#12345'}]", "`series` #1: `color` must be `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or a CSS color name, got \"#12345\""),
        ("series: [{ds: v}, {label: x}]", "`series` #2: set exactly one of `ds`, `band` or `diff`"),
        ("series: [{ds: v, diff: [a, b], label: x}]", "`series` #1: set exactly one of `ds`, `band` or `diff`"),
        ("series: [{band: {lower: a, upper: b}}]", "`series` #1: a `band` series needs a `label`"),
        ("series: [{diff: [a, b]}]", "`series` #1: a `diff` series needs a `label`"),
        ("series: [{label: x, diff: [a]}]", "`series` #1: `diff` must list exactly two data sources"),
        ("series: [{ds: v, floor: 0}]", "`series` #1: `floor` is only valid on a `diff` series"),
        ("series: [{label: x, band: {lower: a, upper: b, opacity: 0}}]", "`series` #1: band `opacity` must be in (0, 1]"),
        ("series: [{label: x, band: {lower: a, upper: b}, width: 2}]", "`series` #1: a `band` series draws no line: `dash`, `width` and `fill` are not allowed"),
        ("series: [{label: x, diff: [a, b], floor: .nan}]", "`series` #1: `floor` must be a finite number"),
        ("ds: v\ny-axis: {interval: 1, minor-interval: 0.3}", "`y-axis`: `minor-interval` must divide `interval` into an integer count of at least 2"),
        ("ds: v\ny-axis: {interval: 1, minor-interval: 1}", "`y-axis`: `minor-interval` must divide `interval` into an integer count of at least 2"),
        ("ds: v\ny-axis: {minor-interval: 0.5}", "`y-axis`: `minor-interval` needs `interval`"),
        ("ds: v\ny-axis: {interval: 0}", "`y-axis`: `interval` must be a finite number above 0"),
        ("ds: v\ny-axis: {min: 5, max: 5}", "`y-axis`: `min` must be below `max`"),
        ("ds: v\ny-axis: {max: .inf}", "`y-axis`: `max` must be a finite number"),
        ("type: stacked\nseries: [{ds: v, fill: 0.1}]", "`series` #1: `fill`, `band` and `diff` cannot be combined with `type: stacked` or `stack-toggle`"),
        ("stack-toggle: true\nseries: [{ds: v}, {label: x, band: {lower: a, upper: v}}]", "`series` #2: `fill`, `band` and `diff` cannot be combined with `type: stacked` or `stack-toggle`"),
        ("top: 2\nseries: [{label: x, diff: [a, b]}]", "`series` #1: `band` and `diff` cannot be combined with `top`"),
    ] {
        assert_eq!(
            build_error(&format!("{base}{yaml}")),
            format!("Root > One: {expected}"),
            "{yaml}"
        );
    }
    for yaml in [
        "series: [{ds: v, dash: wavy}]",
        "series: [{label: x, band: {lower: a, upper: b, pattern: dots}}]",
        "series: [{label: x, band: {lower: a}}]",
        "ds: v\ny-axis: {step: 1}",
    ] {
        assert!(
            GraphFields::parse(&format!("{base}{yaml}")).is_err(),
            "{yaml}"
        );
    }
}

#[test]
fn frisquet_example_is_a_valid_graph() {
    let g = graph(include_str!("../examples/frisquet.yaml"));
    assert_eq!(g.series.len(), 8);
}

#[test]
fn y_axis_is_a_graph_only_key() {
    let fields = GraphFields::parse("y-axis: {interval: 2}").unwrap();
    assert_eq!(fields.graph_only_keys(), ["y-axis"]);
}

#[test]
fn invalid_hold_keeps_the_original_error_path() {
    let fields = GraphFields::parse("files: a.rrd\nds: v\nhold: 2 weeks").unwrap();
    assert_eq!(fields.build("one".into(), "One".into(), "Root > One", Some(PathBuf::from("."))).unwrap_err().to_string(), "Root > One: `hold`: expected seconds or a duration like `30m`, `2h`, `1d`, got String(\"2 weeks\")");
}
