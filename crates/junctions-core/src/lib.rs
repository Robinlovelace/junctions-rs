//! Portable, projected-coordinate road-junction detection.
//!
//! The core owns topology: endpoint/interior-crossing detection, grade-level
//! separation and deterministic clustering. CRS transformation, OSM ingestion,
//! and language-specific shapes are deliberately adapters, not core concerns.

use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{ConvexHull, Line, MultiPoint};
use geo_types::{Coord, Point, Polygon};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const EPSILON: f64 = 1e-8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Road {
    pub id: String,
    /// Projected planar coordinates in metres, with at least two positions.
    pub coordinates: Vec<[f64; 2]>,
    /// Vertical connectivity class: bridges/tunnels must use a separate level.
    #[serde(default)]
    pub level: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Merge candidate nodes nearer than this distance in projected metres.
    pub cluster_distance_m: f64,
    /// Half-width of the square used to make a deterministic junction polygon.
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
    /// Closed convex-hull ring, serialisable by all adapters.
    pub polygon: Vec<[f64; 2]>,
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
}

/// Find same-level endpoint and interior-crossing junctions in projected roads.
///
/// This is intentionally deterministic: output IDs are sorted by level, y then x.
/// Input road geometry must be in a local projected CRS in metres.
pub fn find_junctions(roads: &[Road], config: Config) -> Result<Vec<Junction>, JunctionError> {
    validate(roads, config)?;
    let mut candidates = endpoint_candidates(roads);
    if config.detect_intersections {
        candidates.extend(intersection_candidates(roads));
    }
    let mut clusters = cluster_candidates(candidates, config.cluster_distance_m);
    let mut result = Vec::new();

    for cluster in &mut clusters {
        cluster.roads.sort_unstable();
        cluster.roads.dedup();
        let arm_count = cluster.roads.len();
        if arm_count < config.min_arms {
            continue;
        }
        let center = centroid(&cluster.points);
        let polygon = hull_for_points(&cluster.points, config.buffer_m);
        result.push(Junction {
            id: String::new(),
            level: cluster.level,
            x: center.x,
            y: center.y,
            num_nodes: cluster.points.len(),
            num_arms: arm_count,
            polygon,
        });
    }
    result.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.y.total_cmp(&b.y))
            .then_with(|| a.x.total_cmp(&b.x))
    });
    for (index, junction) in result.iter_mut().enumerate() {
        junction.id = format!("j{index}");
    }
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
    }
    Ok(())
}

fn endpoint_candidates(roads: &[Road]) -> Vec<Candidate> {
    let mut out = Vec::with_capacity(roads.len() * 2);
    for (road_index, road) in roads.iter().enumerate() {
        for coordinate in [
            road.coordinates[0],
            *road.coordinates.last().expect("validated"),
        ] {
            out.push(Candidate {
                point: Coord::from(coordinate),
                level: road.level,
                roads: vec![road_index],
            });
        }
    }
    out
}

fn intersection_candidates(roads: &[Road]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (left_index, left) in roads.iter().enumerate() {
        for (right_index, right) in roads.iter().enumerate().skip(left_index + 1) {
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
                        out.push(Candidate {
                            point: intersection,
                            level: left.level,
                            roads: vec![left_index, right_index],
                        });
                    }
                }
            }
        }
    }
    out
}

#[derive(Debug)]
struct Cluster {
    level: i32,
    points: Vec<Coord<f64>>,
    roads: Vec<usize>,
}

fn cluster_candidates(mut candidates: Vec<Candidate>, distance: f64) -> Vec<Cluster> {
    candidates.sort_by(|a, b| {
        a.level
            .cmp(&b.level)
            .then_with(|| a.point.y.total_cmp(&b.point.y))
            .then_with(|| a.point.x.total_cmp(&b.point.x))
    });
    let mut clusters: Vec<Cluster> = Vec::new();
    'candidate: for candidate in candidates {
        for cluster in clusters
            .iter_mut()
            .filter(|cluster| cluster.level == candidate.level)
        {
            if cluster.points.iter().any(|point| {
                squared_distance(*point, candidate.point) <= distance * distance + EPSILON
            }) {
                cluster.points.push(candidate.point);
                cluster.roads.extend(candidate.roads);
                continue 'candidate;
            }
        }
        clusters.push(Cluster {
            level: candidate.level,
            points: vec![candidate.point],
            roads: candidate.roads,
        });
    }
    clusters
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

fn hull_for_points(points: &[Coord<f64>], buffer: f64) -> Vec<[f64; 2]> {
    let corners = points
        .iter()
        .flat_map(|point| {
            [
                Point::new(point.x - buffer, point.y - buffer),
                Point::new(point.x - buffer, point.y + buffer),
                Point::new(point.x + buffer, point.y - buffer),
                Point::new(point.x + buffer, point.y + buffer),
            ]
        })
        .collect::<Vec<_>>();
    let polygon: Polygon<f64> = MultiPoint::from(corners).convex_hull();
    polygon
        .exterior()
        .0
        .iter()
        .map(|coord| [coord.x, coord.y])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn road(id: &str, coordinates: Vec<[f64; 2]>, level: i32) -> Road {
        Road {
            id: id.into(),
            coordinates,
            level,
        }
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
        assert!(found[0].polygon.len() >= 4);
    }
}
