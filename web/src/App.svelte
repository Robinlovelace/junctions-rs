<script lang="ts">
  import { onMount } from 'svelte';
  import * as maplibregl from 'maplibre-gl';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import proj4 from 'proj4';
  import { loadOvertureRoads } from './lib/overture';
  import { generate_junctions, generate_junctions_arrow } from './lib/wasm/junctions_wasm';

  type Element = { type: string; id: number; nodes?: number[]; geometry?: { lat: number; lon: number }[]; tags?: Record<string, string> };
  type OsmResponse = { elements: Element[] };
  type Road = { id: string; coordinates: [number, number][]; node_ids: string[]; level: number };
  type Origin = { lon: number; lat: number };

  let mapContainer: HTMLElement;
  let map = $state<maplibregl.Map | null>(null);
  let origin = $state<Origin>({ lon: -1.555, lat: 53.8067 });
  // Retain only the fields the detector and map require. The raw Overpass
  // response contains tags, wrappers, and WGS84 geometry that are not needed
  // after this normalization step.
  let roads = $state.raw<Road[]>([]);
  let overtureArrowIpc = $state.raw<Uint8Array | null>(null);
  let detectorCrs = $state<'local' | 'bng'>('local');
  let hasRoadData = $state(false);
  let junctions = $state<GeoJSON.FeatureCollection | null>(null);
  let status = $state('Pan or zoom to an area, then get OSM data.');
  let error = $state('');
  let loading = $state(false);
  let minArms = $state(3);
  let bufferM = $state(5);
  let clusterDistanceM = $state(0.01);
  let detectIntersections = $state(true);
  let panelOpen = $state(true);

  const mapStyle = 'https://tiles.openfreemap.org/styles/bright';
  const overpass = 'https://overpass-api.de/api/interpreter';
  proj4.defs('EPSG:27700', '+proj=tmerc +lat_0=49 +lon_0=-2 +k=0.9996012717 +x_0=400000 +y_0=-100000 +ellps=airy +datum=OSGB36 +units=m +no_defs');

  onMount(() => {
    map = new maplibregl.Map({ container: mapContainer, style: mapStyle, center: [origin.lon, origin.lat], zoom: 13 });
    map.addControl(new maplibregl.NavigationControl(), 'top-right');
    map.addControl(new maplibregl.ScaleControl({ unit: 'metric' }));
    map.on('load', () => addSources());
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
    return osm.elements.filter((e) => e.type === 'way' && e.geometry && e.geometry.length >= 2).map((e) => ({
      id: String(e.id),
      coordinates: e.geometry!.map((p) => project([p.lon, p.lat])),
      node_ids: (e.nodes ?? []).map(String),
      level: roadLevel(e.tags ?? {})
    }));
  }

  function roadLevel(tags: Record<string, string>): number {
    const layer = Number.parseInt(tags.layer ?? '', 10);
    if (Number.isFinite(layer)) return layer;
    if (tags.bridge === 'yes' || tags.bridge === 'true') return 1;
    if (tags.tunnel === 'yes' || tags.tunnel === 'true') return -1;
    return 0;
  }

  function project(point: [number, number]): [number, number] {
    const r = 6371000;
    const rad = Math.PI / 180;
    return [r * Math.cos(origin.lat * rad) * (point[0] - origin.lon) * rad, r * (point[1] - origin.lat) * rad];
  }

  function unproject(point: number[]): [number, number] {
    const r = 6371000;
    const rad = Math.PI / 180;
    return [origin.lon + point[0] / (r * Math.cos(origin.lat * rad)) / rad, origin.lat + point[1] / r / rad];
  }

  function roadsGeoJson(): GeoJSON.FeatureCollection {
    return { type: 'FeatureCollection', features: roads.map((road) => ({ type: 'Feature', properties: { id: road.id, level: road.level }, geometry: { type: 'LineString', coordinates: road.coordinates } })) };
  }

  function setSource(id: string, data: GeoJSON.GeoJSON) {
    const source = map?.getSource(id) as maplibregl.GeoJSONSource | undefined;
    source?.setData(data);
  }

  async function downloadOsm() {
    if (!map) return;
    const zoom = map.getZoom();
    if (zoom < 12) { error = 'Zoom in to level 12 or closer before getting data (keeps the public API request small).'; return; }
    loading = true; error = ''; junctions = null; status = 'Getting OSM data from Overpass…';
    try {
      const bounds = map.getBounds();
      const [west, south] = [bounds.getWest(), bounds.getSouth()];
      const [east, north] = [bounds.getEast(), bounds.getNorth()];
      origin = { lon: (west + east) / 2, lat: (south + north) / 2 };
      const query = `[out:json][timeout:60];way[highway](${south},${west},${north},${east});out body geom;`;
      const response = await fetch(`${overpass}?data=${encodeURIComponent(query)}`);
      if (!response.ok) throw new Error(`Overpass returned HTTP ${response.status}`);
      roads = normalizeRoads(await response.json() as OsmResponse);
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
      const bounds = map.getBounds();
      const result = await loadOvertureRoads(
        { west: bounds.getWest(), south: bounds.getSouth(), east: bounds.getEast(), north: bounds.getNorth() },
        (message) => { status = message; }
      );
      roads = [];
      overtureArrowIpc = result.arrowIpc;
      detectorCrs = 'bng';
      hasRoadData = result.count > 0;
      setSource('roads', result.roadGeoJson);
      status = `Loaded ${result.count.toLocaleString()} Overture road segments from ${result.release} via GeoParquet and DuckDB-WASM. Click Generate junctions.`;
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'Overture GeoParquet query failed.'; }
    finally { loading = false; }
  }

  function displayPoint(point: number[]): [number, number] {
    return detectorCrs === 'bng' ? proj4('EPSG:27700', 'EPSG:4326', point) as [number, number] : unproject(point);
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
        <button onclick={getOvertureRoads} disabled={loading}>Get Overture roads</button>
        <button onclick={generate} disabled={loading || !hasRoadData}>Generate junctions</button>
        <button class="secondary" onclick={downloadJunctions} disabled={!junctions}>Download junction GeoJSON</button>
      </div>
      <section>
        <h2>Detection parameters</h2>
        <div class="parameter-grid">
          <label>Minimum arms <input type="number" min="2" max="12" step="1" bind:value={minArms} /></label>
          <label>Buffer (m) <input type="number" min="0.1" max="50" step="0.5" bind:value={bufferM} /></label>
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
