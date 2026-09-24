"""Widen every practical +/-13 V route while preserving J1 pad clearances."""

import argparse
import hashlib
import json
from pathlib import Path

import pcbnew


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--input", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
parser.add_argument("--report", type=Path)
args = parser.parse_args()

board = pcbnew.LoadBoard(str(args.input))


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


tracks = list(board.GetTracks())
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
    removed.append(
        {
            "kind": "segment",
            "net": net_name,
            "layer": board.GetLayerName(layer),
            "width_mm": mm(item.GetWidth()),
            "start_mm": start,
            "end_mm": end,
        }
    )
    board.Remove(item)


def remove_via(net_name, position):
    matches = [
        item
        for item in tracks
        if item.Type() == pcbnew.PCB_VIA_T
        and item.GetNetname() == net_name
        and same([mm(item.GetPosition().x), mm(item.GetPosition().y)], position)
    ]
    if len(matches) != 1:
        raise ValueError(f"Expected one {net_name} via at {position}, found {len(matches)}")
    removed.append({"kind": "via", "net": net_name, "position_mm": position})
    board.Remove(matches[0])


added = []


def add_route(name, net_name, layer, width_mm, positions):
    for start, end in zip(positions, positions[1:]):
        segment = pcbnew.PCB_TRACK(board)
        segment.SetStart(point(start))
        segment.SetEnd(point(end))
        segment.SetWidth(pcbnew.FromMM(width_mm))
        segment.SetLayer(layer)
        segment.SetNet(board.GetNetsByName()[net_name])
        board.Add(segment)
    added.append(
        {
            "kind": "route",
            "name": name,
            "net": net_name,
            "layer": board.GetLayerName(layer),
            "width_mm": width_mm,
            "points_mm": positions,
        }
    )


def add_via(name, net_name, position):
    via = pcbnew.PCB_VIA(board)
    via.SetPosition(point(position))
    via.SetWidth(pcbnew.F_Cu, pcbnew.FromMM(0.6))
    via.SetWidth(pcbnew.B_Cu, pcbnew.FromMM(0.6))
    via.SetDrill(pcbnew.FromMM(0.3))
    via.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
    via.SetNet(board.GetNetsByName()[net_name])
    board.Add(via)
    added.append(
        {
            "kind": "via",
            "name": name,
            "net": net_name,
            "position_mm": position,
            "pad_mm": 0.6,
            "drill_mm": 0.3,
        }
    )


# Replace the two close, parallel J1 back-layer necks with immediately diverging
# 1.0 mm routes.  The +13 V via moves 0.28 mm so the two 1.0 mm end caps retain
# the board's 0.20 mm clearance.  Only the fine-pitch front escapes stay narrow.
for net_name, layer, start, end in (
    ("+13V8", pcbnew.B_Cu, [29.5, 42.7], [29.5, 45.5]),
    ("-13V8", pcbnew.B_Cu, [30.5, 42.9], [30.5, 41.5]),
    ("+13V8", pcbnew.B_Cu, [29.5, 45.5], [22.5, 45.5]),
    ("+13V8", pcbnew.B_Cu, [22.5, 45.5], [22.5, 48.5]),
    ("+13V8", pcbnew.B_Cu, [22.5, 48.5], [13.0, 48.5]),
    ("+13V8", pcbnew.B_Cu, [13.0, 48.5], [13.0, 42.7]),
    ("-13V8", pcbnew.B_Cu, [30.5, 41.5], [23.5, 41.5]),
    ("+13V8", pcbnew.F_Cu, [29.5, 44.0], [29.5, 42.7]),
):
    remove_segment(net_name, layer, start, end)
remove_via("+13V8", [29.5, 42.7])

add_route(
    "positive-j1-pad-escape",
    "+13V8",
    pcbnew.F_Cu,
    0.3,
    [[29.5, 44.0], [29.5, 42.7], [29.3, 42.5]],
)
add_via("positive-j1-via", "+13V8", [29.3, 42.5])
add_route(
    "positive-j1-trunk",
    "+13V8",
    pcbnew.B_Cu,
    1.0,
    [[29.3, 42.5], [22.5, 42.9], [15.0, 41.8], [13.0, 42.7]],
)
add_route(
    "negative-j1-trunk",
    "-13V8",
    pcbnew.B_Cu,
    1.0,
    [[30.5, 42.9], [30.8, 41.1], [23.5, 41.5]],
)


# The original negative LED branch was safe at 0.2 mm, but a 1.0 mm trace was
# too close to R7, VCPP_ADJ and the stepped board edge.  Move it into the open
# channel and keep at least the configured copper-to-edge margin at the notch.
negative_led_original = (
    ([21.0, 29.0], [24.8, 29.0]),
    ([24.8, 29.0], [26.3, 27.5]),
    ([26.3, 27.5], [26.3, 11.3]),
    ([26.3, 11.3], [24.4, 11.3]),
    ([24.4, 11.3], [23.0, 10.5]),
    ([23.0, 10.5], [20.5, 6.5]),
)
for start, end in negative_led_original:
    remove_segment("-13V8", pcbnew.F_Cu, start, end)
add_route(
    "negative-indicator-feed",
    "-13V8",
    pcbnew.F_Cu,
    1.0,
    [
        [21.0, 29.0],
        [24.8, 29.0],
        [26.45, 27.35],
        [26.45, 11.6],
        [23.4, 11.6],
        [23.4, 10.8],
        [20.5, 6.5],
    ],
)


narrow_necks = [
    ("+13V8", [29.5, 44.0], [29.5, 42.7]),
    ("+13V8", [29.5, 42.7], [29.3, 42.5]),
    ("-13V8", [30.5, 44.0], [30.5, 42.9]),
]
widened = []
for item in board.GetTracks():
    if item.Type() != pcbnew.PCB_TRACE_T:
        continue
    if item.GetNetname() not in ("+13V8", "-13V8"):
        continue
    start, end = endpoints(item)
    if any(
        item.GetNetname() == net_name
        and ((same(start, first) and same(end, second))
             or (same(start, second) and same(end, first)))
        for net_name, first, second in narrow_necks
    ):
        continue
    old_width = mm(item.GetWidth())
    if old_width < 0.999:
        widened.append(
            {
                "net": item.GetNetname(),
                "layer": board.GetLayerName(item.GetLayer()),
                "start_mm": start,
                "end_mm": end,
                "old_width_mm": old_width,
                "new_width_mm": 1.0,
            }
        )
        item.SetWidth(pcbnew.FromMM(1.0))


# A 45-degree thermal orientation preserves two B.Cu spokes at C11 after the
# adjacent sense branch is widened, without reducing the 0.30 mm gap or spoke.
c11_ground = board.FindFootprintByReference("C11").FindPadByNumber("1")
if c11_ground.GetNetname() != "GND":
    raise ValueError("Expected C11 pad 1 to be GND")
c11_ground.SetThermalSpokeAngleDegrees(45.0)

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "all-practical-plus-minus-13v8-routes-to-1mm",
    "removed_items": removed,
    "added_items": added,
    "widened_segments": widened,
    "narrow_j1_pad_escapes": [
        {"net": net_name, "start_mm": start, "end_mm": end, "width_mm": 0.3}
        for net_name, start, end in narrow_necks
    ],
    "c11_ground_thermal_spoke_angle_degrees": 45.0,
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
