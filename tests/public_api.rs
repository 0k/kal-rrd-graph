//! Host-facing metadata and typed failure contracts.

use indoc::indoc;
use kal_rrd_graph::config::{parse_duration, DurationError};
use kal_rrd_graph::pattern::{FilePattern, PatternError};
use kal_rrd_graph::{GraphFields, GraphType, SeriesKind};

#[test]
fn validated_metadata_is_readable_without_mutation() {
    let graph = GraphFields::parse(indoc! {"
        files: zones/*/temperature.rrd
        ds: value
        label: 'zone {1}'
        unit: degrees
        stack-toggle: true
        y-axis: {min: 0, max: 100, interval: 10, minor-interval: 5}
    "})
    .unwrap()
    .build(
        "boiler".into(),
        "Boiler".into(),
        "Boiler",
        Some("/rrd".into()),
    )
    .unwrap();
    assert_eq!(
        (graph.id(), graph.title(), graph.unit()),
        ("boiler", "Boiler", Some("degrees"))
    );
    assert_eq!(graph.rrd_dir(), std::path::Path::new("/rrd"));
    assert_eq!(graph.files().as_str(), "zones/*/temperature.rrd");
    assert_eq!(graph.label(), Some("zone {1}"));
    assert_eq!(graph.graph_type(), GraphType::Line);
    assert!(graph.stack_toggle());
    assert_eq!(graph.series().len(), 1);
    let series = &graph.series()[0];
    assert_eq!(series.label(), None);
    assert_eq!(series.own_label(), "value");
    assert_eq!(series.kind(), &SeriesKind::Ds("value".into()));
    assert_eq!(series.style().color(), None);
    let axis = graph.y_axis().unwrap();
    assert_eq!(
        (
            axis.min(),
            axis.max(),
            axis.interval(),
            axis.minor_interval()
        ),
        (Some(0.0), Some(100.0), Some(10.0), Some(5.0))
    );
}

#[test]
fn pattern_failures_are_typed_with_original_display_texts() {
    for (pattern, message) in [
        ("", "pattern is empty"),
        (
            "/a",
            "\"/a\" must be relative to `rrd-dir` (no leading `/`)",
        ),
        (
            "a/../b",
            "\"a/../b\" has an empty, `.` or `..` path component",
        ),
    ] {
        let error = FilePattern::parse(pattern).unwrap_err();
        assert_eq!(error.to_string(), message);
        assert!(matches!(
            error,
            PatternError::Empty
                | PatternError::Absolute { .. }
                | PatternError::InvalidComponent { .. }
        ));
    }
}

#[test]
fn duration_failures_are_typed_with_original_display_texts() {
    let value = serde_yaml::Value::String("2 weeks".into());
    let error = parse_duration(&value).unwrap_err();
    assert!(matches!(&error, DurationError::Invalid { value: rejected } if rejected == &value));
    assert_eq!(
        error.to_string(),
        "expected seconds or a duration like `30m`, `2h`, `1d`, got String(\"2 weeks\")"
    );
}
