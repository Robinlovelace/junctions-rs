<script lang="ts">
  import { onMount } from 'svelte';
  import * as maplibregl from 'maplibre-gl';
  import ChevronDown from '@lucide/svelte/icons/chevron-down';
  import ChevronUp from '@lucide/svelte/icons/chevron-up';
  import SlidersHorizontal from '@lucide/svelte/icons/sliders-horizontal';
  import { generate_junctions } from './lib/wasm/junctions_wasm';

  type Element = { type: string; id: number; geometry?: { lat: number; lon: number }[]; tags?: Record<string, string> };
  type OsmResponse = { elements: Element[] };
  type Road = { id: string; coordinates: [number, number][]; level: number };
  type Origin = { lon: number; lat: number };

  let mapContainer: HTMLElement;
  let map = $state<maplibregl.Map | null>(null);
  let origin = $state<Origin>({ lon: -1.555, lat: 53.8067 });
  let osm = $state<OsmResponse | null>(null);
  let junctions = $state<GeoJSON.FeatureCollection | null>(null);
  let status = $state('Pan or zoom to an area, then download OSM data.');
  let error = $state('');
  let loading = $state(false);
  let minArms = $state(3);
  let bufferM = $state(5);
  let clusterDistanceM = $state(0.01);
  let detectIntersections = $state(true);
  let panelOpen = $state(true);

  const mapStyle = 'https://tiles.openfreemap.org/styles/bright';
  const overpass = 'https://overpass-api.de/api/interpreter';

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

  function getRoads(): Road[] {
    return (osm?.elements ?? []).filter((e) => e.type === 'way' && e.geometry && e.geometry.length >= 2).map((e) => ({
      id: String(e.id),
      coordinates: e.geometry!.map((p) => project([p.lon, p.lat])),
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
    return { type: 'FeatureCollection', features: getRoads().map((road) => ({ type: 'Feature', properties: { id: road.id, level: road.level }, geometry: { type: 'LineString', coordinates: road.coordinates } })) };
  }

  function setSource(id: string, data: GeoJSON.GeoJSON) {
    const source = map?.getSource(id) as maplibregl.GeoJSONSource | undefined;
    source?.setData(data);
  }

  async function downloadOsm() {
    if (!map) return;
    const zoom = map.getZoom();
    if (zoom < 12) { error = 'Zoom in to level 12 or closer before downloading (keeps the public API request small).'; return; }
    loading = true; error = ''; junctions = null; status = 'Downloading OSM data from Overpass…';
    try {
      const bounds = map.getBounds();
      const [west, south] = [bounds.getWest(), bounds.getSouth()];
      const [east, north] = [bounds.getEast(), bounds.getNorth()];
      origin = { lon: (west + east) / 2, lat: (south + north) / 2 };
      const query = `[out:json][timeout:60];way[highway](${south},${west},${north},${east});out geom;`;
      const response = await fetch(`${overpass}?data=${encodeURIComponent(query)}`);
      if (!response.ok) throw new Error(`Overpass returned HTTP ${response.status}`);
      osm = await response.json() as OsmResponse;
      setSource('roads', roadsGeoJson());
      status = `Downloaded ${getRoads().length.toLocaleString()} road ways. Click Generate junctions.`;
      const blob = new Blob([JSON.stringify(osm, null, 2)], { type: 'application/json' });
      const link = document.createElement('a'); link.href = URL.createObjectURL(blob); link.download = 'osm-data-overpass.json'; link.click(); URL.revokeObjectURL(link.href);
    } catch (reason) { error = reason instanceof Error ? reason.message : String(reason); status = 'Download failed.'; }
    finally { loading = false; }
  }

  async function generate() {
    if (!osm) { error = 'Download OSM data first.'; return; }
    loading = true; error = ''; status = 'Running junction detection in WebAssembly…';
    try {
      const config = { buffer_m: bufferM, min_arms: minArms, cluster_distance_m: clusterDistanceM, detect_intersections: detectIntersections };
      const projected = JSON.parse(generate_junctions(JSON.stringify(getRoads()), JSON.stringify(config))) as GeoJSON.FeatureCollection;
      projected.features.forEach((feature) => {
        if (feature.geometry?.type === 'Polygon') feature.geometry.coordinates = feature.geometry.coordinates.map((ring) => ring.map((point) => unproject(point)));
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
      <p class="intro">Browse an area, download its OpenStreetMap road data, then detect junctions locally in your browser.</p>
      <div class="actions">
        <button class="primary" onclick={downloadOsm} disabled={loading}>{loading ? 'Working…' : 'Download OSM for current view'}</button>
        <button onclick={generate} disabled={loading || !osm}>Generate junctions</button>
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
      <p class="attribution">Basemap © OpenFreeMap contributors · Data © <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a> contributors · Overpass API</p>
  </aside>
  <button class:show-panel-hidden={panelOpen} class="show-panel" type="button" onclick={showPanel} aria-expanded={panelOpen} aria-controls="control-panel" aria-label="Show controls" title="Show controls">
      <SlidersHorizontal size={21} strokeWidth={2.5} aria-hidden="true" />
      <span>Controls</span>
      <ChevronUp size={18} strokeWidth={2.5} aria-hidden="true" />
  </button>
  <main bind:this={mapContainer} class="map"></main>
</div>
