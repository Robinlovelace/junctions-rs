#!/usr/bin/env python3
"""Cold acquisition benchmark: Geofabrik PBF, Overpass, and Overture roads.

It reports wall time and returned road count for circles approximated by their
WGS84 bounding boxes around University of Leeds. Binary extracts are timed as
(a) initial regional PBF transfer and (b) local bbox filtering; production use
should cache the PBF, exactly as osmextract does.
"""
from __future__ import annotations

import json
import math
import os
import subprocess
import sys
import time
from pathlib import Path

import duckdb
import osmium
import requests

OUT = Path(__file__).resolve().parents[1] / "benchmarks"
OUT.mkdir(exist_ok=True)
CENTER = (53.8067, -1.5550)  # Parkinson Building / University of Leeds
RADII = (1, 10, 100)
PBF_URL = "https://download.geofabrik.de/europe/united-kingdom/england/west-yorkshire-260824.osm.pbf"
OVERTURE = "s3://overturemaps-us-west-2/release/2026-08-19.0/theme=transportation/type=segment/*"


def bbox(radius_km: int) -> tuple[float, float, float, float]:
    lat, lon = CENTER
    dlat = radius_km / 111.32
    dlon = radius_km / (111.32 * math.cos(math.radians(lat)))
    return lon - dlon, lat - dlat, lon + dlon, lat + dlat


class RoadCounter(osmium.SimpleHandler):
    def __init__(self, bounds: tuple[float, float, float, float]):
        super().__init__()
        self.west, self.south, self.east, self.north = bounds
        self.count = 0

    def way(self, way):
        if "highway" not in way.tags:
            return
        # Any node inside bbox is sufficient for a cheap benchmark counter.
        for node in way.nodes:
            if node.location.valid() and self.west <= node.location.lon <= self.east and self.south <= node.location.lat <= self.north:
                self.count += 1
                return


def elapsed(callable_):
    started = time.perf_counter()
    value = callable_()
    return time.perf_counter() - started, value


def download_pbf() -> tuple[Path, int]:
    target = OUT / "west-yorkshire-260824.osm.pbf"
    if target.exists():
        return target, target.stat().st_size
    response = requests.get(PBF_URL, stream=True, timeout=(20, 600))
    response.raise_for_status()
    with target.open("wb") as file:
        for chunk in response.iter_content(1024 * 1024):
            file.write(chunk)
    return target, target.stat().st_size


def binary_filter(pbf: Path, bounds):
    counter = RoadCounter(bounds)
    counter.apply_file(str(pbf), locations=True)
    return counter.count


def overpass(bounds):
    west, south, east, north = bounds
    query = f"[out:json][timeout:240];way[highway]({south},{west},{north},{east});out geom;"
    response = requests.post(
        "https://overpass-api.de/api/interpreter",
        data={"data": query},
        headers={"User-Agent": "junctions-rs benchmark/0.1 (github.com/Robinlovelace/junctions-rs)"},
        timeout=(20, 300),
    )
    response.raise_for_status()
    payload = response.json()
    return len(payload.get("elements", [])), len(response.content)


def overture(bounds):
    west, south, east, north = bounds
    con = duckdb.connect()
    con.execute("INSTALL httpfs; LOAD httpfs; INSTALL spatial; LOAD spatial; SET s3_region='us-west-2'")
    query = f"""
        SELECT count(*)
        FROM read_parquet('{OVERTURE}', hive_partitioning=1)
        WHERE subtype = 'road'
          AND bbox.xmin <= {east} AND bbox.xmax >= {west}
          AND bbox.ymin <= {north} AND bbox.ymax >= {south}
    """
    return con.execute(query).fetchone()[0]


def main():
    rows = []
    seconds, (pbf, bytes_downloaded) = elapsed(download_pbf)
    rows.append({"method": "geofabrik_pbf_download", "radius_km": None, "seconds": seconds, "roads": None, "bytes": bytes_downloaded, "note": "one West Yorkshire extract shared by all radii"})
    for radius in RADII:
        bounds = bbox(radius)
        seconds, roads = elapsed(lambda: binary_filter(pbf, bounds))
        rows.append({"method": "geofabrik_pbf_local_filter", "radius_km": radius, "seconds": seconds, "roads": roads, "bytes": bytes_downloaded, "note": "cached binary extract; scans full regional PBF"})
        try:
            seconds, (roads, size) = elapsed(lambda: overpass(bounds))
            rows.append({"method": "overpass_api", "radius_km": radius, "seconds": seconds, "roads": roads, "bytes": size, "note": "fresh API query; bbox approximation of circle"})
        except Exception as error:
            rows.append({"method": "overpass_api", "radius_km": radius, "seconds": None, "roads": None, "bytes": None, "note": f"FAILED: {type(error).__name__}: {error}"})
        try:
            seconds, roads = elapsed(lambda: overture(bounds))
            rows.append({"method": "overture_geoparquet", "radius_km": radius, "seconds": seconds, "roads": roads, "bytes": None, "note": "cloud GeoParquet with bbox predicate pushdown"})
        except Exception as error:
            rows.append({"method": "overture_geoparquet", "radius_km": radius, "seconds": None, "roads": None, "bytes": None, "note": f"FAILED: {type(error).__name__}: {error}"})
        print(json.dumps(rows[-3:], indent=2), flush=True)
    (OUT / "acquisition.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(json.dumps(rows, indent=2))

if __name__ == "__main__":
    main()
