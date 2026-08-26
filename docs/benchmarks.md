# Benchmarks: University of Leeds road data

**Run:** 2026-08-25 on the project host. Center: 53.8067 N, -1.5550 E/W; each circle is approximated by its WGS84 bounding box so all methods receive comparable rectangular predicates. Values are one cold-ish run, not a statistical network benchmark; endpoint availability and CDN/API load can dominate.

## Acquisition

| Radius | Geofabrik cached PBF local filter | Overpass road geometry transfer | Overture road GeoParquet materialised |
|---:|---:|---:|---:|
| 1 km | 2.253 s / 3,567 ways | **5.595 s** / 3,567 ways / 2.26 MB | 18.131 s / 3,403 segments / 0.25 MB |
| 10 km | **2.085 s** / 72,676 ways | 5.631 s / 72,692 ways / 49.38 MB | 20.511 s / 81,923 segments / 6.58 MB |
| 100 km | **1.716 s** / 218,546 ways* | 99.231 s / 1,837,279 ways / 1.34 GB | 29.482 s / 1,966,098 segments / 186.24 MB |

\*The selected West Yorkshire Geofabrik extract does **not cover the entire 100 km bbox**. Its count is therefore not a completeness comparison for that radius; it demonstrates the danger of choosing an undersized regional binary extract. For a correct 100 km study, select a containing Yorkshire/national extract or an API/cloud source.

The initial transfer of the pinned 2026-08-24 West Yorkshire PBF was 53.52 MB in 1.515 s. `osmextract`-style operation amortises that once via a persistent cache; its per-AOI time is then the local filter column. Geofabrik daily updates require pinning the dated URL and recording the extract date.

### Verdict

- **1 km:** Overpass is fastest among methods that return a complete fresh road network on this run; it is also exactly tied in returned OSM way count with the local binary filter. Use it for a one-off, small AOI, and cache the response.
- **10 km:** cached binary PBF is fastest; Overpass remains practical. Overture is slower but provides a compact, cloud-filtered segment product.
- **100 km:** Overture is the fastest **complete** result measured (29.5 s vs Overpass 99.2 s). A correct Geofabrik comparison needs a larger extract; the tiny West Yorkshire PBF cannot be presented as complete.
- **Repeated / offline / reproducible OSM work:** choose binary extracts, pin and cache them. **Fresh small custom queries:** choose Overpass, respecting endpoint capacity. **Large global/cross-country coverage:** choose Overture, while recognising it is an integrated data product rather than a verbatim OSM extract.

The raw JSON measurements are committed as `benchmarks/acquisition.json`, `benchmarks/overpass.json`, and `benchmarks/overture.json`; scripts are `scripts/benchmark_*.py`.

## Core throughput

A separate in-process, warm benchmark used a 200-road projected star with one shared endpoint. This matches the **endpoint-only** topology used by `junctions_cluster`; Rust had interior-crossing detection disabled because neither workload needs it.

| Engine | Median of 7 runs |
|---|---:|
| DuckDB `junctions_cluster` | 3.387 ms |
| Rust `junctions-core` | **0.137 ms** |

That is **24.7× faster** for this narrow synthetic endpoint workload. It is not a geometry-equivalence claim: DuckDB uses GEOS buffers/dissolve/convex hull and supports its full macro policy; the Rust prototype emits a deterministic square-derived convex hull. The result proves the Rust core can outperform the current macro on a compatible hot path, not that it has replaced the production extension.
