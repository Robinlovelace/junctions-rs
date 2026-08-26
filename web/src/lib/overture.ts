import * as duckdb from '@duckdb/duckdb-wasm';
import duckdbWasm from '@duckdb/duckdb-wasm/dist/duckdb-mvp.wasm?url';
import duckdbWasmEh from '@duckdb/duckdb-wasm/dist/duckdb-eh.wasm?url';
import duckdbWorker from '@duckdb/duckdb-wasm/dist/duckdb-browser-mvp.worker.js?url';
import duckdbWorkerEh from '@duckdb/duckdb-wasm/dist/duckdb-browser-eh.worker.js?url';
import { tableToIPC } from 'apache-arrow';

export type Bounds = { west: number; south: number; east: number; north: number; wraps?: boolean };
export type OvertureRoads = {
  arrowIpc: Uint8Array;
  roadGeoJson: GeoJSON.FeatureCollection;
  count: number;
  release: string;
  /** UTM zone CRS used for the metre-based detector, as a proj4 string. */
  crsProj: string;
  /** UTM EPSG code used for the DuckDB ST_Transform (e.g. 32630). */
  epsg: number;
};

type StacLink = { rel?: string; href: string; title?: string };
type StacCatalog = { links: StacLink[] };
type StacItem = { bbox?: [number, number, number, number]; assets?: Record<string, { href: string }> };

const STAC_CATALOG = 'https://stac.overturemaps.org/catalog.json';
const BUNDLES: duckdb.DuckDBBundles = {
  mvp: { mainModule: duckdbWasm, mainWorker: duckdbWorker },
  eh: { mainModule: duckdbWasmEh, mainWorker: duckdbWorkerEh }
};

let connectionPromise: Promise<duckdb.AsyncDuckDBConnection> | undefined;

// Reuse the previous query when the new viewport overlaps it almost entirely,
// so panning around one area does not re-download matching row groups.
let cache: (OvertureRoads & { bounds: Bounds }) | undefined;

function firstLink(catalog: StacCatalog, rel: string, title?: string): string {
  const link = catalog.links.find((candidate) => candidate.rel === rel && (!title || candidate.title === title));
  if (!link) throw new Error(`Overture STAC catalog is missing its ${title ?? ''} ${rel} link.`);
  return link.href;
}

function intersects(bounds: Bounds, bbox?: [number, number, number, number]): boolean {
  if (!bbox) return false;
  const lonOverlap = bounds.wraps
    ? bbox[2] >= bounds.west || bbox[0] <= bounds.east
    : !(bbox[2] < bounds.west || bbox[0] > bounds.east);
  return lonOverlap && !(bbox[3] < bounds.south || bbox[1] > bounds.north);
}

/** Viewport-centre longitude in [-180, 180), antimeridian-aware. */
function boundsCenterLon(bounds: Bounds): number {
  const span = bounds.wraps ? bounds.east - bounds.west + 360 : bounds.east - bounds.west;
  let lon = bounds.west + span / 2;
  if (lon > 180) lon -= 360;
  if (lon < -180) lon += 360;
  return lon;
}

async function stacJson<T>(url: string): Promise<T> {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`Overture STAC returned HTTP ${response.status}.`);
  return response.json() as Promise<T>;
}

async function overtureAssets(bounds: Bounds): Promise<{ assets: string[]; release: string }> {
  const releases = await stacJson<StacCatalog>(STAC_CATALOG);
  const releaseUrl = firstLink(releases, 'child', 'Latest Overture Release');
  const root = await stacJson<StacCatalog>(releaseUrl);
  const release = new URL(releaseUrl).pathname.split('/').filter(Boolean)[0] ?? 'latest';
  const transport = await stacJson<StacCatalog>(firstLink(root, 'child', 'transportation'));
  const segments = await stacJson<StacCatalog>(firstLink(transport, 'child', 'segment'));
  const items = await Promise.all(segments.links.filter((link) => link.rel === 'item').map((link) => stacJson<StacItem>(link.href)));
  const assets = items
    .filter((item) => intersects(bounds, item.bbox))
    .map((item) => item.assets?.aws?.href ?? item.assets?.azure?.href)
    .filter((url): url is string => Boolean(url));
  if (assets.length === 0) throw new Error('No Overture transportation shards intersect this map area.');
  return { assets, release };
}

async function connection(onProgress: (message: string) => void): Promise<duckdb.AsyncDuckDBConnection> {
  if (!connectionPromise) {
    connectionPromise = (async () => {
      onProgress('Starting DuckDB-WASM in a worker…');
      const bundle = await duckdb.selectBundle(BUNDLES);
      const worker = new Worker(bundle.mainWorker!);
      const database = new duckdb.AsyncDuckDB(new duckdb.VoidLogger(), worker);
      await database.instantiate(bundle.mainModule, bundle.pthreadWorker);
      const connection = await database.connect();
      onProgress('Loading DuckDB Spatial for BNG projection…');
      await connection.query('INSTALL spatial; LOAD spatial;');
      return connection;
    })();
  }
  return connectionPromise;
}

