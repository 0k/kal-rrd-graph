//! The boundary between graphs and application-owned configuration.

use crate::Graph;

/// A graph lookup preserves the distinction between an unknown id and an
/// application navigation item that cannot be graphed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Lookup {
    /// A validated graph supplied by the host.
    Graph(Box<Graph>),
    /// A known host navigation item that is not a graph (HTTP 400).
    NotAGraph,
    /// An id the host does not recognize (HTTP 404).
    NotFound,
}

/// Supply graph definitions without exposing a navigation tree to the library.
/// A fixed graph needs only `lookup`; changing hosts also implement `revision`.
pub trait GraphSource: Send + Sync + 'static {
    /// Resolve one opaque host-assigned id to a graph or a lookup outcome.
    ///
    /// Implementations own synchronization and must return validated graphs.
    fn lookup(&self, id: &str) -> Lookup;

    /// Monotonically advances after a successful graph configuration change.
    ///
    /// The default zero is suitable for fixed sources. The watcher compares
    /// this revision on each tick to retarget interests after host reloads.
    fn revision(&self) -> u64 {
        0
    }

    /// Resolve one watch operation against a single configuration snapshot.
    /// Mutable hosts should override this to hold their current snapshot once.
    /// Return exactly one result per id, in input order.
    fn lookup_many(&self, ids: &[String]) -> Vec<Lookup> {
        ids.iter().map(|id| self.lookup(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GraphFields;

    struct Single(Graph);

    impl GraphSource for Single {
        fn lookup(&self, id: &str) -> Lookup {
            if id == self.0.id {
                Lookup::Graph(Box::new(self.0.clone()))
            } else {
                Lookup::NotFound
            }
        }
    }

    #[test]
    fn fixed_single_graph_needs_only_lookup() {
        let graph = GraphFields::parse("files: a.rrd\nds: v")
            .unwrap()
            .build("one".into(), "One".into(), "One", Some(".".into()))
            .unwrap();
        let source = Single(graph);
        assert_eq!(source.revision(), 0);
        let results = source.lookup_many(&["missing".into(), "one".into()]);
        assert!(matches!(results[0], Lookup::NotFound));
        let Lookup::Graph(graph) = &results[1] else {
            panic!("the second result must be the supplied graph");
        };
        assert_eq!((&graph.id, &graph.title), (&"one".into(), &"One".into()));
    }
}
