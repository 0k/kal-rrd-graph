//! Keep the README's Rust blocks identical to examples checked by the compiler.
#![cfg(feature = "api")]

fn compiled_examples() -> Result<(), Box<dyn std::error::Error>> {
    // BEGIN README
    use kal_rrd_graph::{
        graph::{build, Window},
        GraphFields,
    };

    let fields = GraphFields::parse("files: temperature.rrd\nds: value\nunit: °C")?;
    let graph = fields.build(
        "boiler".into(),
        "Boiler".into(),
        "Boiler",
        Some("/var/lib/boiler/rrd".into()),
    )?;
    let (start, end) = (1_700_000_000, 1_700_003_600);
    let data = build(&graph, Window::from_points(start, end, 800), None);
    // END README
    let _ = data;

    // BEGIN README
    use kal_rrd_graph::{api, watch::GraphWatch, Graph, GraphSource, Lookup};
    use std::sync::Arc;

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
    let source: Arc<dyn GraphSource> = Arc::new(Single(graph));
    let watch = GraphWatch::new(source.clone())?;
    let router = api::router(api::ApiState { source, watch });
    // Merge router into the application's axum router.
    // END README
    let _ = router;
    Ok(())
}

#[test]
fn readme_rust_blocks_match_compiled_examples() {
    // Typecheck the examples without reading host RRD files or starting watches.
    let _ = compiled_examples as fn() -> Result<(), Box<dyn std::error::Error>>;
    let documented: Vec<_> = include_str!("../README.org")
        .split("#+begin_src rust\n")
        .skip(1)
        .map(|rest| rest.split_once("#+end_src").unwrap().0)
        .collect();
    let compiled: Vec<_> = include_str!("readme.rs")
        .split("\n    // BEGIN README\n")
        .skip(1)
        .map(|rest| {
            rest.split_once("    // END README")
                .unwrap()
                .0
                .lines()
                .map(|line| format!("{}\n", line.strip_prefix("    ").unwrap_or(line)))
                .collect::<String>()
        })
        .collect();
    assert_eq!(documented, compiled);
}
