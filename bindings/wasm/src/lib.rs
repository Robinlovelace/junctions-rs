use arrow_array::{Array, BinaryArray, Float64Array, Int32Array, RecordBatch, StringArray};
use arrow_ipc::reader::StreamReader;
use geo_types::Geometry;
use geozero::{ToGeo, wkb::Wkb};
use junctions_core::{Config, Road, find_junctions};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

/// Generate junction polygons from a JSON array of projected Road objects.
///
/// Kept for generic clients. The explorer uses `generate_junctions_arrow` for
/// the standard DuckDB → Arrow IPC path.
#[wasm_bindgen]
pub fn generate_junctions(roads_json: &str, config_json: &str) -> Result<String, JsValue> {
    let roads: Vec<Road> = serde_json::from_str(roads_json)
        .map_err(|error| JsValue::from_str(&format!("invalid roads JSON: {error}")))?;
    generate(roads, config_json)
}

/// Generate junction polygons from an Apache Arrow IPC stream.
///
/// The stream must provide `id` (UTF-8), `geometry` (WKB binary), and `level`
/// (Int32) columns. This is the binary hand-off used for Overture GeoParquet
/// queried by DuckDB-WASM; geometry remains WKB until it reaches Rust.
#[wasm_bindgen]
pub fn generate_junctions_arrow(arrow_ipc: &[u8], config_json: &str) -> Result<String, JsValue> {
    let roads = roads_from_arrow(arrow_ipc).map_err(|error| JsValue::from_str(&error))?;
    generate(roads, config_json)
}

fn generate(roads: Vec<Road>, config_json: &str) -> Result<String, JsValue> {
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
                    "node_ids": junction.node_ids,
                    "way_ids": junction.way_ids,
                    "x": junction.x,
                    "y": junction.y,
                },
                "geometry": {"type": "MultiPolygon", "coordinates": junction.polygons},
            })
        })
        .collect();
    serde_json::to_string(&json!({"type": "FeatureCollection", "features": features}))
        .map_err(|error| JsValue::from_str(&error.to_string()))
}

fn roads_from_arrow(arrow_ipc: &[u8]) -> Result<Vec<Road>, String> {
    let reader = StreamReader::try_new(arrow_ipc, None)
        .map_err(|error| format!("invalid Arrow IPC stream: {error}"))?;
    let mut roads = Vec::new();
    for batch in reader {
        let batch = batch.map_err(|error| format!("invalid Arrow record batch: {error}"))?;
        append_roads(&batch, &mut roads)?;
    }
    Ok(roads)
}

fn append_roads(batch: &RecordBatch, roads: &mut Vec<Road>) -> Result<(), String> {
    let ids = column::<StringArray>(batch, "id")?;
    let geometries = column::<BinaryArray>(batch, "geometry")?;
    let levels = column::<Int32Array>(batch, "level")?;
    let buffers = batch
        .column_by_name("buffer_m")
        .and_then(|c| c.as_any().downcast_ref::<Float64Array>());
    for row in 0..batch.num_rows() {
        if ids.is_null(row) || geometries.is_null(row) || levels.is_null(row) {
            return Err(format!(
                "Arrow road row {row} has a null id, geometry, or level"
            ));
        }
        let geometry = Wkb(geometries.value(row))
            .to_geo()
            .map_err(|error| format!("road `{}` has invalid WKB: {error}", ids.value(row)))?;
        let Geometry::LineString(line) = geometry else {
            return Err(format!(
                "road `{}` geometry is not a LineString",
                ids.value(row)
            ));
        };
        roads.push(Road {
            id: ids.value(row).to_owned(),
            coordinates: line.0.into_iter().map(|coord| [coord.x, coord.y]).collect(),
            node_ids: Vec::new(),
            level: levels.value(row),
            buffer_m: buffers.and_then(|b| (!b.is_null(row)).then(|| b.value(row))),
        });
    }
    Ok(())
}

fn column<'a, T: Array + 'static>(batch: &'a RecordBatch, name: &str) -> Result<&'a T, String> {
    batch
        .column_by_name(name)
        .ok_or_else(|| format!("Arrow stream is missing required `{name}` column"))?
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| format!("Arrow `{name}` column has an unexpected type"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, BinaryArray, Int32Array, StringArray};
    use arrow_ipc::writer::StreamWriter;
    use arrow_schema::{DataType, Field, Schema};
    use geo_types::LineString;
    use geozero::{CoordDimensions, ToWkb};
    use std::sync::Arc;

    #[test]
    fn decodes_arrow_ipc_wkb_lines_into_roads() {
        let geometry = Geometry::LineString(LineString::from(vec![(0.0, 0.0), (10.0, 0.0)]));
        let line = geometry.to_wkb(CoordDimensions::xy()).unwrap();
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("geometry", DataType::Binary, false),
            Field::new("level", DataType::Int32, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["segment-1"])) as ArrayRef,
                Arc::new(BinaryArray::from(vec![line.as_slice()])) as ArrayRef,
                Arc::new(Int32Array::from(vec![0])) as ArrayRef,
            ],
        )
        .unwrap();
        let mut bytes = Vec::new();
        let mut writer = StreamWriter::try_new(&mut bytes, &schema).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();

        let roads = roads_from_arrow(&bytes).unwrap();
        assert_eq!(roads.len(), 1);
        assert_eq!(roads[0].id, "segment-1");
        assert_eq!(roads[0].coordinates, vec![[0.0, 0.0], [10.0, 0.0]]);
    }
}
