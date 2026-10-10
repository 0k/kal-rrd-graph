//! One YAML-described RRD graph, independent of application navigation.

pub mod api;
pub mod config;
pub mod graph;
pub mod pattern;
pub mod source;
pub mod watch;

pub use config::{Graph, GraphFields, GraphType, SeriesDef};
pub use source::{GraphSource, Lookup};