function sqlString(value: string): string {
  return `'${value.replaceAll("'", "''")}'`;
}

/** UTM zone (1-60) for a longitude, WGS84. */
function utmZone(lon: number): number {
  return Math.floor((lon + 180) / 6) + 1;
}

/**
 * Pick a metre-based projected CRS valid anywhere on Earth: WGS84 UTM, zone
 * chosen from the viewport centre. 326xx = northern hemisphere, 327xx = south.
 */
export function utmCrs(center: { lon: number; lat: number }): { epsg: number; proj: string } {
  const zone = utmZone(center.lon);
  const south = center.lat < 0 ? '+south' : '';
  return {
    epsg: (center.lat >= 0 ? 32600 : 32700) + zone,
    proj: `+proj=utm +zone=${zone}${south} +datum=WGS84 +units=m +no_defs`
  };
}

function sourceSql(assets: string[], bounds: Bounds, epsg: number): string {
  const files = assets.map(sqlString).join(', ');
  // A wrapped viewport (west > east, antimeridian) overlaps longitudes on both
  // sides of ±180, so the bbox predicate becomes an OR of the two slivers.
  const lonPredicate = bounds.wraps
    ? `(bbox.xmin <= 180 AND bbox.xmax >= ${bounds.west}) OR (bbox.xmin <= ${bounds.east} AND bbox.xmax >= -180)`
    : `bbox.xmin <= ${bounds.east} AND bbox.xmax >= ${bounds.west}`;
  return `
    CREATE TEMP TABLE overture_roads AS
    SELECT
      id,
      geometry,
      COALESCE(level_rules[1].value, 0)::INTEGER AS level
    FROM read_parquet([${files}])
    WHERE subtype = 'road'
      AND (${lonPredicate})
      AND bbox.ymin <= ${bounds.north}
      AND bbox.ymax >= ${bounds.south}`;
}

function projectionSql(epsg: number): string {
  return `
    SELECT
      id,
      ST_AsWKB(ST_Transform(geometry, 'EPSG:4326', 'EPSG:${epsg}')) AS geometry,
      level,
      ST_AsGeoJSON(geometry) AS geometry_json
    FROM overture_roads`;
}

export async function loadOvertureRoads(bounds: Bounds, onProgress: (message: string) => void): Promise<OvertureRoads> {
  if (cache && cacheContains(cache.bounds, bounds)) {
    onProgress('Reusing the previous Overture query for this area.');
    return cache;
  }
  onProgress('Resolving Overture STAC assets for this map area…');
  const { assets, release } = await overtureAssets(bounds);
  onProgress(`Selected ${assets.length} Overture GeoParquet shard${assets.length === 1 ? '' : 's'}; starting DuckDB…`);
  const database = await connection(onProgress);
  const started = Date.now();
  onProgress('Reading Overture GeoParquet… the first query for an area takes about a minute (row groups span whole regions); repeat queries reuse the cached result.');
  const center = { lon: boundsCenterLon(bounds), lat: (bounds.south + bounds.north) / 2 };
  const { epsg, proj } = utmCrs(center);
  await database.query(sourceSql(assets, bounds, epsg));
  const ingestMs = Date.now() - started;
  onProgress(`Filtered Overture segments in ${(ingestMs / 1000).toFixed(0)} s; projecting to UTM zone ${utmZone(center.lon)}…`);
  const table = await database.query(projectionSql(epsg));
  const rows = table.toArray() as { id: string; level: number; geometry_json: string }[];
  const roadGeoJson: GeoJSON.FeatureCollection = {
    type: 'FeatureCollection',
    features: rows.map((row) => ({
      type: 'Feature',
      properties: { id: row.id, level: row.level },
      geometry: JSON.parse(row.geometry_json) as GeoJSON.Geometry
    }))
  };
  const arrowIpc = tableToIPC(table.select(['id', 'geometry', 'level']), 'stream');
  cache = { arrowIpc, roadGeoJson, count: rows.length, release, crsProj: proj, epsg, bounds };
  return cache;
}

function cacheContains(cached: Bounds, requested: Bounds): boolean {
  if (cached.wraps !== requested.wraps) return false;
  const overlapX = Math.min(cached.east, requested.east) - Math.max(cached.west, requested.west);
  const overlapY = Math.min(cached.north, requested.north) - Math.max(cached.south, requested.south);
  const width = requested.east - requested.west;
  const height = requested.north - requested.south;
  return overlapX >= 0.8 * width && overlapY >= 0.8 * height;
}
