#!/usr/bin/env python3
"""Run the corrected Overpass road geometry transfer benchmark only."""
import importlib.util
import json
import time
from pathlib import Path

spec = importlib.util.spec_from_file_location("bench", Path(__file__).with_name("benchmark_acquisition.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
rows = []
for radius in module.RADII:
    started = time.perf_counter()
    try:
        roads, size = module.overpass(module.bbox(radius))
        row = {"method": "overpass_api", "radius_km": radius, "seconds": time.perf_counter() - started, "roads": roads, "bytes": size, "note": "fresh API query; bbox approximation of circle"}
    except Exception as error:
        row = {"method": "overpass_api", "radius_km": radius, "seconds": None, "roads": None, "bytes": None, "note": f"FAILED: {type(error).__name__}: {error}"}
    rows.append(row)
    print(json.dumps(row), flush=True)
Path("benchmarks/overpass.json").write_text(json.dumps(rows, indent=2) + "\n")
