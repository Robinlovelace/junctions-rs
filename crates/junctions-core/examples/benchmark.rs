use junctions_core::{Config, Road, find_junctions};
use std::time::Instant;

fn star(size: usize) -> Vec<Road> {
    (0..size)
        .map(|index| {
            let angle = (index as f64) * std::f64::consts::TAU / size as f64;
            Road {
                id: format!("r{index}"),
                coordinates: vec![[angle.cos() * 100.0, angle.sin() * 100.0], [0.0, 0.0]],
                level: 0,
            }
        })
        .collect()
}

fn main() {
    let roads = star(200); // endpoint-only topology, equivalent to junctions_cluster's node model.
    let config = Config {
        min_arms: 2,
        detect_intersections: false,
        ..Config::default()
    };
    let started = Instant::now();
    let output = find_junctions(&roads, config).expect("valid synthetic roads");
    println!(
        "roads={},junctions={},elapsed_ms={:.3}",
        roads.len(),
        output.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
}
