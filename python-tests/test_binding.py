import json
import junctions_rs


def test_python_binding_detects_junction():
    roads = [
        {"id": "a", "coordinates": [[-10, 0], [0, 0]]},
        {"id": "b", "coordinates": [[0, 0], [10, 0]]},
        {"id": "c", "coordinates": [[0, 0], [0, 10]]},
    ]
    output = json.loads(junctions_rs.find_junctions_json(json.dumps(roads)))
    assert len(output) == 1
    assert output[0]["num_arms"] == 3
