//! Portable, projected-coordinate road-junction detection.
//!
//! The core owns topology: endpoint/interior-crossing detection, grade-level
//! separation and deterministic clustering. CRS transformation, OSM ingestion,
//! and language-specific shapes are deliberately adapters, not core concerns.

use geo::algorithm::bool_ops::unary_union;
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{Buffer, Intersects, Line};
use geo_types::{Coord, MultiPolygon, Point, Polygon};
use rstar::{AABB, RTree, RTreeObject};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const EPSILON: f64 = 1e-8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Road {
    pub id: String,
    /// Projected planar coordinates in metres, with at least two positions.
    pub coordinates: Vec<[f64; 2]>,
    /// Optional source-node IDs aligned with `coordinates`. OSM ways provide
    /// these directly; generic line sources may omit them.
    #[serde(default)]
    pub node_ids: Vec<String>,
    /// Vertical connectivity class: bridges/tunnels must use a separate level.
    #[serde(default)]
    pub level: i32,
    /// Optional per-road buffer radius in metres. Falls back to
    /// `Config::buffer_m`. Junctions merge when their buffers overlap, so
    /// roads with larger radii (motorways) merge over longer distances.
    #[serde(default)]
    pub buffer_m: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Merge candidate nodes nearer than this distance in projected metres.
    pub cluster_distance_m: f64,
    /// Radius of round point buffers used to make junction polygons.
    pub buffer_m: f64,
    /// Ignore clusters with fewer contributing road arms.
    pub min_arms: usize,
    /// Detect geometric intersections that fall in road interiors. Disable for
    /// endpoint-only sources or to match legacy endpoint-based workflows.
    #[serde(default = "default_true")]
    pub detect_intersections: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cluster_distance_m: 0.01,
            buffer_m: 5.0,
            min_arms: 3,
            detect_intersections: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Junction {
    pub id: String,
    pub level: i32,
    pub x: f64,
    pub y: f64,
    pub num_nodes: usize,
    pub num_arms: usize,
    /// Canonical IDs of source nodes merged into this junction, when supplied.
    pub node_ids: Vec<String>,
    /// Canonical IDs of source ways contributing an arm to this junction.
    pub way_ids: Vec<String>,
    /// GeoJSON MultiPolygon coordinates: polygons → rings → positions.
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
}

#[derive(Debug, Error, PartialEq)]
pub enum JunctionError {
    #[error("road `{id}` has fewer than two coordinates")]
    TooFewCoordinates { id: String },
    #[error("road `{id}` has a non-finite coordinate")]
    NonFiniteCoordinate { id: String },
    #[error("configuration values must be finite and non-negative")]
    InvalidConfig,
}

#[derive(Debug, Clone)]
struct Candidate {
    point: Coord<f64>,
    level: i32,
    roads: Vec<usize>,
    node_ids: Vec<String>,
    buffer_m: f64,
}

/// Find same-level junctions in projected roads.
///
/// Junctions are the connected components of the union of per-node circular
/// buffers: every candidate node (road endpoint or interior crossing) with at
/// least `min_arms` incident road ends is buffered at its road's radius, all
/// buffers on one level are dissolved, and each connected component becomes
/// one junction polygon (its convex hull). This matches the GEOS
/// `ST_Union_Agg(ST_Buffer(...))` semantics of `junctions_cluster`, so nearby
/// junctions whose buffers overlap merge into a single junction system.
///
/// This is intentionally deterministic: output IDs are sorted by level, y then
/// x. Input road geometry must be in a local projected CRS in metres.
pub fn find_junctions(roads: &[Road], config: Config) -> Result<Vec<Junction>, JunctionError> {
    validate(roads, config)?;
    let mut candidates = endpoint_candidates(roads, config.buffer_m);
    if config.detect_intersections {
        candidates.extend(intersection_candidates(roads, config.buffer_m));
    }
    let positions = snap_positions(candidates, config.cluster_distance_m);

    let mut result = Vec::new();
    let mut levels: Vec<i32> = positions.iter().map(|p| p.level).collect();
    levels.sort_unstable();
    levels.dedup();

    for level in levels {
        let qualifying: Vec<&Position> = positions
            .iter()
            .filter(|p| p.level == level && p.incidence >= config.min_arms)
            .collect();
        if qualifying.is_empty() {
            continue;
        }
        let buffers: Vec<MultiPolygon<f64>> = qualifying
            .iter()
            .copied()
            .map(|p| Point::new(p.point.x, p.point.y).buffer(p.buffer_m))
            .collect();
        let dissolved = unary_union(buffers.iter());
        // geo's i_overlay union can return overlapping parts when many
        // circles nearly touch (observed with ~180 overlapping buffers).
        // Merge any intersecting parts to a fixpoint so components are
        // guaranteed disjoint — a junction system never overlaps another.
        let mut parts = dissolved.0;
        let mut passes = 0;
        loop {
            passes += 1;
            if passes > parts.len() + 1 {
                break;
            }
            let mut merged_any = false;
            'outer: for i in 0..parts.len() {
                for j in (i + 1)..parts.len() {
                    if parts[i].intersects(&parts[j]) {
                        let pair = unary_union([&parts[i], &parts[j]]);
                        parts[i] = pair.0[0].clone();
                        parts.extend(pair.0.into_iter().skip(1));
                        parts.swap_remove(j);
                        merged_any = true;
                        break 'outer;
                    }
                }
            }
            if !merged_any {
                break;
            }
        }
        for component in &parts {
            let members: Vec<&Position> = qualifying
                .iter()
                .copied()
                .filter(|p| component.intersects(&Point::new(p.point.x, p.point.y)))
                .collect();
            if members.is_empty() {
                continue;
            }
            let mut road_indices = members
                .iter()
                .flat_map(|p| p.roads.iter().copied())
                .collect::<Vec<_>>();
            road_indices.sort_unstable();
            road_indices.dedup();
            if road_indices.len() < config.min_arms {
                continue;
            }
            let mut node_ids = members
                .iter()
                .flat_map(|p| p.node_ids.iter().cloned())
                .collect::<Vec<_>>();
            node_ids.sort();
            node_ids.dedup();
            let points = members.iter().map(|p| p.point).collect::<Vec<_>>();
            let mut way_ids = road_indices
                .iter()
                .map(|index| roads[*index].id.clone())
                .collect::<Vec<_>>();
            way_ids.sort();
            way_ids.dedup();
            // The junction polygon is the dissolved buffer component itself:
            // circular buffers union only where they touch, so disjoint
            // junction systems can never overlap. (The counterflow/DuckDB
            // references take a convex hull here instead, which inflates each
            // component and lets nearby hulls overlap.)
            let polygons = vec![polygon_coordinates(component)];
            result.push(Junction {
                id: stable_junction_id(level, &node_ids, &way_ids, &points),
                level,
                x: centroid(&points).x,
                y: centroid(&points).y,
                num_nodes: points.len(),
                num_arms: road_indices.len(),
                node_ids,
                way_ids,
                polygons,
            });
        }
    }
    result.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.y.total_cmp(&b.y))
            .then_with(|| a.x.total_cmp(&b.x))
    });
    Ok(result)
}

