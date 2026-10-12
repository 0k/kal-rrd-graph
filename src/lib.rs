//! One YAML-described RRD graph, independent of application navigation.
//!
//! Parse graph keys with [`GraphFields`], validate them into a [`Graph`], and
//! fetch its series with [`graph::build`]. Reads go through `rrd`/librrd;
//! this library never updates RRD files. Missing files, data sources and band
//! archives are reported in [`graph::GraphData::notes`].
//!
//! # System requirements
//!
//! Building requires librrd **1.5 or newer**, its development headers, and
//! `pkg-config` to locate it. On Debian/Ubuntu install `librrd-dev` and
//! `pkg-config`; the librrd shared library is also required at runtime.
//!
//! # Features
//!
//! - `watch` (default): file interests and change tracking through
//!   `kal-fs-watch` and the `kal-sse` event log.
//! - `api` (default, implies `watch`): axum graph JSON and SSE endpoints.
//! - Disable default features for YAML validation and graph fetching only.
//!   Enable just `watch` for file interests without the HTTP adapter.
//!
//! # Host responsibilities
//!
//! The host assigns ids and titles, resolves the RRD directory, and owns
//! navigation, groups and configuration reloads. [`GraphSource`] supplies
//! validated definitions to the watcher and HTTP adapter. A mutable source
//! advances [`GraphSource::revision`] after successful changes and overrides
//! [`GraphSource::lookup_many`] to resolve a batch from one snapshot. Failed
//! reloads should retain the previous definitions and revision.
//!
//! # Parse and fetch
//!
//! ```no_run
//! use kal_rrd_graph::{GraphFields, graph::{build, Window}};
//!
//! let fields = GraphFields::parse("files: temperature.rrd\nds: value\nunit: degrees")?;
//! let graph = fields.build(
//!     "boiler".into(), "Boiler".into(), "Boiler",
//!     Some("/var/lib/boiler/rrd".into()),
//! )?;
//! let (start, end) = (1_700_000_000, 1_700_003_600);
//! let data = build(&graph, Window::from_points(start, end, 800), None);
//! assert_eq!(data.id, "boiler");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # HTTP and file watching
//!
//! With `api` enabled, share the same source between the watcher and router.
//! The router exposes `/api/graph`, `/api/watch` and `/api/watch/events`;
//! the host merges or nests it into its own axum application and runs the
//! HTTP server inside a Tokio runtime.
//!
//! ```no_run
//! # #[cfg(feature = "api")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::sync::Arc;
//! use kal_rrd_graph::{Graph, GraphFields, GraphSource, Lookup, api, watch::GraphWatch};
//!
//! struct Single(Graph);
//! impl GraphSource for Single {
//!     fn lookup(&self, id: &str) -> Lookup {
//!         if id == self.0.id() {
//!             Lookup::Graph(Box::new(self.0.clone()))
//!         } else {
//!             Lookup::NotFound
//!         }
//!     }
//! }
//! let graph = GraphFields::parse("files: temperature.rrd\nds: value")?
//!     .build("boiler".into(), "Boiler".into(), "Boiler",
//!         Some("/var/lib/boiler/rrd".into()))?;
//! let source: Arc<dyn GraphSource> = Arc::new(Single(graph));
//! let watch = GraphWatch::new(source.clone())?;
//! let router = api::router(api::ApiState { source, watch });
//! // Merge `router` into the host's axum application.
//! # let _ = router;
//! # Ok(())
//! # }
//! # #[cfg(not(feature = "api"))]
//! # fn main() {}
//! ```

#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "api")]
#[cfg_attr(docsrs, doc(cfg(feature = "api")))]
pub mod api;
pub mod config;
pub mod graph;
pub mod pattern;
pub mod source;
#[cfg(feature = "watch")]
#[cfg_attr(docsrs, doc(cfg(feature = "watch")))]
pub mod watch;

pub use config::{
    BandDef, BandPattern, BandStyle, Dash, Graph, GraphFields, GraphType, Series, SeriesDef,
    SeriesKind, SeriesStyle, YAxis,
};
pub use source::{GraphSource, Lookup};
