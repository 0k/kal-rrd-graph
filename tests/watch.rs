//! Watch-only host contracts, independent of the HTTP adapter.

use std::sync::Arc;
use std::time::Duration;

use kal_rrd_graph::watch::{GraphWatch, WatchError, WatchTiming};
use kal_rrd_graph::{Graph, GraphFields, GraphSource, Lookup};

struct Single(Graph);

impl GraphSource for Single {
    fn lookup(&self, id: &str) -> Lookup {
        if id == self.0.id() {
            Lookup::Graph(Box::new(self.0.clone()))
        } else {
            Lookup::NotFound
        }
    }
}

#[test]
fn standalone_watch_registers_sorted_deduplicated_interests() {
    let dir = tempfile::tempdir().unwrap();
    let graph = GraphFields::parse("files: values.rrd\nds: value")
        .unwrap()
        .build("g".into(), "G".into(), "G", Some(dir.path().into()))
        .unwrap();
    let watch = GraphWatch::new(Arc::new(Single(graph))).unwrap();
    let (snapshot, sub, index) = watch.subscribe(None, vec!["g".into(), "g".into()]).unwrap();
    assert_eq!(snapshot.ids, ["g"]);
    let (snapshot, same_sub, same_index) = watch.subscribe(Some(sub.clone()), vec![]).unwrap();
    assert_eq!((snapshot.ids, same_sub, same_index), (vec![], sub, index));
    let error = watch.subscribe(None, vec!["missing".into()]).unwrap_err();
    assert!(matches!(&error, WatchError::NotFound { id } if id == "missing"));
    assert_eq!(error.to_string(), "no graph \"missing\"");
}

#[test]
fn standalone_watch_rejects_each_zero_timing() {
    let graph = GraphFields::parse("files: values.rrd\nds: value")
        .unwrap()
        .build("g".into(), "G".into(), "G", Some(".".into()))
        .unwrap();
    let source: Arc<dyn GraphSource> = Arc::new(Single(graph));
    for timing in [
        WatchTiming {
            settle: Duration::ZERO,
            ..WatchTiming::default()
        },
        WatchTiming {
            grace: Duration::ZERO,
            ..WatchTiming::default()
        },
        WatchTiming {
            tick: Duration::ZERO,
            ..WatchTiming::default()
        },
    ] {
        let error = GraphWatch::with_timing(source.clone(), timing).unwrap_err();
        assert!(matches!(error, WatchError::NonzeroTiming));
        assert_eq!(error.to_string(), "graph watch timing must be nonzero");
    }
}
