use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use geojson::{Feature, FeatureCollection, GeoJson, Geometry, Value};
use junctions_core::{Config, Road, find_junctions};
use serde_json::{Map, Value as JsonValue, json};
use std::io::{self, Read, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum IntersectionMode {
    Enabled,
    Disabled,
}

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Find projected road-centreline junctions from GeoJSON"
)]
struct Args {
    /// Input GeoJSON file, or - for stdin.
    #[arg(default_value = "-")]
    input: PathBuf,
    /// Destination GeoJSON file, or - for stdout.
    #[arg(short, long, default_value = "-")]
    output: PathBuf,
    #[arg(long, default_value_t = 5.0)]
    buffer_m: f64,
    #[arg(long, default_value_t = 3)]
    min_arms: usize,
    #[arg(long, default_value_t = 0.01)]
    cluster_distance_m: f64,
    /// Whether to detect same-level interior crossings as well as endpoints.
    #[arg(long, value_enum, default_value_t = IntersectionMode::Enabled)]
    intersections: IntersectionMode,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let text = read_input(&args.input)?;
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
    let features = find_junctions(
        &roads,
        Config {
            buffer_m: args.buffer_m,
            min_arms: args.min_arms,
            cluster_distance_m: args.cluster_distance_m,
            detect_intersections: matches!(args.intersections, IntersectionMode::Enabled),
        },
    )?
    .into_iter()
    .map(junction_feature)
    .collect();
    let output = GeoJson::FeatureCollection(FeatureCollection {
        bbox: None,
        foreign_members: None,
        features,
    })
    .to_string();
    write_output(&args.output, output.as_bytes())
}

fn read_input(path: &PathBuf) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .context("reading stdin")?;
        Ok(text)
    } else {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    }
}

fn write_output(path: &PathBuf, bytes: &[u8]) -> Result<()> {
    if path.as_os_str() == "-" {
        io::stdout().write_all(bytes).context("writing stdout")?;
        io::stdout().write_all(b"\n").context("writing stdout")?;
        Ok(())
    } else {
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }
}

fn junction_feature(junction: junctions_core::Junction) -> Feature {
    let mut properties = Map::new();
    properties.insert("junction_id".into(), json!(junction.id));
    properties.insert("level".into(), json!(junction.level));
    properties.insert("num_nodes".into(), json!(junction.num_nodes));
    properties.insert("num_arms".into(), json!(junction.num_arms));
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
        _ => bail!("feature {index} is not a LineString"),
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
