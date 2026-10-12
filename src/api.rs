//! Graph JSON API and kal-sse graph-change channel.
//!
//! * `GET api/graph?id=…&start=…&end=…&points=…` (or `step=…`) — series
//!   over a window (UNIX seconds), see [`crate::graph::GraphData`].
//! * `GET api/watch?id=…&id=…[&sub=…]` — declare displayed graphs;
//!   returns `{"ids":[…]}`, `x-sub-id` and `x-snapshot-index`.
//! * `GET api/watch/events?sub=…` — indexed `{"id":"graph-id"}` changes.

use std::collections::BTreeSet;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::{Body, BodyDataStream, Bytes};
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_core::Stream;
use serde::Deserialize;

use crate::graph::{self, Window};
use crate::source::{GraphSource, Lookup};
use crate::watch::{valid_sub, GraphWatch, StreamGuard, WatchError};

/// Bounds on `points`, so a request cannot ask for a useless or huge
/// response.
pub const MIN_POINTS: usize = 10;
pub const MAX_POINTS: usize = 5000;

#[derive(Clone)]
pub struct ApiState {
    pub source: Arc<dyn GraphSource>,
    pub watch: Arc<GraphWatch>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("watch", &self.watch)
            .finish_non_exhaustive()
    }
}

pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/graph", get(graph_data))
        .route("/api/watch", get(watch_snapshot))
        .route("/api/watch/events", get(watch_events))
        .with_state(state)
}

async fn watch_snapshot(
    State(state): State<ApiState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Response {
    let mut ids = Vec::new();
    let mut sub = None;
    for (key, value) in params {
        match key.as_str() {
            "id" => ids.push(value),
            "sub" if sub.is_none() => sub = Some(value),
            "sub" => return error(StatusCode::BAD_REQUEST, "duplicate subscription id"),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    &format!("unknown query parameter {key:?}"),
                )
            }
        }
    }
    // `subscribe` waits for the worker to arm the watches (directory walk):
    // keep that blocking wait off the async executor.
    let watch = state.watch.clone();
    let subscribed = match tokio::task::spawn_blocking(move || watch.subscribe(sub, ids)).await {
        Ok(result) => result,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("watch task failed: {e}"),
            )
        }
    };
    match subscribed {
        Ok((snapshot, sub, index)) => {
            let mut response = Json(snapshot).into_response();
            response.headers_mut().insert(
                "x-sub-id",
                HeaderValue::from_str(&sub).expect("validated subscription header"),
            );
            response.headers_mut().insert(
                kal_sse::axum::SNAPSHOT_INDEX_HEADER,
                HeaderValue::from_str(&index.to_string()).expect("numeric index header"),
            );
            response
        }
        Err(error_value) => {
            let status = match &error_value {
                WatchError::NotFound { .. } => StatusCode::NOT_FOUND,
                WatchError::InvalidSubscription | WatchError::NotAGraph { .. } => {
                    StatusCode::BAD_REQUEST
                }
                WatchError::NonzeroTiming
                | WatchError::Initialize(_)
                | WatchError::Spawn(_)
                | WatchError::WorkerStopped(_)
                | WatchError::Rescan(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            error(status, &error_value.to_string())
        }
    }
}

async fn watch_events(
    State(state): State<ApiState>,
    Query(params): Query<Vec<(String, String)>>,
) -> Response {
    let mut sub = None;
    for (key, value) in params {
        match key.as_str() {
            "sub" if sub.is_none() => sub = Some(value),
            "sub" => return error(StatusCode::BAD_REQUEST, "duplicate subscription id"),
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    &format!("unknown query parameter {key:?}"),
                )
            }
        }
    }
    let Some(sub) = sub else {
        return error(StatusCode::BAD_REQUEST, "missing subscription id");
    };
    if !valid_sub(&sub) {
        return error(StatusCode::BAD_REQUEST, "invalid subscription id");
    }
    match kal_sse::axum::sse_response(
        state.watch.log(),
        sub.clone(),
        |change| serde_json::to_string(change).expect("serialize graph change"),
        "resync via GET /api/watch",
    ) {
        Ok(stream) => {
            let Some(guard) = state.watch.attach(&sub) else {
                return (StatusCode::CONFLICT, Json(serde_json::json!({"error": "needs_snapshot", "sub": sub, "hint": "resync via GET /api/watch"}))).into_response();
            };
            let (parts, body) = stream.into_response().into_parts();
            Response::from_parts(
                parts,
                Body::from_stream(InterestStream {
                    inner: body.into_data_stream(),
                    guard: Some(guard),
                }),
            )
        }
        Err(error) => error.into_response(),
    }
}

// Retain the interest for exactly the body stream's lifetime (including
// cancellation before its first poll). kal-sse still owns all SSE semantics.
struct InterestStream {
    inner: BodyDataStream,
    guard: Option<StreamGuard>,
}

impl Stream for InterestStream {
    type Item = Result<Bytes, axum::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let result = Pin::new(&mut self.inner).poll_next(cx);
        if matches!(result, Poll::Ready(None)) {
            self.guard.take();
        }
        result
    }
}

#[derive(Debug, Deserialize)]
struct GraphQuery {
    id: String,
    start: i64,
    end: i64,
    points: Option<usize>,
    step: Option<i64>,
}

/// Default number of points when the client does not say.
pub const DEFAULT_POINTS: usize = 800;

async fn graph_data(
    State(state): State<ApiState>,
    Query(q): Query<GraphQuery>,
    Query(params): Query<Vec<(String, String)>>,
) -> Response {
    // Scalar fields use serde's numeric validation; pairs retain repeated labels.
    let keep: BTreeSet<_> = params
        .into_iter()
        .filter_map(|(key, value)| (key == "keep").then_some(value))
        .collect();
    let keep = (!keep.is_empty()).then_some(keep);
    if q.end <= q.start {
        return error(StatusCode::BAD_REQUEST, "`end` must be after `start`");
    }
    if !Window::from_points(q.start, q.end, DEFAULT_POINTS).in_range(None) {
        return error(StatusCode::BAD_REQUEST, "`start` or `end` is out of range");
    }
    if q.points.is_some() && q.step.is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "give either `points` or `step`, not both",
        );
    }
    let window = if let Some(step) = q.step {
        if step < 1 {
            return error(StatusCode::BAD_REQUEST, "`step` must be at least 1");
        }
        if (i128::from(q.end) - i128::from(q.start)) / i128::from(step) > MAX_POINTS as i128 {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("`step` requests more than {MAX_POINTS} points"),
            );
        }
        Window::from_step(q.start, q.end, step)
    } else {
        let points = q.points.unwrap_or(DEFAULT_POINTS);
        if !(MIN_POINTS..=MAX_POINTS).contains(&points) {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("`points` must be between {MIN_POINTS} and {MAX_POINTS}"),
            );
        }
        Window::from_points(q.start, q.end, points)
    };
    let graph = match state.source.lookup(&q.id) {
        Lookup::Graph(g) => *g,
        Lookup::NotAGraph => {
            return error(
                StatusCode::BAD_REQUEST,
                &format!("{:?} is a group, not a graph", q.id),
            )
        }
        Lookup::NotFound => return error(StatusCode::NOT_FOUND, &format!("no graph {:?}", q.id)),
    };
    if !window.in_range(graph.hold) {
        return error(StatusCode::BAD_REQUEST, "`start` or `end` is out of range");
    }
    match tokio::task::spawn_blocking(move || graph::build(&graph, window, keep.as_ref())).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("graph task failed: {e}"),
        ),
    }
}

fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}
