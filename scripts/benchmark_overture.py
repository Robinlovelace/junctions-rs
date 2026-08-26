#!/usr/bin/env python3
"""Materialise Overture road geometry for the three Leeds benchmark bboxes."""
import importlib.util
import json
import time
from pathlib import Path
import duckdb

spec = importlib.util.spec_from_file_location("bench", Path(__file__).with_name("benchmark_acquisition.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
rows=[]
for radius in module.RADII:
    west, south, east, north = module.bbox(radius)
    target = Path("benchmarks") / f"overture-{radius}km.parquet"
    target.unlink(missing_ok=True)
    con=duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; SET s3_region='us-west-2'")
    started=time.perf_counter()
    try:
        con.execute(f"""COPY (
          SELECT id, class, geometry FROM read_parquet('{module.OVERTURE}', hive_partitioning=1)
          WHERE subtype='road' AND bbox.xmin <= {east} AND bbox.xmax >= {west}
            AND bbox.ymin <= {north} AND bbox.ymax >= {south}
        ) TO '{target}' (FORMAT PARQUET, COMPRESSION ZSTD)""")
        count=con.execute(f"SELECT count(*) FROM read_parquet('{target}')").fetchone()[0]
        row={"method":"overture_geoparquet_materialized","radius_km":radius,"seconds":time.perf_counter()-started,"roads":count,"bytes":target.stat().st_size,"note":"remote GeoParquet predicate query with geometry materialised locally"}
    except Exception as error:
        row={"method":"overture_geoparquet_materialized","radius_km":radius,"seconds":None,"roads":None,"bytes":None,"note":f"FAILED: {type(error).__name__}: {error}"}
    rows.append(row); print(json.dumps(row), flush=True)
Path("benchmarks/overture.json").write_text(json.dumps(rows,indent=2)+"\n")