fn validate(roads: &[Road], config: Config) -> Result<(), JunctionError> {
    if !config.cluster_distance_m.is_finite()
        || !config.buffer_m.is_finite()
        || config.cluster_distance_m < 0.0
        || config.buffer_m < 0.0
    {
        return Err(JunctionError::InvalidConfig);
    }
    for road in roads {
        if road.coordinates.len() < 2 {
            return Err(JunctionError::TooFewCoordinates {
                id: road.id.clone(),
            });
        }
        if road.coordinates.iter().flatten().any(|x| !x.is_finite()) {
            return Err(JunctionError::NonFiniteCoordinate {
                id: road.id.clone(),
            });
        }
        if let Some(buffer) = road.buffer_m {
            if !buffer.is_finite() || buffer < 0.0 {
                return Err(JunctionError::InvalidConfig);
            }
        }
    }
    Ok(())
}

fn endpoint_candidates(roads: &[Road], default_buffer: f64) -> Vec<Candidate> {
    let mut out = Vec::with_capacity(roads.len() * 2);
    for (road_index, road) in roads.iter().enumerate() {
        for coordinate_index in [0, road.coordinates.len() - 1] {
            out.push(Candidate {
                point: Coord::from(road.coordinates[coordinate_index]),
                level: road.level,
                roads: vec![road_index],
                node_ids: road
                    .node_ids
                    .get(coordinate_index)
                    .filter(|id| !id.is_empty())
                    .cloned()
                    .into_iter()
                    .collect(),
                buffer_m: road.buffer_m.unwrap_or(default_buffer),
            });
        }
    }
    out
}

