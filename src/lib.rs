//! One YAML-described RRD graph, independent of application navigation.

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
