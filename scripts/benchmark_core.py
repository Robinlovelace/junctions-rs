"""Compare the Rust core against DuckDB junctions_cluster on equivalent endpoints."""
from pathlib import Path
from statistics import median
import math
import subprocess
import time
import duckdb

ROOT = Path(__file__).resolve().parents[1]
EXTENSION = Path("/home/robin/github/robinlovelace/junctions/build/release/extension/junctions/junctions.duckdb_extension")
N = 200

# Build a single in-memory table once; timed work is the macro execution only.
con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
con.execute("INSTALL spatial; LOAD spatial")
con.execute(f"LOAD '{EXTENSION}'")
con.execute("CREATE TABLE roads(geom_bng GEOMETRY, road_function VARCHAR)")
rows=[]
for index in range(N):
    angle = index * math.tau / N
    rows.append(f"(ST_GeomFromText('LINESTRING ({math.cos(angle)*100} {math.sin(angle)*100}, 0 0)'), 'Minor Road')")
con.execute("INSERT INTO roads VALUES " + ",".join(rows))

# First macro invocation plans/caches; use medians of warm runs.
duck=[]
for _ in range(7):
    started=time.perf_counter()
    assert con.execute("SELECT count(*) FROM junctions_cluster('roads', min_arms := 2, output_crs := 'EPSG:27700')").fetchone()[0] == 1
    duck.append((time.perf_counter()-started)*1000)

# The Rust example creates the same 200-road endpoint-only star; strip process startup
# by reading its internal elapsed time.
rust=[]
for _ in range(7):
    output=subprocess.check_output(["cargo", "run", "--quiet", "--release", "-p", "junctions-core", "--example", "benchmark"], cwd=ROOT, text=True).strip()
    fields=dict(field.split("=") for field in output.split(","))
    assert fields["junctions"] == "1"
    rust.append(float(fields["elapsed_ms"]))
result={"input_roads":N,"runs":7,"duckdb_warm_median_ms":median(duck),"rust_core_median_ms":median(rust),"speedup":median(duck)/median(rust),"notes":"Same synthetic endpoint-only 200-road star; DuckDB uses GEOS buffer/dissolve/convex hull and Rust uses deterministic square-hull output. This is a throughput comparison, not geometry-equivalence proof."}
(ROOT / "benchmarks" / "core.json").write_text(__import__("json").dumps(result,indent=2)+"\n")
print(result)
