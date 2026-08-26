<script lang="ts">
  import { onMount } from 'svelte';
  import * as maplibregl from 'maplibre-gl';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import proj4 from 'proj4';
  import { loadOvertureRoads } from './lib/overture';
  import { parseOsmPbf } from './lib/pbf';
  import { BUFFER_PRESETS, bufferForTags } from './lib/roadClasses';
  import { generate_junctions, generate_junctions_arrow } from './lib/wasm/junctions_wasm';

  type Element = { type: string; id: number; nodes?: number[]; geometry?: { lat: number; lon: number }[]; tags?: Record<string, string> };
  type OsmResponse = { elements: Element[] };
  type Road = { id: string; coordinates: [number, number][]; node_ids: string[]; level: number; buffer_m?: number };
  type Origin = { lon: number; lat: number };

  // Mirror of the example-leeds-station-200m release asset, committed to the
  // repo so it is served with CORS headers (GitHub release assets are not).
  const EXAMPLE_ASSET = 'https://raw.githubusercontent.com/Robinlovelace/junctions-rs/main/data/examples/leeds-station-200m.osm.pbf';
  const EXAMPLE_CENTER: Origin = { lon: -1.5474, lat: 53.795 };

  let mapContainer: HTMLElement;
  let map = $state<maplibregl.Map | null>(null);
  let origin = $state<Origin>({ lon: -1.555, lat: 53.8067 });
  // Retain only the fields the detector and map require. The raw Overpass
  // response contains tags, wrappers, and WGS84 geometry that are not needed
  // after this normalization step.
  let roads = $state.raw<Road[]>([]);
  let overtureArrowIpc = $state.raw<Uint8Array | null>(null);
  // Detector output CRS: 'local' = origin-anchored tangent plane (OSM path);
  // a proj4 string = UTM zone for the Overture path (valid worldwide).
  let detectorCrs = $state<'local' | string>('local');
  let hasRoadData = $state(false);
  let junctions = $state<GeoJSON.FeatureCollection | null>(null);
  let status = $state('Pan or zoom to an area, then get OSM data.');
  let error = $state('');
  let loading = $state(false);
  let minArms = $state(3);
  let bufferM = $state(5);
  let bufferPresetIndex = $state(0);
  let clusterDistanceM = $state(0.01);
  let detectIntersections = $state(true);
  let panelOpen = $state(true);
  /** OSM tags per road id, so a buffer-preset change can re-derive radii. */
  let roadTags = $state<Record<string, Record<string, string>>>({});

  const mapStyle = 'https://tiles.openfreemap.org/styles/bright';
  const overpass = 'https://overpass-api.de/api/interpreter';
  /** Default view: University of Leeds campus with a ~200 m window. */
  const DEFAULT_VIEW: Origin = { lon: -1.556, lat: 53.808 };
  const DEFAULT_WINDOW_M = 200;

  function initialView(): { center: [number, number]; zoom: number; fromUrl: boolean } {
    const params = new URLSearchParams(window.location.search);
    // Number(null) is 0, so missing params must yield NaN, not a phantom (0, 0).
    const param = (name: string): number => {
      const value = params.get(name);
      return value === null ? NaN : Number(value);
    };
    const lat = param('lat');
    const lng = param('lng');
    const zoom = param('z');
    if (Number.isFinite(lat) && Number.isFinite(lng) && lat >= -90 && lat <= 90 && lng >= -180 && lng <= 180) {
      return { center: [lng, lat], zoom: Number.isFinite(zoom) ? Math.min(Math.max(zoom, 2), 19) : 15, fromUrl: true };
    }
    return { center: [DEFAULT_VIEW.lon, DEFAULT_VIEW.lat], zoom: 17.5, fromUrl: false };
  }

  onMount(() => {
    const view = initialView();
    origin = { lon: view.center[0], lat: view.center[1] };
    map = new maplibregl.Map({ container: mapContainer, style: mapStyle, center: view.center, zoom: view.zoom });
    map.addControl(new maplibregl.NavigationControl(), 'top-right');
    map.addControl(new maplibregl.ScaleControl({ unit: 'metric' }));
    map.on('load', () => {
      addSources();
      if (!view.fromUrl) {
        // Fit a ~200 m window around the default centre, whatever the canvas size.
        const half = DEFAULT_WINDOW_M / 2;
        const dLon = half / (111320 * Math.cos((view.center[1] * Math.PI) / 180));
        const dLat = half / 110574;
        map!.fitBounds(
          [[view.center[0] - dLon, view.center[1] - dLat], [view.center[0] + dLon, view.center[1] + dLat]],
          { duration: 0 }
        );
      }
    });
    return () => map?.remove();
  });

  function addSources() {
    if (!map || map.getSource('roads')) return;
    map.addSource('roads', { type: 'geojson', data: emptyCollection() });
    map.addLayer({ id: 'roads', type: 'line', source: 'roads', paint: { 'line-color': '#64748b', 'line-width': 2, 'line-opacity': 0.8 } });
    map.addSource('junctions', { type: 'geojson', data: emptyCollection() });
    map.addLayer({ id: 'junction-fills', type: 'fill', source: 'junctions', paint: { 'fill-color': '#f97316', 'fill-opacity': 0.55, 'fill-outline-color': '#9a3412' } });
    map.addLayer({ id: 'junction-points', type: 'circle', source: 'junctions', paint: { 'circle-color': '#c2410c', 'circle-radius': 4, 'circle-stroke-color': '#fff', 'circle-stroke-width': 1 } });
  }

  function emptyCollection(): GeoJSON.FeatureCollection { return { type: 'FeatureCollection', features: [] }; }

  function normalizeRoads(osm: OsmResponse): Road[] {
    const preset = BUFFER_PRESETS[bufferPresetIndex];
    const result: Road[] = [];
    roadTags = {};
    for (const e of osm.elements) {
      if (e.type !== 'way' || !e.geometry || e.geometry.length < 2) continue;
      const tags = e.tags ?? {};
      roadTags[String(e.id)] = tags;
      result.push({
        id: String(e.id),
        coordinates: e.geometry!.map((p) => project([p.lon, p.lat])),
        node_ids: (e.nodes ?? []).map(String),
        level: roadLevel(tags),
        buffer_m: bufferForTags(preset, tags)
      });
    }
    return result;
  }

  /** Re-apply the selected buffer preset to in-memory roads (live for Generate). */
  function applyBufferPreset() {
    const preset = BUFFER_PRESETS[bufferPresetIndex];
    bufferM = preset.fallback;
    roads = roads.map((road) => ({ ...road, buffer_m: bufferForTags(preset, roadTags[road.id] ?? {}) }));
    if (roads.length > 0) status = `Re-applied "${preset.name}" buffers to ${roads.length.toLocaleString()} road ways. Click Generate junctions.`;
  }

  async function loadExample() {
    if (!map) return;
    loading = true; error = ''; junctions = null;
    status = 'Downloading the Leeds station example PBF…';
    try {
      origin = EXAMPLE_CENTER;
      map.flyTo({ center: [EXAMPLE_CENTER.lon, EXAMPLE_CENTER.lat], zoom: 16, duration: 800 });
      const response = await fetch(EXAMPLE_ASSET);
      if (!response.ok) throw new Error(`Example data request returned HTTP ${response.status}`);
      const bytes = new Uint8Array(await response.arrayBuffer());
      status = 'Parsing the OSM PBF in the browser…';
      const { nodes, ways } = await parseOsmPbf(bytes);
      const preset = BUFFER_PRESETS[bufferPresetIndex];
      const result: Road[] = [];
      roadTags = {};
      for (const way of ways) {
        if (!way.tags.highway || way.tags.highway === 'construction' || way.tags.highway === 'corridor') continue;
        const coordinates: [number, number][] = [];
        const nodeIds: string[] = [];
        for (const ref of way.refs) {
          const coord = nodes.get(ref);
          if (!coord) continue;
          coordinates.push(project(coord));
          nodeIds.push(String(ref));
        }
        if (coordinates.length < 2) continue;
        const tags = way.tags;
        roadTags[String(way.id)] = tags;
        result.push({
          id: String(way.id),
          coordinates,
          node_ids: nodeIds,
          level: roadLevel(tags),
          buffer_m: bufferForTags(preset, tags)
        });
      }
      roads = result;
      overtureArrowIpc = null;
      detectorCrs = 'local';
      hasRoadData = roads.length > 0;
      setSource('roads', roadsGeoJson());
      status = `Loaded ${roads.length.toLocaleString()} example road ways (200 m around Leeds station) from the release PBF. Click Generate junctions.`;
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'Example data load failed.'; }
    finally { loading = false; }
  }

  function roadLevel(tags: Record<string, string>): number {
    const layer = Number.parseInt(tags.layer ?? '', 10);
    if (Number.isFinite(layer)) return layer;
    if (tags.bridge === 'yes' || tags.bridge === 'true') return 1;
    if (tags.tunnel === 'yes' || tags.tunnel === 'true') return -1;
    return 0;
  }

  /** Longitude difference in [-180, 180), safe across the antimeridian. */
  function lonDiff(lon: number, refLon: number): number {
    return ((lon - refLon + 540) % 360) - 180;
  }

  function project(point: [number, number]): [number, number] {
    const r = 6371000;
    const rad = Math.PI / 180;
    return [r * Math.cos(origin.lat * rad) * lonDiff(point[0], origin.lon) * rad, r * (point[1] - origin.lat) * rad];
  }

  function unproject(point: number[]): [number, number] {
    const r = 6371000;
    const rad = Math.PI / 180;
    return [origin.lon + lonDiff(origin.lon + point[0] / (r * Math.cos(origin.lat * rad)) / rad, origin.lon), origin.lat + point[1] / r / rad];
  }

  /** Visible bounds; when the view crosses the antimeridian, west > east. */
  function viewportBounds() {
    const bounds = map!.getBounds();
    const west = bounds.getWest();
    const east = bounds.getEast();
    const south = bounds.getSouth();
    const north = bounds.getNorth();
    return { west, east, south, north, wraps: west > east };
  }

  async function fetchOsmBox(south: number, west: number, north: number, east: number): Promise<OsmResponse> {
    const query = `[out:json][timeout:60];way[highway](${south},${west},${north},${east});out body geom;`;
    const response = await fetch(`${overpass}?data=${encodeURIComponent(query)}`);
    if (!response.ok) throw new Error(`Overpass returned HTTP ${response.status}`);
    return response.json() as Promise<OsmResponse>;
  }

  function roadsGeoJson(): GeoJSON.FeatureCollection {
    return { type: 'FeatureCollection', features: roads.map((road) => ({ type: 'Feature', properties: { id: road.id, level: road.level }, geometry: { type: 'LineString', coordinates: road.coordinates } })) };
  }

  function setSource(id: string, data: GeoJSON.GeoJSON) {
    const source = map?.getSource(id) as maplibregl.GeoJSONSource | undefined;
    source?.setData(data);
  }

  /** Centre of the visible viewport, antimeridian-aware (lon in [-180, 180)). */
  function viewportCenter(): Origin {
    const { west, east, south, north, wraps } = viewportBounds();
    const span = wraps ? east - west + 360 : east - west;
    let lon = west + span / 2;
    if (lon > 180) lon -= 360;
    if (lon < -180) lon += 360;
    return { lon, lat: (south + north) / 2 };
  }

  async function downloadOsm() {
    if (!map) return;
    const zoom = map.getZoom();
    if (zoom < 12) { error = 'Zoom in to level 12 or closer before getting data (keeps the public API request small).'; return; }
    loading = true; error = ''; junctions = null; status = 'Getting OSM data from Overpass…';
    try {
      const { west, east, south, north, wraps } = viewportBounds();
      origin = viewportCenter();
      const osm = wraps
        ? { elements: [...(await fetchOsmBox(south, west, north, 180)).elements, ...(await fetchOsmBox(south, -180, north, east)).elements] }
        : await fetchOsmBox(south, west, north, east);
      roads = normalizeRoads(osm);
      overtureArrowIpc = null;
      detectorCrs = 'local';
      hasRoadData = roads.length > 0;
      setSource('roads', roadsGeoJson());
      status = `Loaded ${roads.length.toLocaleString()} road ways in browser memory. Click Generate junctions.`;
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'OSM request failed.'; }
    finally { loading = false; }
  }

  async function getOvertureRoads() {
    if (!map) return;
    if (map.getZoom() < 12) { error = 'Zoom in to level 12 or closer before getting Overture data (keeps the GeoParquet query small).'; return; }
    loading = true; error = ''; junctions = null; status = 'Resolving Overture GeoParquet assets and starting DuckDB-WASM…';
    try {
      const { west, east, south, north, wraps } = viewportBounds();
      origin = viewportCenter();
      const result = await loadOvertureRoads(
        { west, south, east, north, wraps },
        (message) => { status = message; }
      );
      roads = [];
      overtureArrowIpc = result.arrowIpc;
      detectorCrs = result.crsProj;
      hasRoadData = result.count > 0;
      setSource('roads', result.roadGeoJson);
      status = `Loaded ${result.count.toLocaleString()} Overture road segments from ${result.release} via GeoParquet and DuckDB-WASM. Click Generate junctions.`;
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'Overture GeoParquet query failed.'; }
    finally { loading = false; }
  }

  function displayPoint(point: number[]): [number, number] {
    return detectorCrs === 'local' ? unproject(point) : proj4(detectorCrs, 'EPSG:4326', point) as [number, number];
  }

  async function generate() {
    if (!hasRoadData) { error = 'Get OSM or Overture road data first.'; return; }
    loading = true; error = ''; status = 'Running junction detection in WebAssembly…';
    try {
      const config = { buffer_m: bufferM, min_arms: minArms, cluster_distance_m: clusterDistanceM, detect_intersections: detectIntersections };
      const projected = JSON.parse(overtureArrowIpc
        ? generate_junctions_arrow(overtureArrowIpc, JSON.stringify(config))
        : generate_junctions(JSON.stringify(roads), JSON.stringify(config))) as GeoJSON.FeatureCollection;
      projected.features.forEach((feature) => {
        if (feature.geometry?.type === 'MultiPolygon') feature.geometry.coordinates = feature.geometry.coordinates.map((polygon) => polygon.map((ring) => ring.map((point) => displayPoint(point))));
      });
      junctions = projected; setSource('junctions', projected);
      status = `Generated ${projected.features.length.toLocaleString()} junctions in the browser.`;
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'Generation failed.'; }
    finally { loading = false; }
  }

  function downloadJunctions() {
    if (!junctions) return;
    const blob = new Blob([JSON.stringify(junctions, null, 2)], { type: 'application/geo+json' });
    const link = document.createElement('a'); link.href = URL.createObjectURL(blob); link.download = 'junctions.geojson'; link.click(); URL.revokeObjectURL(link.href);
 }

 function hidePanel() { panelOpen = false; }
 function showPanel() { panelOpen = true; }
 </script>

