use junctions_core::{Config, Road, find_junctions};
use pyo3::prelude::*;

/// Find junctions from projected road coordinates.
///
/// `roads` is a list of dictionaries with `id`, `coordinates` ([[x, y], ...])
/// and optional integer `level`. Results are dictionaries serialisable as JSON.
#[pyfunction]
#[pyo3(signature = (roads_json, buffer_m=5.0, min_arms=3, cluster_distance_m=0.01))]
fn find_junctions_json(
    roads_json: &str,
    buffer_m: f64,
    min_arms: usize,
    cluster_distance_m: f64,
) -> PyResult<String> {
    let roads: Vec<Road> = serde_json::from_str(roads_json)
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    let output = find_junctions(
        &roads,
        Config {
            buffer_m,
            min_arms,
            cluster_distance_m,
            detect_intersections: true,
        },
    )
    .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.to_string()))?;
    serde_json::to_string(&output)
        .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))
}

#[pymodule]
fn junctions_rs(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(find_junctions_json, module)?)?;
    Ok(())
}
