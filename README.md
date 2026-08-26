# junctions-rs

Fast, portable detection of road-centreline junctions. `junctions-rs` uses a Rust topology core with a CLI and thin Python/R adapters, so the same deterministic semantics are available without binding the algorithm to a database or one host language.

> **Status: 0.1.0 prototype.** The core detects same-level endpoint and interior-crossing connections in **projected metre coordinates** and emits a deterministic convex-hull polygon per cluster. It is not yet geometry-equivalent to the production DuckDB `junctions` extension: that extension has OSM CRS handling, tag policy, road-class buffer rules, and GEOS dissolve semantics. See [`docs/architecture.md`](docs/architecture.md).

## Installation

### CLI / Rust

```sh
cargo install --path crates/junctions-cli
junctions projected-roads.geojson --output junctions.geojson --min-arms 3
```

Input must be a GeoJSON `FeatureCollection` of projected `LineString` roads in metres. Optional `id`/`feature_id` and integer `level` properties are read. A different level prevents bridge/tunnel crossings from connecting.

### Python

```sh
uv pip install maturin
maturin develop --release
```

```python
import json
import junctions_rs

roads = [
    {"id": "west", "coordinates": [[-10, 0], [0, 0]], "level": 0},
    {"id": "east", "coordinates": [[0, 0], [10, 0]], "level": 0},
    {"id": "north", "coordinates": [[0, 0], [0, 10]], "level": 0},
]
print(json.loads(junctions_rs.find_junctions_json(json.dumps(roads))))
```

### R (experimental)

The R package is in [`r/junctionsrs`](r/junctionsrs) and uses **extendr**. Once `cargo extendr` is installed, generate glue and build it through `rextendr`:

```r
rextendr::document("r/junctionsrs")
pak::pkg_install("r/junctionsrs")
```

```r
junctionsrs::junctions(list(
  list(id = "west", coordinates = list(c(-10, 0), c(0, 0)), level = 0L),
  list(id = "east", coordinates = list(c(0, 0), c(10, 0)), level = 0L),
  list(id = "north", coordinates = list(c(0, 0), c(0, 10)), level = 0L)
))
```

## Browser explorer

The `web/` app is a Svelte 5 + Vite + MapLibre explorer powered by the WASM
adapter in `bindings/wasm`. It follows the osm2streets workflow: pan/zoom the
map, download the current OSM road ways from Overpass, then generate junctions
locally in WebAssembly. Parameters can be changed and both downloaded OSM JSON
and junction GeoJSON are available from the UI.

```sh
cd web
npm install
npm run check
npm run dev
```

The deployed explorer is <https://robinlovelace.github.io/junctions-rs/>.
OSM-derived downloads retain OpenStreetMap attribution and are subject to ODbL.

## Design

- **Core (`junctions-core`)**: stable Rust domain structs and algorithm; no Python, R, OSM API, DuckDB, or I/O dependencies.
- **Adapters**: CLI takes/returns GeoJSON; Python uses PyO3/maturin; R uses extendr. They only translate types and errors.
- **Future hosts**: keep the `Road → Junction` serialisable contract; add WASM/UniFFI, Node N-API, or Arrow/GeoArrow adapters without moving topology out of the core.
- **CRS boundary**: callers transform geographic OSM lines to a suitable local projected CRS before invoking the core. This avoids hidden CRS policy and makes metre parameters honest.

## Benchmarks

See [`docs/benchmarks.md`](docs/benchmarks.md) for live Leeds acquisition measurements across a pinned Geofabrik binary extract, Overpass, and Overture GeoParquet (1 km, 10 km, 100 km), plus the qualified Rust-vs-DuckDB core throughput comparison.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
/tmp/.qosm-venv/bin/maturin develop --release
pytest python-tests
```

## Licence

MIT. OSM-derived data must retain OpenStreetMap attribution and comply with ODbL.