<div class="app-shell">
  <aside class:panel-hidden={!panelOpen} class="panel" id="control-panel" aria-label="Junction detection controls" aria-hidden={!panelOpen}>
      <div class="panel-header">
        <div>
          <div class="eyebrow">junctions-rs · WebAssembly</div>
          <h1>Junction explorer</h1>
        </div>
        <button class="panel-toggle" type="button" onclick={hidePanel} aria-expanded={panelOpen} aria-controls="control-panel" aria-label="Hide controls" title="Hide controls">
          <ChevronDown size={20} strokeWidth={2.5} aria-hidden="true" />
        </button>
      </div>
      <p class="intro">Browse an area, get OpenStreetMap road data into browser memory, then detect junctions locally. Overture Maps GeoParquet is available for larger areas.</p>
      <div class="actions">
        <button class="primary" onclick={downloadOsm} disabled={loading}>{loading ? 'Working…' : 'Get OSM data'}</button>
        <button onclick={loadExample} disabled={loading}>Use example data</button>
        <button onclick={getOvertureRoads} disabled={loading}>Get Overture roads</button>
        <button onclick={generate} disabled={loading || !hasRoadData}>Generate junctions</button>
        <button class="secondary" onclick={downloadJunctions} disabled={!junctions}>Download junction GeoJSON</button>
      </div>
      <section>
        <h2>Detection parameters</h2>
        <div class="parameter-grid">
          <label>Buffer size by road class
            <select bind:value={bufferPresetIndex} onchange={applyBufferPreset}>
              {#each BUFFER_PRESETS as preset, index (preset.name)}
                <option value={index}>{preset.name}</option>
              {/each}
            </select>
          </label>
          <label>Fallback buffer (m) <input type="number" min="0.1" max="50" step="0.5" bind:value={bufferM} /></label>
          <label>Minimum arms <input type="number" min="2" max="12" step="1" bind:value={minArms} /></label>
          <label>Cluster distance (m) <input type="number" min="0" max="10" step="0.01" bind:value={clusterDistanceM} /></label>
          <label class="check"><input type="checkbox" bind:checked={detectIntersections} /> Detect interior crossings</label>
        </div>
      </section>
      <p class="status">{status}</p>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <p class="attribution">Basemap © OpenFreeMap contributors · OSM data © <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a> contributors via Overpass · Overture Maps data via GeoParquet</p>
  </aside>
  <button class:show-panel-hidden={panelOpen} class="show-panel" type="button" onclick={showPanel} aria-expanded={panelOpen} aria-controls="control-panel" aria-label="Show controls" title="Show controls">
      <SlidersHorizontal size={21} strokeWidth={2.5} aria-hidden="true" />
      <span>Controls</span>
      <ChevronUp size={18} strokeWidth={2.5} aria-hidden="true" />
  </button>
  <main bind:this={mapContainer} class="map"></main>
</div>
