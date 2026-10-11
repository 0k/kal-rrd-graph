//! Graph and watch HTTP API over real RRD files created with librrd in a
//! temp dir.  The host side is a minimal `GraphSource`: no tree, only a map
//! of graph ids plus the ids the host knows as non-graph navigation nodes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use indoc::indoc;
use rrd::ops::{create, update};
use rrd::ConsolidationFn;
use serde_json::Value;
use tower::ServiceExt;

use kal_rrd_graph::api::{router, ApiState};
use kal_rrd_graph::watch::{GraphWatch, WatchTiming};
use kal_rrd_graph::{Graph, GraphFields, GraphSource, Lookup};

/// Aligned on the 60 s step so every update lands exactly on a slot.
const T0: i64 = 1_699_999_980;
const STEP: i64 = 60;

// Parallel native RRD creation can corrupt DS parsing; serialize fixture writes.
static FIXTURE_WRITES: Mutex<()> = Mutex::new(());

/// Create `rel` under `root` with gauge data sources `ds`, and store
/// `values[i]` (one per data source) at `T0 + (i + 1) * STEP`.
fn rrd_file(root: &Path, rel: &str, ds: &[&str], values: &[&[f64]]) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let sources: Vec<_> = ds
        .iter()
        .map(|n| {
            create::DataSource::gauge(&create::DataSourceName::new(*n).unwrap(), 300, None, None)
        })
        .collect();
    let archives: Vec<_> = [
        ConsolidationFn::Avg,
        ConsolidationFn::Min,
        ConsolidationFn::Max,
    ]
    .into_iter()
    .map(|cf| create::Archive::new(cf, 0.5, 1, 1000).unwrap())
    .collect();
    create::create(
        &path,
        UNIX_EPOCH + Duration::from_secs(T0 as u64),
        Duration::from_secs(STEP as u64),
        false,
        None,
        &[],
        &sources,
        &archives,
    )
    .unwrap();
    let batches: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let t = UNIX_EPOCH + Duration::from_secs((T0 + (i as i64 + 1) * STEP) as u64);
            let data: Vec<update::Datum> = row.iter().map(|v| (*v).into()).collect();
            (update::BatchTime::Timestamp(t), data)
        })
        .collect();
    update::update_all(&path, update::Options::default(), &batches).unwrap();
}

/// Application side of the contract: a map of graphs, the ids of its
/// non-graph navigation nodes, and a revision bumped on every change.
#[derive(Default)]
struct Source {
    graphs: Mutex<BTreeMap<String, Graph>>,
    groups: BTreeSet<String>,
    revision: Mutex<u64>,
}

impl Source {
    fn set(&self, graph: Graph) {
        self.graphs.lock().unwrap().insert(graph.id.clone(), graph);
        *self.revision.lock().unwrap() += 1;
    }
}

impl GraphSource for Source {
    fn lookup(&self, id: &str) -> Lookup {
        match self.graphs.lock().unwrap().get(id) {
            Some(graph) => Lookup::Graph(Box::new(graph.clone())),
            None if self.groups.contains(id) => Lookup::NotAGraph,
            None => Lookup::NotFound,
        }
    }

    fn revision(&self) -> u64 {
        *self.revision.lock().unwrap()
    }
}

fn graph(id: &str, dir: &Path, yaml: &str) -> Graph {
    GraphFields::parse(yaml)
        .unwrap()
        .build(id.into(), id.into(), id, Some(dir.to_path_buf()))
        .unwrap()
}

