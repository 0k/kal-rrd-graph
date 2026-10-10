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

#[test]
fn invalid_hold_keeps_the_original_error_path() {
    let fields = GraphFields::parse("files: a.rrd\nds: v\nhold: 2 weeks").unwrap();
    assert_eq!(fields.build("one".into(), "One".into(), "Root > One", Some(PathBuf::from("."))).unwrap_err().to_string(), "Root > One: `hold`: expected seconds or a duration like `30m`, `2h`, `1d`, got String(\"2 weeks\")");
}