/// Bounding box of one road, used as the R-tree key for pair pruning.
struct RoadEntry {
    index: usize,
    envelope: AABB<[f64; 2]>,
}

impl RTreeObject for RoadEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

fn road_envelope(coordinates: &[[f64; 2]]) -> AABB<[f64; 2]> {
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for coordinate in coordinates {
        min[0] = min[0].min(coordinate[0]);
        min[1] = min[1].min(coordinate[1]);
        max[0] = max[0].max(coordinate[0]);
        max[1] = max[1].max(coordinate[1]);
    }
    AABB::from_corners(min, max)
}

fn intersection_candidates(roads: &[Road], default_buffer: f64) -> Vec<Candidate> {
    let entries: Vec<RoadEntry> = roads
        .iter()
        .enumerate()
        .map(|(index, road)| RoadEntry {
            index,
            envelope: road_envelope(&road.coordinates),
        })
        .collect();
    let tree: RTree<RoadEntry> = RTree::bulk_load(entries);

    let mut out = Vec::new();
    for (left_index, left) in roads.iter().enumerate() {
        // Only visit each unordered pair once: neighbours with a strictly
        // greater index (ties impossible — envelopes are unique per road).
        for neighbour in tree.locate_in_envelope_intersecting(&road_envelope(&left.coordinates)) {
            let right_index = neighbour.index;
            if right_index <= left_index {
                continue;
            }
            let right = &roads[right_index];
            if left.level != right.level {
                continue;
            }
            for left_segment in left.coordinates.windows(2) {
                let left_line =
                    Line::new(Coord::from(left_segment[0]), Coord::from(left_segment[1]));
                for right_segment in right.coordinates.windows(2) {
                    let right_line =
                        Line::new(Coord::from(right_segment[0]), Coord::from(right_segment[1]));
                    if let Some(LineIntersection::SinglePoint { intersection, .. }) =
                        line_intersection(left_line, right_line)
                    {
                        let left_buffer = left.buffer_m.unwrap_or(default_buffer);
                        let right_buffer = right.buffer_m.unwrap_or(default_buffer);
                        out.push(Candidate {
                            point: intersection,
                            level: left.level,
                            roads: vec![left_index, right_index],
                            node_ids: Vec::new(),
                            buffer_m: left_buffer.min(right_buffer),
                        });
                    }
                }
            }
        }
    }
    out
}

#[derive(Debug)]
struct Position {
    level: i32,
    point: Coord<f64>,
    roads: Vec<usize>,
    node_ids: Vec<String>,
    /// Number of road-end incidences at this position (endpoints count one
    /// each; an interior crossing counts the number of crossing roads).
    incidence: usize,
    /// Buffer radius in metres for this position: the minimum over its roads.
    buffer_m: f64,
}

/// R-tree key over every accepted position, so a new candidate can find its
/// snapped position without scanning all positions.
struct PositionEntry {
    position_index: usize,
    level: i32,
    point: Coord<f64>,
}

impl RTreeObject for PositionEntry {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_corners([self.point.x, self.point.y], [self.point.x, self.point.y])
    }
}

/// Group candidates by snapped position: candidates closer than `distance`
/// (default 1 cm) at the same level share one position. Each position keeps
/// the union of its roads/node ids, the summed incidence, and the minimum
/// buffer radius of its roads (matching the reference implementations, which
/// buffer a node at the minimum class radius of its incident links).
fn snap_positions(mut candidates: Vec<Candidate>, distance: f64) -> Vec<Position> {
    candidates.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.point.y.total_cmp(&b.point.y))
            .then_with(|| a.point.x.total_cmp(&b.point.x))
    });
    let mut positions: Vec<Position> = Vec::new();
    let mut entries: RTree<PositionEntry> = RTree::new();
    let tolerance = distance * distance + EPSILON;
    'candidate: for candidate in candidates {
        let query = AABB::from_corners(
            [candidate.point.x - distance, candidate.point.y - distance],
            [candidate.point.x + distance, candidate.point.y + distance],
        );
        // Collect before mutating: rstar iterators borrow the tree.
        let mut target: Option<usize> = None;
        for entry in entries.locate_in_envelope_intersecting(&query) {
            if entry.level != candidate.level {
                continue;
            }
            if squared_distance(entry.point, candidate.point) > tolerance {
                continue;
            }
            target = Some(entry.position_index);
            break;
        }
        if let Some(position_index) = target {
            let position = &mut positions[position_index];
            position.point = midpoint(position.point, candidate.point);
            position.incidence += candidate.roads.len();
            position.roads.extend(candidate.roads);
            position.node_ids.extend(candidate.node_ids);
            position.buffer_m = position.buffer_m.min(candidate.buffer_m);
            entries.insert(PositionEntry {
                position_index,
                level: candidate.level,
                point: position.point,
            });
            continue 'candidate;
        }
        let position_index = positions.len();
        positions.push(Position {
            level: candidate.level,
            point: candidate.point,
            incidence: candidate.roads.len(),
            roads: candidate.roads,
            node_ids: candidate.node_ids,
            buffer_m: candidate.buffer_m,
        });
        entries.insert(PositionEntry {
            position_index,
            level: candidate.level,
            point: candidate.point,
        });
    }
    positions
}

