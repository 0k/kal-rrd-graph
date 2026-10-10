# kal-rrd-graph

Reusable description, fetching, file watching and HTTP API for one RRD graph.
The host application owns navigation, groups, graph identifiers and config reloads.

## Blocking rules

- YAML is the only graph description. No hard-coded paths, data sources or units.
- RRD files are read-only outside tests. Read exclusively through `rrd`/librrd,
  never an `rrdtool` subprocess or a custom file parser.
- File notifications use `kal-fs-watch`; SSE uses `kal-sse`.
- Graph lookup is supplied by `GraphSource`; never add a group/tree model here.
- Preserve graph YAML defaults, validation paths, API JSON/status/query contracts,
  common grids, tail/keep/hold semantics, debounce and subscription lifetimes.
- Manifests use published dependencies. Local sibling overrides belong only in
  git-ignored `.cargo/config.toml`.

## Project map

- `src/config.rs`: graph YAML fields, strict validation, labels and durations.
- `src/pattern.rs`: relative file patterns and numbered captures.
- `src/graph.rs`: librrd fetch, common grid, scale, top/keep and hold.
- `src/source.rs`: application-supplied lookup and revision contract.
- `src/watch.rs`: displayed-graph interests, file watches and event log.
- `src/api.rs`: axum graph/watch endpoints; no tree or config endpoints.
- `tests/`: real librrd and notification/SSE contracts.

## Verification

Requires librrd development headers. Run `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`.
Use temporary RRD fixtures only; never update production data in tests.
