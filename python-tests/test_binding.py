import json
import junctions_rs


def test_python_binding_detects_junction():
    roads = [
        {"id": "a", "coordinates": [[-10, 0], [0, 0]], "node_ids": ["west", "centre"]},
        {"id": "b", "coordinates": [[0, 0], [10, 0]], "node_ids": ["centre", "east"]},
        {"id": "c", "coordinates": [[0, 0], [0, 10]], "node_ids": ["centre", "north"]},
    ]
    output = json.loads(junctions_rs.find_junctions_json(json.dumps(roads)))
    assert len(output) == 1
    assert output[0]["num_arms"] == 3
    assert output[0]["node_ids"] == ["centre"]
    assert output[0]["way_ids"] == ["a", "b", "c"]
    assert len(output[0]["polygons"]) == 1