fn midpoint(a: Coord<f64>, b: Coord<f64>) -> Coord<f64> {
    Coord {
        x: (a.x + b.x) / 2.0,
        y: (a.y + b.y) / 2.0,
    }
}

fn squared_distance(a: Coord<f64>, b: Coord<f64>) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
}

fn centroid(points: &[Coord<f64>]) -> Coord<f64> {
    let count = points.len() as f64;
    Coord {
        x: points.iter().map(|p| p.x).sum::<f64>() / count,
        y: points.iter().map(|p| p.y).sum::<f64>() / count,
    }
}

fn polygon_coordinates(polygon: &Polygon<f64>) -> Vec<Vec<[f64; 2]>> {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|ring| ring.0.iter().map(|coord| [coord.x, coord.y]).collect())
        .collect()
}

fn stable_junction_id(
    level: i32,
    node_ids: &[String],
    way_ids: &[String],
    points: &[Coord<f64>],
) -> String {
    let mut identity = format!(
        "junction:level={level}:nodes={}:ways={}",
        canonical_id_list(node_ids),
        canonical_id_list(way_ids)
    );
    if node_ids.is_empty() {
        identity.push_str(&format!(";points={}", canonical_point_list(points)));
    }
    identity
}

fn canonical_id_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("{}:{id}", id.len()))
        .collect::<Vec<_>>()
        .join("|")
}

