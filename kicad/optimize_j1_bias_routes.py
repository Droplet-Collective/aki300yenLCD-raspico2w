"""Shorten the routed J1 +/-13 V returns without changing placement or width."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import pcbnew


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--input", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--report", type=Path)
args = parser.parse_args()

board = pcbnew.LoadBoard(str(args.input))
tracks = list(board.GetTracks())


def point(position):
    return pcbnew.VECTOR2I(pcbnew.FromMM(position[0]), pcbnew.FromMM(position[1]))


def mm(value):
    return round(pcbnew.ToMM(value), 3)


def same(first, second):
    return all(abs(a - b) < 0.001 for a, b in zip(first, second))


def endpoints(item):
    return (
        [mm(item.GetStart().x), mm(item.GetStart().y)],
        [mm(item.GetEnd().x), mm(item.GetEnd().y)],
    )


def route_length(positions):
    return round(sum(math.dist(start, end) for start, end in zip(positions, positions[1:])), 3)


removed = []


def remove_segment(net_name, layer, start, end):
    matches = []
    for item in tracks:
        if item.Type() != pcbnew.PCB_TRACE_T:
            continue
        if item.GetNetname() != net_name or item.GetLayer() != layer:
            continue
        item_start, item_end = endpoints(item)
        if (same(item_start, start) and same(item_end, end)) or (
            same(item_start, end) and same(item_end, start)
        ):
            matches.append(item)
    if len(matches) != 1:
        raise ValueError(
            f"Expected one {net_name} segment {start}->{end}, found {len(matches)}"
        )
    item = matches[0]
    if abs(mm(item.GetWidth()) - 1.0) > 0.001:
        raise ValueError(f"Expected 1.0 mm width for {net_name} {start}->{end}")
    removed.append(
        {
            "net": net_name,
            "layer": board.GetLayerName(layer),
            "width_mm": 1.0,
            "start_mm": start,
            "end_mm": end,
        }
    )
    board.Remove(item)


added = []


def add_route(name, net_name, positions):
    for start, end in zip(positions, positions[1:]):
        segment = pcbnew.PCB_TRACK(board)
        segment.SetStart(point(start))
        segment.SetEnd(point(end))
        segment.SetWidth(pcbnew.FromMM(1.0))
        segment.SetLayer(pcbnew.B_Cu)
        segment.SetNet(board.GetNetsByName()[net_name])
        board.Add(segment)
    added.append(
        {
            "name": name,
            "net": net_name,
            "layer": "B.Cu",
            "width_mm": 1.0,
            "points_mm": positions,
            "length_mm": route_length(positions),
        }
    )


old_positive = [
    [29.3, 42.5],
    [27.8, 44.8],
    [27.8, 45.5],
    [22.5, 45.5],
    [22.5, 48.5],
    [13.0, 48.5],
    [13.0, 42.7],
]
old_negative = [
    [30.5, 42.9],
    [31.7, 41.1],
    [31.7, 40.5],
    [23.5, 40.5],
    [23.5, 41.5],
]
for net_name, positions in (("+13V8", old_positive), ("-13V8", old_negative)):
    for start, end in zip(positions, positions[1:]):
        remove_segment(net_name, pcbnew.B_Cu, start, end)

new_positive = [[29.3, 42.5], [22.5, 42.9], [15.0, 41.8], [13.0, 42.7]]
new_negative = [[30.5, 42.9], [30.8, 41.1], [23.5, 41.5]]
add_route("positive-j1-return", "+13V8", new_positive)
add_route("negative-j1-return", "-13V8", new_negative)

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

old_positive_length = route_length(old_positive)
old_negative_length = route_length(old_negative)
new_positive_length = route_length(new_positive)
new_negative_length = route_length(new_negative)
report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "shorten-j1-plus-minus-13v8-returns",
    "placement_changed": False,
    "track_width_mm": 1.0,
    "removed_segments": removed,
    "added_routes": added,
    "lengths": {
        "+13V8": {
            "before_mm": old_positive_length,
            "after_mm": new_positive_length,
            "reduction_percent": round(
                (1 - new_positive_length / old_positive_length) * 100, 1
            ),
        },
        "-13V8": {
            "before_mm": old_negative_length,
            "after_mm": new_negative_length,
            "reduction_percent": round(
                (1 - new_negative_length / old_negative_length) * 100, 1
            ),
        },
    },
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
