/**
 * Junction buffer radii by road class.
 *
 * The "Counterflow" preset mirrors the per-class radii used by the counterflow
 * OpenRoads ingest (ingest/openroads.py), translated from OS Open Roads road
 * functions to OSM highway tags:
 *
 *   Motorway 20 m  <- motorway, motorway_link
 *   A Road   15 m  <- trunk, trunk_link, primary, primary_link
 *   B Road   10 m  <- secondary, secondary_link, tertiary, tertiary_link
 *   Minor    10 m  <- unclassified, residential, living_street, service
 *   else      5 m  <- footway, cycleway, path, pedestrian, steps, track, ...
 *
 * The core buffers each node at the minimum radius of its incident roads
 * (the same min(CASE ...) rule the counterflow ingest applies).
 */

export type BufferPreset = {
  name: string;
  /** Radius for highway classes listed here, in metres. */
  radii: Record<string, number>;
  /** Radius for every other highway tag, in metres. */
  fallback: number;
};

export const BUFFER_PRESETS: BufferPreset[] = [
  {
    name: 'Counterflow (OpenRoads classes → OSM)',
    fallback: 5,
    radii: {
      motorway: 20,
      motorway_link: 20,
      trunk: 15,
      trunk_link: 15,
      primary: 15,
      primary_link: 15,
      secondary: 10,
      secondary_link: 10,
      tertiary: 10,
      tertiary_link: 10,
      unclassified: 10,
      residential: 10,
      living_street: 10,
      service: 10,
    },
  },
  { name: 'Uniform 5 m', fallback: 5, radii: {} },
  { name: 'Uniform 10 m', fallback: 10, radii: {} },
  { name: 'Uniform 20 m', fallback: 20, radii: {} },
];

/** Buffer radius (metres) for an OSM way's tags under a preset. */
export function bufferForTags(preset: BufferPreset, tags: Record<string, string>): number {
  const highway = tags.highway ?? '';
  return preset.radii[highway] ?? preset.fallback;
}
