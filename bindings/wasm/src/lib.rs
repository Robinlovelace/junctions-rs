use junctions_core::{Config, Road, find_junctions};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// Generate junction polygons from a JSON array of projected Road objects.
/// The JSON boundary keeps this adapter usable from any bundler without
/// exposing Rust layout or requiring wasm-bindgen object graphs.
#[wasm_bindgen]
pub fn generate_junctions(roads_json: &str, config_json: &str) -> Result<String, JsValue> {
    let roads: Vec<Road> = serde_json::from_str(roads_json)
        .map_err(|error| JsValue::from_str(&format!("invalid roads JSON: {error}")))?;
    let config: Config = serde_json::from_str(config_json)
        .map_err(|error| JsValue::from_str(&format!("invalid config JSON: {error}")))?;
    let junctions =
        find_junctions(&roads, config).map_err(|error| JsValue::from_str(&error.to_string()))?;
    let features: Vec<Value> = junctions
        .into_iter()
        .map(|junction| {
            json!({
                "type": "Feature",
                "properties": {
                    "junction_id": junction.id,
                    "level": junction.level,
                    "num_nodes": junction.num_nodes,
                    "num_arms": junction.num_arms,
                    "x": junction.x,
                    "y": junction.y,
                },
                "geometry": {"type": "Polygon", "coordinates": [junction.polygon]},
            })
        })
        .collect();
    serde_json::to_string(&json!({"type": "FeatureCollection", "features": features}))
        .map_err(|error| JsValue::from_str(&error.to_string()))
}
