use anyhow::{Context, Result, bail};
use clap::Parser;
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use junctions_core::{Config, Road, find_junctions};
use serde_json::{Map, Value as JsonValue, json};
use std::{fs, path::PathBuf};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Find projected road-centreline junctions from GeoJSON"
)]
struct Args {
    /// Input GeoJSON FeatureCollection of LineStrings in a projected CRS (metres).
    input: PathBuf,
    /// Destination GeoJSON FeatureCollection.
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long, default_value_t = 5.0)]
    buffer_m: f64,
    #[arg(long, default_value_t = 3)]
    min_arms: usize,
    #[arg(long, default_value_t = 0.01)]
    cluster_distance_m: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let collection = match text.parse::<GeoJson>()? {
        GeoJson::FeatureCollection(c) => c,
        _ => bail!("input must be a GeoJSON FeatureCollection"),
    };
    let roads = collection
        .features
        .iter()
        .enumerate()
        .map(feature_to_road)
        .collect::<Result<Vec<_>>>()?;
    let config = Config {
        buffer_m: args.buffer_m,
        min_arms: args.min_arms,
        cluster_distance_m: args.cluster_distance_m,
        detect_intersections: true,
    };
    let features = find_junctions(&roads, config)?
        .into_iter()
        .map(|junction| {
            let mut properties = Map::new();
            properties.insert("junction_id".to_string(), json!(junction.id));
            properties.insert("level".to_string(), json!(junction.level));
            properties.insert("num_nodes".to_string(), json!(junction.num_nodes));
            properties.insert("num_arms".to_string(), json!(junction.num_arms));
            Feature {
                bbox: None,
                id: None,
                foreign_members: None,
                properties: Some(properties),
                geometry: Some(Geometry::new(Value::Polygon(vec![
                    junction
                        .polygon
                        .into_iter()
                        .map(|point| point.to_vec())
                        .collect(),
                ]))),
            }
        })
        .collect();
    let output = GeoJson::FeatureCollection(FeatureCollection {
        bbox: None,
        foreign_members: None,
        features,
    });
    fs::write(&args.output, output.to_string())
        .with_context(|| format!("writing {}", args.output.display()))?;
    Ok(())
}

fn feature_to_road((index, feature): (usize, &Feature)) -> Result<Road> {
    let geometry = feature
        .geometry
        .as_ref()
        .context("feature has no geometry")?;
    let coordinates = match &geometry.value {
        Value::LineString(coords) => coords
            .iter()
            .map(|point| {
                if point.len() < 2 {
                    bail!("LineString coordinate has fewer than two ordinates");
                }
                Ok([point[0], point[1]])
            })
            .collect::<Result<Vec<_>>>()?,
        _ => bail!("feature {} is not a LineString", index),
    };
    let props = feature.properties.as_ref();
    let id = props
        .and_then(|p| p.get("id").or_else(|| p.get("feature_id")))
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("road-{index}"));
    let level = props
        .and_then(|p| p.get("level"))
        .and_then(JsonValue::as_i64)
        .unwrap_or(0) as i32;
    Ok(Road {
        id,
        coordinates,
        level,
    })
}