fn canonical_point_list(points: &[Coord<f64>]) -> String {
    let mut encoded = points
        .iter()
        .map(|point| format!("{:016x}:{:016x}", point.x.to_bits(), point.y.to_bits()))
        .collect::<Vec<_>>();
    encoded.sort();
    encoded.join("|")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn road(id: &str, coordinates: Vec<[f64; 2]>, level: i32) -> Road {
        Road {
            id: id.into(),
            coordinates,
            node_ids: Vec::new(),
            level,
            buffer_m: None,
        }
    }

    #[test]
    fn merges_overlapping_buffer_junctions_into_one() {
        // Two 2-arm junctions 8 m apart: 5 m buffers overlap, so the dissolved
        // union produces ONE junction polygon spanning both nodes.
        let roads = vec![
            road("a", vec![[-10., 0.], [0., 0.]], 0),
            road("b", vec![[0., 0.], [10., 0.]], 0),
            road("c", vec![[8., -10.], [8., 0.]], 0),
            road("d", vec![[8., 0.], [8., 10.]], 0),
        ];
        let found = find_junctions(
            &roads,
            Config {
                min_arms: 2,
                buffer_m: 5.0,
                ..Config::default()
            },
        )
        .unwrap();
        assert_eq!(
            found.len(),
            1,
            "overlapping buffers must dissolve into one junction"
        );
        assert_eq!(found[0].num_arms, 4);
        assert_eq!(found[0].num_nodes, 2);
        let ring = &found[0].polygons[0][0];
        assert!(
            ring.len() > 8,
            "a round buffer needs more than square corners"
        );
        assert!(ring.iter().any(|point| point[0] < -4.9));
        assert!(ring.iter().any(|point| point[0] > 12.9));
    }

    #[test]
    fn keeps_disjoint_buffer_junctions_separate() {
        // Same as above but 11 m apart: 5 m buffers do not overlap, so two
        // separate junction polygons result.
        let roads = vec![
            road("a", vec![[-10., 0.], [0., 0.]], 0),
            road("b", vec![[0., 0.], [10., 0.]], 0),
            road("c", vec![[11., -10.], [11., 0.]], 0),
            road("d", vec![[11., 0.], [11., 10.]], 0),
        ];
        let found = find_junctions(
            &roads,
            Config {
                min_arms: 2,
                buffer_m: 5.0,
                ..Config::default()
            },
        )
        .unwrap();
        assert_eq!(found.len(), 2, "separate circles must not be bridged");
    }

    #[test]
    fn respects_per_road_buffer_radii() {
        // A motorway (20 m) and a minor road (5 m) meet: the node buffers at
        // the minimum class radius (5 m), matching junctions_cluster's
        // min(CASE ...) semantics, so the junction hull stays small.
        let mut motorway = road("m", vec![[-10., 0.], [0., 0.]], 0);
        motorway.buffer_m = Some(20.0);
        let roads = vec![
            motorway,
            road("minor-a", vec![[0., 0.], [10., 0.]], 0),
            road("minor-b", vec![[0., 0.], [0., 10.]], 0),
        ];
        let found = find_junctions(&roads, Config::default()).unwrap();
        assert_eq!(found.len(), 1);
        let ring = &found[0].polygons[0][0];
        let extent = ring
            .iter()
            .map(|point| (point[0].powi(2) + point[1].powi(2)).sqrt())
            .fold(0.0_f64, f64::max);
        assert!(
            extent < 5.6,
            "junction must buffer at the minimum radius, got {extent:.2} m"
        );
    }

    #[test]
    fn uses_canonical_source_nodes_and_ways_for_stable_ids() {
        let mut roads = vec![
            road("way-z", vec![[-10., 0.], [0., 0.]], 0),
            road("way-a", vec![[0., 0.], [10., 0.]], 0),
            road("way-m", vec![[0., 0.], [0., 10.]], 0),
        ];
        roads[0].node_ids = vec!["node-west".into(), "node-centre".into()];
        roads[1].node_ids = vec!["node-centre".into(), "node-east".into()];
        roads[2].node_ids = vec!["node-centre".into(), "node-north".into()];

        let found = find_junctions(&roads, Config::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].node_ids, vec!["node-centre"]);
        assert_eq!(found[0].way_ids, vec!["way-a", "way-m", "way-z"]);
        assert!(found[0].id.contains("node-centre"));
        assert!(found[0].id.contains("way-a"));

        roads.reverse();
        let reordered = find_junctions(&roads, Config::default()).unwrap();
        assert_eq!(found[0].id, reordered[0].id);
    }

    #[test]
    fn detects_interior_crossing_with_four_arms() {
        let roads = vec![
            road("h", vec![[-10., 0.], [10., 0.]], 0),
            road("v", vec![[0., -10.], [0., 10.]], 0),
        ];
        let found = find_junctions(
            &roads,
            Config {
                min_arms: 2,
                ..Config::default()
            },
        )
        .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].num_arms, 2);
        assert_eq!(found[0].num_nodes, 1);
    }

    #[test]
    fn distinguishes_node_less_junctions_by_exact_candidate_points() {
        let ways = vec!["h".into(), "v".into()];
        let first = stable_junction_id(0, &[], &ways, &[Coord { x: 0.0, y: 0.0 }]);
        let second = stable_junction_id(
            0,
            &[],
            &ways,
            &[Coord {
                x: 0.000_001,
                y: 0.0,
            }],
        );
        assert_ne!(first, second);
    }

    #[test]
    fn keeps_grade_separated_crossings_apart() {
        let roads = vec![
            road("ground", vec![[-10., 0.], [10., 0.]], 0),
            road("bridge", vec![[0., -10.], [0., 10.]], 1),
        ];
        assert!(
            find_junctions(&roads, Config::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn clusters_shared_endpoints_and_counts_arms() {
        let roads = vec![
            road("a", vec![[-10., 0.], [0., 0.]], 0),
            road("b", vec![[0., 0.], [10., 0.]], 0),
            road("c", vec![[0., 0.], [0., 10.]], 0),
        ];
        let found = find_junctions(&roads, Config::default()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].num_arms, 3);
        assert_eq!(found[0].polygons.len(), 1);
        assert!(found[0].polygons[0][0].len() > 8);
    }
}
