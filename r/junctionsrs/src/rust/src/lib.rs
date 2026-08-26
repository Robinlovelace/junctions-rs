use extendr_api::prelude::*;
use junctions_core::{Config, Road, find_junctions};

/// Detect junctions from a JSON array of projected road centrelines.
///
/// @param roads_json A JSON array of `{id, coordinates, node_ids, level}` roads.
/// @param buffer_m Radius of round point buffers in projected metres.
/// @param min_arms Minimum contributing-road count.
/// @param cluster_distance_m Candidate-node merge tolerance in metres.
/// @return A JSON array of deterministic junctions.
/// @export
#[extendr]
fn junctions_json(
    roads_json: String,
    buffer_m: f64,
    min_arms: i32,
    cluster_distance_m: f64,
) -> Result<String> {
    let roads: Vec<Road> =
        serde_json::from_str(&roads_json).map_err(|error| Error::Other(error.to_string()))?;
    let result = find_junctions(
        &roads,
        Config {
            buffer_m,
            min_arms: min_arms.max(0) as usize,
            cluster_distance_m,
            detect_intersections: true,
        },
    )
    .map_err(|error| Error::Other(error.to_string()))?;
    serde_json::to_string(&result).map_err(|error| Error::Other(error.to_string()))
}

extendr_module! {
    mod junctionsrs;
    fn junctions_json;
}
