# Architecture and acquisition guidance

## Binding architecture

`junctions-rs` follows the **Rust domain-core / thin-adapter** pattern:

1. `junctions-core` owns the versioned `Road`, `Config`, and `Junction` contract plus deterministic planar topology.
2. Adapters own host-language conversion and never reimplement the algorithm:
   - `junctions` CLI: GeoJSON files;
   - PyO3 + maturin: Python native extension and wheels;
   - extendr: R package native registration.
3. Geographic CRS transforms, OSM tag normalisation, and download policy live *outside* the core. Inputs must use a local projected CRS in metres. This keeps the core reproducible, testable, and usable with non-OSM road sources.
4. Future adapters should consume the same serialisable contract. GeoArrow/Arrow is the preferred high-throughput interchange once vectorised arrays are introduced; UniFFI/WASM are candidates for other languages and browsers.

## Geometry and identifiers

The core creates a round planar buffer for every accepted candidate node
(road endpoint or interior crossing) at its road's radius, dissolves all
buffers on one level with a batched unary union, and takes a convex hull for
each connected component. A component is one junction system: nearby nodes
whose circular buffers overlap merge into a single junction polygon. This
matches the `ST_Buffer` → `ST_Union_Agg` → `ST_Dump` → `ST_ConvexHull` footprint
sequence used by DuckDB `junctions_cluster`, while remaining pure Rust and
WASM-compatible. Output is always represented as GeoJSON-compatible
MultiPolygon coordinates so disconnected components are never falsely bridged.

Geometric overlap contract: Candidate circular buffers merge into the same
component strictly when they touch or overlap. Disjoint buffer components do
not merge into the same junction, preserving deterministic node membership and
arm counts. When each component's convex hull is emitted as the junction polygon,
the hulls of nearby disjoint components can geometrically overlap in planar space
(an inherent property of convex hulls of non-convex/multi-node clusters), but they
remain separate topological junctions with unique IDs, centroids, and stats.

Buffer radii: a road may carry its own `buffer_m` (e.g. road-class radii from
OS Open Roads); a node buffers at the minimum radius of its incident roads,
and candidate positions are snapped with `cluster_distance_m` (1 cm) before
buffering. Only nodes with at least `min_arms` incident road ends are
buffered, so dead ends never inflate a neighbouring junction.

When aligned `Road.node_ids` are available, a junction ID is a length-prefixed,
collision-free canonical combination of its level plus sorted, deduplicated
merged node IDs and contributing way IDs. The canonical `node_ids` and
`way_ids` are also output for direct auditability. Interior-only crossings
without a source node add sorted exact candidate-coordinate bit patterns to the
identity as a deterministic fallback.

This is inspired by the GeoRust/GeoArrow ecosystem: native algorithms separated from Python/WASM adapters and Arrow-shaped interoperability. PyO3/maturin is the standard Python wheel route; extendr provides the R-native route.

## Why not use a C ABI as the primary public API?

A C ABI is valuable for mature, stable scalar/array APIs, but serialising geometries to JSON initially makes the language contract explicit and easier to test. A future `junctions-ffi` crate can expose Arrow C Data Interface buffers without changing `junctions-core`.

## OSM road acquisition: decision guide

| Situation | Preferred path | Why | Constraint |
|---|---|---|---|
| Repeatable regional/national analysis, offline work, many study areas | Geofabrik `.osm.pbf` + libosmium/GDAL/osmextract | one cacheable binary download; local, reproducible filtering; extract metadata/date can be pinned | smallest provider extract may be far bigger than AOI |
| Ad-hoc small AOI, freshest OSM tags/geometry | Overpass API (`osmdata`) | server-side tag+bbox predicate sends only requested ways | public endpoint capacity/timeout is variable; cache outputs and obey policy |
| Global comparative basemaps / broad road coverage | Overture GeoParquet | cloud columnar data, bbox predicate pushdown, no whole-world download | not a verbatim OSM extract; schema/provenance/coverage can differ; needs cloud scan |

### OSM package precedents

- **osmextract** downloads a provider `.pbf`, translates to GeoPackage, then reads it. Its cache avoids repeated download/translation.
- **osmdata** builds an Overpass query (`opq()` + `add_osm_feature()`) and is designed for small-to-medium custom extracts.
- **Overture** publishes cloud GeoParquet. DuckDB with `httpfs`/`spatial` or the Overture client reads only bbox-selected data.

## Sources

- [osmextract `oe_get`](https://docs.ropensci.org/osmextract/reference/oe_get.html)
- [osmdata](https://docs.ropensci.org/osmdata/)
- [Overture DuckDB guide](https://docs.overturemaps.org/getting-data/duckdb/)
- [PyO3 guide](https://pyo3.rs/main/), [maturin guide](https://www.maturin.rs/)
- [GeoArrow Rust layout](https://github.com/geoarrow/geoarrow-rs)
- [extendr](https://extendr.github.io/extendr/)
