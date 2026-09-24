"""Move routed copper inward while preserving the completed PCB topology."""

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


def mm(value):
    return round(pcbnew.ToMM(value), 3)


def xy(item):
    position = item.GetPosition()
    return [mm(position.x), mm(position.y)]


def point(position):
    return pcbnew.VECTOR2I(pcbnew.FromMM(position[0]), pcbnew.FromMM(position[1]))


def same(a, b):
    return abs(a[0] - b[0]) < 0.001 and abs(a[1] - b[1]) < 0.001


def layer_name(item):
    return board.GetLayerName(item.GetLayer())


def find_track(net_name, layer, start, end, width=None):
    matches = []
    for item in board.GetTracks():
        if item.Type() != pcbnew.PCB_TRACE_T:
            continue
        if item.GetNetname() != net_name or layer_name(item) != layer:
            continue
        endpoints = ([mm(item.GetStart().x), mm(item.GetStart().y)],
                     [mm(item.GetEnd().x), mm(item.GetEnd().y)])
        if not ((same(endpoints[0], start) and same(endpoints[1], end)) or
                (same(endpoints[0], end) and same(endpoints[1], start))):
            continue
        if width is not None and abs(mm(item.GetWidth()) - width) >= 0.001:
            continue
        matches.append(item)
    if len(matches) != 1:
        raise ValueError(
            f"Expected one {net_name} {layer} track {start}->{end}, found {len(matches)}"
        )
    return matches[0]


def find_via(net_name, position):
    matches = [
        item for item in board.GetTracks()
        if item.Type() == pcbnew.PCB_VIA_T
        and item.GetNetname() == net_name
        and same(xy(item), position)
    ]
    if len(matches) != 1:
        raise ValueError(f"Expected one {net_name} via at {position}, found {len(matches)}")
    return matches[0]


def move_node(net_name, old, new):
    moved = 0
    for item in board.GetTracks():
        if item.GetNetname() != net_name:
            continue
        if item.Type() == pcbnew.PCB_VIA_T:
            if same(xy(item), old):
                item.SetPosition(point(new))
                moved += 1
            continue
        start = [mm(item.GetStart().x), mm(item.GetStart().y)]
        end = [mm(item.GetEnd().x), mm(item.GetEnd().y)]
        if same(start, old):
            item.SetStart(point(new))
            moved += 1
        if same(end, old):
            item.SetEnd(point(new))
            moved += 1
    if moved == 0:
        raise ValueError(f"No {net_name} track endpoint or via found at {old}")
    changes.append({"net": net_name, "from_mm": old, "to_mm": new, "items": moved})


def add_segment(net_name, layer, width, start, end):
    segment = pcbnew.PCB_TRACK(board)
    segment.SetStart(point(start))
    segment.SetEnd(point(end))
    segment.SetWidth(pcbnew.FromMM(width))
    segment.SetLayer(board.GetLayerID(layer))
    segment.SetNet(board.GetNetsByName()[net_name])
    board.Add(segment)


changes = []

zone_polygons = {
    "LOGIC_GND": [[24.5, 1.0], [99.0, 1.0], [99.0, 51.0], [24.5, 51.0]],
    "POWER_STAGE_GND": [[0.6, 0.6], [24.9, 0.6], [24.9, 51.4], [0.6, 51.4]],
}
for zone in board.Zones():
    zone_name = zone.GetZoneName()
    if zone_name not in zone_polygons:
        continue
    outline = zone.Outline()
    outline.RemoveAllContours()
    outline.NewOutline()
    for position in zone_polygons[zone_name]:
        outline.Append(point(position))
    changes.append({"zone": zone_name, "polygon_mm": zone_polygons[zone_name]})

# Bring the lower LCD bus fanout at least 1.4 mm inside the finished edge.
down_nets = [
    "LCD_R0", "LCD_R1", "LCD_R2", "LCD_R3", "LCD_G0", "LCD_G1",
    "LCD_G2", "LCD_G3", "LCD_VSYNC", "LCD_HSYNC", "LCD_NCLK",
]
for index, net_name in enumerate(down_nets):
    channel_x = round(48.8 + 0.8 * index, 3)
    old_y = round(51.0 - 0.6 * index, 3)
    new_y = round(50.5 - 0.55 * index, 3)
    move_node(net_name, [channel_x, old_y], [channel_x, new_y])
    # Each J1 escape has a vertical-to-horizontal bend at the old lane height.
    candidates = []
    for item in board.GetTracks():
        if item.Type() != pcbnew.PCB_TRACE_T or item.GetNetname() != net_name:
            continue
        for endpoint in (item.GetStart(), item.GetEnd()):
            position = [mm(endpoint.x), mm(endpoint.y)]
            if abs(position[1] - old_y) < 0.001 and position[0] < channel_x:
                candidates.append(position)
    source_bends = []
    for candidate in candidates:
        if not any(same(candidate, existing) for existing in source_bends):
            source_bends.append(candidate)
    if len(source_bends) != 1:
        raise ValueError(f"Expected one {net_name} source bend at y={old_y}, found {source_bends}")
    move_node(net_name, source_bends[0], [source_bends[0][0], new_y])

# Shift the signal and power bridges away from the routed top-notch edges.
for net_name, old, new in (
    ("LCD_R4", [34.0, 11.9], [34.0, 12.1]),
    ("LCD_R4", [54.2, 11.9], [54.2, 12.1]),
    ("+3V3", [51.0, 11.8], [51.3, 11.8]),
    ("+3V3", [51.0, 5.41], [51.3, 5.41]),
    ("-13V8", [26.0, 27.5], [26.3, 27.5]),
    ("-13V8", [26.0, 11.0], [26.3, 11.3]),
    ("-13V8", [24.4, 10.8], [24.4, 11.3]),
    ("-13V8", [23.0, 10.2], [23.0, 10.5]),
):
    move_node(net_name, old, new)

# Replace the edge-hugging +5 V crossing with an inset dogleg.  The short neck
# at x=63 mm is intentionally kept at 0.3 mm for the Pico-header gap.
find_track("+5V", "B.Cu", [28.5, 14.5], [28.5, 10.85], 0.6).SetEnd(point([28.5, 11.3]))
find_via("+5V", [28.5, 10.85]).SetPosition(point([28.5, 11.3]))
obsolete_tracks = [
    find_track("+5V", "F.Cu", start, end, width)
    for start, end, width in (
    ([28.5, 10.85], [63.0, 10.85], 0.6),
    ([63.0, 10.85], [63.0, 10.49], 0.3),
    )
]
for obsolete_track in obsolete_tracks:
    board.Remove(obsolete_track)
for start, end in zip(
    ([28.5, 11.3], [51.5, 11.3], [51.5, 10.49], [63.0, 10.49]),
    ([51.5, 11.3], [51.5, 10.49], [63.0, 10.49]),
):
    add_segment("+5V", "F.Cu", 0.6, start, end)
changes.append({
    "net": "+5V",
    "from_mm": [[28.5, 10.85], [63.0, 10.85]],
    "to_mm": [[28.5, 11.3], [51.5, 11.3], [51.5, 10.49], [63.0, 10.49]],
})

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "edge-routing-refinement",
    "routed_copper_edge_clearance_mm": 1.0,
    "logic_ground_zone_edge_clearance_mm": 1.0,
    "power_ground_zone_edge_clearance_mm": 0.6,
    "changes": changes,
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