const LOAD: &str = indoc! {"
    files: load_avg.rrd
    series:
      - {ds: l1, label: 1 min}
      - {ds: l5, label: 5 min}
    min-max: true
"};

const MEMORY: &str = indoc! {r#"
    files: containers/*/*/memory.rrd
    label: "{2}"
    ds: used
    type: stacked
    stack-toggle: true
    unit: bytes
    scale: 2
    top: 2
"#};

struct Fixture {
    dir: tempfile::TempDir,
    source: Arc<Source>,
    watch: Arc<GraphWatch>,
}

/// Two hosts with the same two graphs; `host-a` / `host-b` are the
/// application's groups.
fn fixture() -> Fixture {
    let _writes = FIXTURE_WRITES.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("host-a");
    rrd_file(
        &a,
        "load_avg.rrd",
        &["l1", "l5"],
        &[&[1.0, 10.0], &[2.0, 20.0], &[3.0, 30.0]],
    );
    rrd_file(
        &a,
        "containers/myc/odoo/memory.rrd",
        &["used"],
        &[&[100.0], &[100.0], &[100.0]],
    );
    rrd_file(
        &a,
        "containers/myc/db/memory.rrd",
        &["used"],
        &[&[50.0], &[50.0], &[50.0]],
    );
    rrd_file(
        &a,
        "containers/myc/cron/memory.rrd",
        &["used"],
        &[&[1.0], &[2.0], &[3.0]],
    );
    rrd_file(
        &a,
        "containers/myc/broken/memory.rrd",
        &["other"],
        &[&[1.0]],
    );
    std::fs::create_dir_all(dir.path().join("host-b")).unwrap();
    let source = Arc::new(Source {
        groups: ["host-a".to_string(), "host-b".to_string()].into(),
        ..Source::default()
    });
    for host in ["host-a", "host-b"] {
        let root = dir.path().join(host);
        source.set(graph(&format!("{host}/load"), &root, LOAD));
        source.set(graph(&format!("{host}/memory"), &root, MEMORY));
    }
    let watch = GraphWatch::with_timing(
        source.clone(),
        WatchTiming {
            settle: Duration::from_millis(100),
            grace: Duration::from_millis(200),
            tick: Duration::from_millis(20),
        },
    )
    .unwrap();
    Fixture { dir, source, watch }
}

fn app(f: &Fixture) -> Router {
    router(ApiState {
        source: f.source.clone(),
        watch: f.watch.clone(),
    })
}

async fn request(f: &Fixture, uri: &str) -> axum::response::Response {
    app(f)
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn get(f: &Fixture, uri: &str) -> (StatusCode, Value) {
    let resp = request(f, uri).await;
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn graph_uri(id: &str) -> String {
    format!(
        "/api/graph?id={id}&start={T0}&end={}&points=10",
        T0 + 3 * STEP
    )
}

async fn first_event(body: &mut Body) -> Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut text = String::new();
        loop {
            let frame = body.frame().await.unwrap().unwrap();
            if let Ok(bytes) = frame.into_data() {
                text.push_str(std::str::from_utf8(&bytes).unwrap());
                if let Some(line) = text.lines().find_map(|line| line.strip_prefix("data: ")) {
                    return serde_json::from_str(line).unwrap();
                }
            }
        }
    })
    .await
    .expect("SSE event timed out")
}

#[tokio::test]
async fn graph_returns_each_data_source_on_the_time_grid() {
    let f = fixture();
    let (status, g) = get(&f, &graph_uri("host-a/load")).await;
    assert_eq!(status, StatusCode::OK, "{g}");
    assert_eq!(
        (g["start"].as_i64(), g["step"].as_i64(), g["len"].as_u64()),
        (Some(T0 + STEP), Some(STEP), Some(3))
    );
    assert_eq!(g["series"][0]["label"], "1 min");
    assert_eq!(g["series"][0]["avg"], serde_json::json!([1.0, 2.0, 3.0]));
    assert_eq!(g["series"][1]["avg"], serde_json::json!([10.0, 20.0, 30.0]));
    assert_eq!(g["series"][0]["min"], serde_json::json!([1.0, 2.0, 3.0]));
    assert_eq!(g["last_update"].as_i64(), Some(T0 + 3 * STEP));
}

#[tokio::test]
async fn glob_graph_labels_scales_keeps_top_and_reports_missing_ds() {
    let f = fixture();
    let (_, g) = get(&f, &graph_uri("host-a/memory")).await;
    let labels: Vec<_> = g["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["odoo", "db", "other (1)"]);
    assert_eq!(
        g["series"][0]["avg"],
        serde_json::json!([200.0, 200.0, 200.0])
    );
    assert_eq!(g["series"][2]["avg"], serde_json::json!([2.0, 4.0, 6.0]));
    let notes = g["notes"].to_string();
    assert!(
        notes.contains("containers/myc/broken/memory.rrd: no data source `used`"),
        "{notes}"
    );
}

#[tokio::test]
async fn host_without_files_gets_an_explanation_not_an_error() {
    let f = fixture();
    let (status, g) = get(&f, &graph_uri("host-b/memory")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(g["series"], serde_json::json!([]));
    assert!(
        g["notes"][0]
            .as_str()
            .unwrap()
            .starts_with("no file matches containers/*/*/memory.rrd"),
        "{g}"
    );
}

#[tokio::test]
async fn top_tail_keeps_full_window_winners_and_values_at_shared_instants() {
    let f = fixture();
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        for (name, values) in [
            ("odoo", [100.0, 100.0, 1.0]),
            ("db", [50.0, 50.0, 2.0]),
            ("cron", [1.0, 1.0, 90.0]),
        ] {
            rrd_file(
                f.dir.path(),
                &format!("host-a/containers/myc/{name}/memory.rrd"),
                &["used"],
                &[&[values[0]], &[values[1]], &[values[2]]],
            );
        }
    }
    let (status, full) = get(&f, &graph_uri("host-a/memory")).await;
    assert_eq!(status, StatusCode::OK);
    let labels = |g: &Value| -> Vec<String> {
        g["series"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["label"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(labels(&full), ["odoo", "db", "other (1)"]);
    let start = T0 + 2 * STEP;
    let end = T0 + 3 * STEP;
    let step = full["step"].as_i64().unwrap();
    let uri = format!("/api/graph?id=host-a/memory&start={start}&end={end}&step={step}");
    let (_, ranked_tail) = get(&f, &uri).await;
    assert_eq!(labels(&ranked_tail), ["cron", "db", "other (1)"]);
    let (status, tail) = get(&f, &format!("{uri}&keep=odoo&keep=db")).await;
    assert_eq!(status, StatusCode::OK);
    let mut actual_labels = labels(&tail);
    let mut expected_labels = labels(&full);
    actual_labels.sort();
    expected_labels.sort();
    assert_eq!(actual_labels, expected_labels);
    assert_eq!(tail["step"], full["step"]);
    assert_eq!(tail["last_update"], full["last_update"]);
    let offset =
        ((tail["start"].as_i64().unwrap() - full["start"].as_i64().unwrap()) / step) as usize;
    for series in tail["series"].as_array().unwrap() {
        let previous = full["series"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["label"] == series["label"])
            .unwrap();
        assert_eq!(
            series["avg"],
            serde_json::json!(&previous["avg"].as_array().unwrap()[offset..])
        );
    }
    let load_uri = graph_uri("host-a/load");
    assert_eq!(
        get(&f, &format!("{load_uri}&keep=1%20min&keep=unknown")).await,
        get(&f, &load_uri).await
    );
}

#[tokio::test]
async fn explicit_step_tail_and_hold_match_full_real_rrd_values() {
    let f = fixture();
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        rrd_file(
            f.dir.path(),
            "host-a/gap.rrd",
            &["v"],
            &[&[5.0], &[5.0], &[f64::NAN], &[f64::NAN], &[7.0], &[7.0]],
        );
    }
    f.source.set(graph(
        "gap",
        &f.dir.path().join("host-a"),
        "files: gap.rrd\nds: v\nmin-max: true\nhold: 180\n",
    ));
    let end = T0 + 6 * STEP;
    let (status, full) = get(
        &f,
        &format!("/api/graph?id=gap&start={T0}&end={end}&points=10"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let step = full["step"].as_i64().unwrap();
    let last = full["start"].as_i64().unwrap() + (full["len"].as_i64().unwrap() - 1) * step;
    for start in [last - 2 * step, T0 + 2 * STEP] {
        let (status, tail) = get(
            &f,
            &format!("/api/graph?id=gap&start={start}&end={end}&step={step}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(tail["step"], full["step"]);
        let offset_secs = tail["start"].as_i64().unwrap() - full["start"].as_i64().unwrap();
        assert_eq!(offset_secs % step, 0);
        let offset = (offset_secs / step) as usize;
        for key in ["avg", "min", "max"] {
            assert_eq!(
                tail["series"][0][key],
                serde_json::json!(&full["series"][0][key].as_array().unwrap()[offset..])
            );
        }
        if start == T0 + 2 * STEP {
            assert_eq!(tail["series"][0]["avg"][0], 5.0);
        }
    }
}

#[tokio::test]
async fn unstyled_graph_json_has_exactly_the_original_fields() {
    let f = fixture();
    let (_, g) = get(&f, &graph_uri("host-a/load")).await;
    let keys = |v: &Value| -> Vec<String> { v.as_object().unwrap().keys().cloned().collect() };
    let mut top = keys(&g);
    top.sort();
    assert_eq!(
        top,
        [
            "id",
            "last_update",
            "len",
            "notes",
            "series",
            "start",
            "step",
            "title",
            "type",
            "unit"
        ]
    );
    for series in g["series"].as_array().unwrap() {
        let mut fields = keys(series);
        fields.sort();
        assert_eq!(fields, ["avg", "label", "max", "min"]);
    }
}

#[tokio::test]
async fn styled_graph_carries_drawing_keys_and_computed_band_and_diff() {
    let f = fixture();
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        // current, sel per slot: below, equal, above, unknown current.
        rrd_file(
            f.dir.path(),
            "boiler/zones/1/temperature.rrd",
            &["current", "sel"],
            &[
                &[18.0, 20.0],
                &[19.0, 19.0],
                &[21.0, 20.0],
                &[f64::NAN, 20.0],
            ],
        );
    }
    f.source.set(graph(
        "boiler",
        &f.dir.path().join("boiler"),
        indoc! {r##"
            files: zones/*/temperature.rrd
            label: "zone {1}"
            scale: 2
            y-axis: {min: 0, interval: 1, minor-interval: 0.5}
            series:
              - {ds: sel, label: selected, color: red, dash: dotted, width: 3}
              - {ds: current, label: current, color: "#000", fill: 0.2}
              - label: chauffe
                band: {lower: current, upper: sel, pattern: hatch, opacity: 0.5}
                color: red
              - label: heating
                diff: [sel, current]
                floor: 0
              - label: missing
                diff: [sel, nope]
        "##},
    ));
    let (status, g) = get(
        &f,
        &format!(
            "/api/graph?id=boiler&start={T0}&end={}&points=10",
            T0 + 4 * STEP
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{g}");
    assert_eq!(
        g["y_axis"],
        serde_json::json!({"min": 0.0, "interval": 1.0, "minor_interval": 0.5})
    );
    assert_eq!(
        g["series"],
        serde_json::json!([
            {"label": "zone 1 selected", "avg": [40.0, 38.0, 40.0, 40.0],
             "color": "red", "dash": "dotted", "width": 3.0},
            {"label": "zone 1 current", "avg": [36.0, 38.0, 42.0, null],
             "color": "#000", "fill": 0.2},
            {"label": "zone 1 chauffe", "avg": [40.0, 38.0, null, null],
             "min": [36.0, 38.0, null, null], "max": [40.0, 38.0, null, null],
             "color": "red", "band": {"pattern": "hatch", "opacity": 0.5}, "line": false},
            {"label": "zone 1 heating", "avg": [4.0, 0.0, 0.0, null]},
        ])
    );
    assert_eq!(
        g["notes"],
        serde_json::json!(["zones/1/temperature.rrd: no data source `nope`"])
    );
}

#[tokio::test]
async fn bad_requests_are_rejected_with_a_reason() {
    let f = fixture();
    let (s, e) = get(&f, "/api/graph?id=nope&start=0&end=10").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "{e}");
    let (s, e) = get(&f, "/api/graph?id=host-a&start=0&end=10").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(e["error"].as_str().unwrap().contains("is a group"));
    let (s, _) = get(&f, "/api/graph?id=host-a/load&start=10&end=10").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = get(&f, "/api/graph?id=host-a/load&start=0&end=10&points=1").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

async fn watch_snapshot(f: &Fixture, query: &str) -> String {
    let response = request(f, &format!("/api/watch?{query}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["x-snapshot-index"]
        .to_str()
        .unwrap()
        .parse::<u64>()
        .is_ok());
    response.headers()["x-sub-id"].to_str().unwrap().to_string()
}

async fn watch_stream(f: &Fixture, sub: &str) -> Body {
    let response = request(f, &format!("/api/watch/events?sub={sub}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let mut body = response.into_body();
    let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_data().unwrap().as_ref(), b": connected\n\n");
    body
}

async fn no_event(body: &mut Body, duration: Duration) {
    assert!(
        tokio::time::timeout(duration, body.frame()).await.is_err(),
        "unexpected SSE frame"
    );
}

fn update_file(f: &Fixture, rel: &str, slot: i64, values: &[f64]) {
    let _writes = FIXTURE_WRITES.lock().unwrap();
    let data: Vec<update::Datum> = values.iter().map(|v| (*v).into()).collect();
    update::update_all(
        &f.dir.path().join(rel),
        update::Options::default(),
        &[(
            update::BatchTime::Timestamp(
                UNIX_EPOCH + Duration::from_secs((T0 + slot * STEP) as u64),
            ),
            data,
        )],
    )
    .unwrap();
}

#[tokio::test]
async fn watch_snapshot_and_step_validation_have_json_errors() {
    let f = fixture();
    for (uri, status, message) in [
        (
            "/api/watch?id=nope",
            StatusCode::NOT_FOUND,
            "no graph \"nope\"",
        ),
        (
            "/api/watch?id=host-a",
            StatusCode::BAD_REQUEST,
            "\"host-a\" is a group, not a graph",
        ),
        (
            "/api/watch?id=host-a/load&unknown=x",
            StatusCode::BAD_REQUEST,
            "unknown query parameter \"unknown\"",
        ),
        (
            "/api/watch?sub=not-a-uuid",
            StatusCode::BAD_REQUEST,
            "invalid subscription id",
        ),
        (
            "/api/watch/events",
            StatusCode::BAD_REQUEST,
            "missing subscription id",
        ),
        (
            "/api/watch/events?sub=%0A",
            StatusCode::BAD_REQUEST,
            "invalid subscription id",
        ),
        (
            "/api/graph?id=host-a/load&start=0&end=10&points=10&step=1",
            StatusCode::BAD_REQUEST,
            "give either `points` or `step`, not both",
        ),
        (
            "/api/graph?id=host-a/load&start=0&end=10&step=0",
            StatusCode::BAD_REQUEST,
            "`step` must be at least 1",
        ),
        (
            "/api/graph?id=host-a/load&start=0&end=10002&step=2",
            StatusCode::BAD_REQUEST,
            "`step` requests more than 5000 points",
        ),
    ] {
        assert_eq!(
            get(&f, uri).await,
            (status, serde_json::json!({"error": message})),
            "{uri}"
        );
    }
    assert_eq!(
        get(&f, "/api/watch").await.1,
        serde_json::json!({"ids": []})
    );
    assert_eq!(
        get(
            &f,
            "/api/watch?id=host-b/load&id=host-a/load&id=host-a/load"
        )
        .await
        .1,
        serde_json::json!({"ids": ["host-a/load", "host-b/load"]})
    );
    let (status, body) = get(
        &f,
        "/api/watch/events?sub=00000000-0000-4000-8000-000000000000",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        body,
        serde_json::json!({"error": "needs_snapshot", "sub": "00000000-0000-4000-8000-000000000000", "hint": "resync via GET /api/watch"})
    );
}

#[tokio::test]
async fn mmap_updates_notify_only_changed_graph_and_expire_disconnected_interest() {
    let f = fixture();
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        rrd_file(
            f.dir.path(),
            "host-b/load_avg.rrd",
            &["l1", "l5"],
            &[&[1.0, 10.0]],
        );
    }
    let sub = watch_snapshot(&f, "id=host-a/load&id=host-b/load").await;
    let mut stream = watch_stream(&f, &sub).await;
    update_file(&f, "host-a/load_avg.rrd", 4, &[4.0, 40.0]);
    assert_eq!(
        first_event(&mut stream).await,
        serde_json::json!({"id": "host-a/load"})
    );
    // Our own librrd fetch/info reads must not feed back into notifications.
    get(&f, &graph_uri("host-a/load")).await;
    no_event(&mut stream, Duration::from_millis(140)).await;
    let b_sub = watch_snapshot(&f, "id=host-b/load").await;
    let mut b_stream = watch_stream(&f, &b_sub).await;
    drop(stream);
    no_event(&mut b_stream, Duration::from_millis(250)).await;
    update_file(&f, "host-a/load_avg.rrd", 5, &[5.0, 50.0]);
    no_event(&mut b_stream, Duration::from_millis(140)).await;
    update_file(&f, "host-b/load_avg.rrd", 2, &[2.0, 20.0]);
    assert_eq!(
        first_event(&mut b_stream).await,
        serde_json::json!({"id": "host-b/load"})
    );
    no_event(&mut b_stream, Duration::from_millis(140)).await;
}

#[tokio::test]
async fn new_glob_directory_and_burst_updates_produce_one_event_each() {
    let f = fixture();
    let sub = watch_snapshot(&f, "id=host-a/memory").await;
    let mut stream = watch_stream(&f, &sub).await;
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        rrd_file(
            f.dir.path(),
            "host-a/containers/new/app/memory.rrd",
            &["used"],
            &[&[8.0]],
        );
    }
    assert_eq!(
        first_event(&mut stream).await,
        serde_json::json!({"id": "host-a/memory"})
    );
    no_event(&mut stream, Duration::from_millis(140)).await;
    update_file(&f, "host-a/containers/myc/odoo/memory.rrd", 4, &[101.0]);
    update_file(&f, "host-a/containers/myc/db/memory.rrd", 4, &[51.0]);
    update_file(&f, "host-a/containers/new/app/memory.rrd", 2, &[9.0]);
    assert_eq!(
        first_event(&mut stream).await,
        serde_json::json!({"id": "host-a/memory"})
    );
    no_event(&mut stream, Duration::from_millis(140)).await;
    // Hidden rsync temporaries are ignored, but renaming one into place is a change.
    let root = f.dir.path().join("host-a/containers/new/app");
    std::fs::write(root.join(".memory.rrd.tmp"), b"temporary").unwrap();
    no_event(&mut stream, Duration::from_millis(140)).await;
    std::fs::rename(root.join("memory.rrd"), root.join(".saved")).unwrap();
    std::fs::rename(root.join(".saved"), root.join("memory.rrd")).unwrap();
    assert_eq!(
        first_event(&mut stream).await,
        serde_json::json!({"id": "host-a/memory"})
    );
}

#[tokio::test]
async fn revision_change_retargets_watches_to_the_new_definition() {
    let f = fixture();
    {
        let _writes = FIXTURE_WRITES.lock().unwrap();
        rrd_file(
            f.dir.path(),
            "host-b/load_avg.rrd",
            &["l1", "l5"],
            &[&[1.0, 10.0]],
        );
    }
    let sub = watch_snapshot(&f, "id=host-b/load").await;
    let mut stream = watch_stream(&f, &sub).await;
    f.source
        .set(graph("host-b/load", &f.dir.path().join("host-a"), LOAD));
    no_event(&mut stream, Duration::from_millis(60)).await;
    update_file(&f, "host-a/load_avg.rrd", 4, &[4.0, 40.0]);
    assert_eq!(
        first_event(&mut stream).await,
        serde_json::json!({"id": "host-b/load"})
    );
    update_file(&f, "host-b/load_avg.rrd", 2, &[2.0, 20.0]);
    no_event(&mut stream, Duration::from_millis(140)).await;
}
