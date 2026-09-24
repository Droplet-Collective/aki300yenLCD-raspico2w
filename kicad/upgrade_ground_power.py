"""Add two-layer thermal GND pours and reinforce power paths on a routed board."""

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


def xy(item):
    position = item.GetPosition()
    return [mm(position.x), mm(position.y)]


def same(first, second):
    return all(abs(a - b) < 0.001 for a, b in zip(first, second))


def add_segment(net_name, layer_name, width_mm, start, end):
    segment = pcbnew.PCB_TRACK(board)
    segment.SetStart(point(start))
    segment.SetEnd(point(end))
    segment.SetWidth(pcbnew.FromMM(width_mm))
    segment.SetLayer(board.GetLayerID(layer_name))
    segment.SetNet(board.GetNetsByName()[net_name])
    board.Add(segment)


def add_via(net_name, position):
    via = pcbnew.PCB_VIA(board)
    via.SetPosition(point(position))
    via.SetWidth(pcbnew.F_Cu, pcbnew.FromMM(0.6))
    via.SetWidth(pcbnew.B_Cu, pcbnew.FromMM(0.6))
    via.SetDrill(pcbnew.FromMM(0.3))
    via.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
    via.SetNet(board.GetNetsByName()[net_name])
    board.Add(via)


def count_vias(net_name, position):
    return sum(
        item.Type() == pcbnew.PCB_VIA_T
        and item.GetNetname() == net_name
        and same(xy(item), position)
        for item in board.GetTracks()
    )


zone_polygons = {
    "POWER_STAGE_GND": [[0.6, 0.6], [24.9, 0.6], [24.9, 51.4], [0.6, 51.4]],
    "LOGIC_GND": [[24.5, 1.0], [99.0, 1.0], [99.0, 51.0], [24.5, 51.0]],
}
existing_zones = {zone.GetZoneName(): zone for zone in board.Zones()}
if set(existing_zones) != set(zone_polygons):
    raise ValueError(
        f"Expected exactly {sorted(zone_polygons)} zones, found {sorted(existing_zones)}"
    )

zone_report = []
for back_name, polygon in zone_polygons.items():
    back_zone = existing_zones[back_name]
    if back_zone.GetLayer() != pcbnew.B_Cu or back_zone.GetNetname() != "GND":
        raise ValueError(f"{back_name} must be a B.Cu GND zone")
    for zone, layer, name in (
        (back_zone, pcbnew.B_Cu, back_name),
        (pcbnew.ZONE(board), pcbnew.F_Cu, f"{back_name}_F"),
    ):
        zone.SetLayer(layer)
        zone.SetNetCode(board.GetNetsByName()["GND"].GetNetCode())
        zone.SetZoneName(name)
        zone.SetLocalClearance(pcbnew.FromMM(0.3))
        zone.SetMinThickness(pcbnew.FromMM(0.25))
        zone.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
        zone.SetThermalReliefGap(pcbnew.FromMM(0.3))
        zone.SetThermalReliefSpokeWidth(pcbnew.FromMM(0.3))
        zone.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
        if layer == pcbnew.F_Cu:
            outline = zone.Outline()
            outline.NewOutline()
            for position in polygon:
                outline.Append(point(position))
            board.Add(zone)
        zone_report.append({
            "name": name,
            "layer": board.GetLayerName(layer),
            "pad_connection": "thermal",
            "thermal_gap_mm": 0.3,
            "thermal_spoke_width_mm": 0.3,
            "polygon_mm": polygon,
        })

widened_segments = []
constrained_segments = [
    ("+13V8", [29.5, 42.7], [29.5, 45.5]),
    ("-13V8", [30.5, 42.9], [30.5, 41.5]),
]
for item in board.GetTracks():
    if item.Type() != pcbnew.PCB_TRACE_T:
        continue
    if item.GetNetname() not in ("+13V8", "-13V8"):
        continue
    if abs(mm(item.GetWidth()) - 0.6) >= 0.001:
        continue
    start = [mm(item.GetStart().x), mm(item.GetStart().y)]
    end = [mm(item.GetEnd().x), mm(item.GetEnd().y)]
    if any(
        item.GetNetname() == net_name
        and ((same(start, first) and same(end, second))
             or (same(start, second) and same(end, first)))
        for net_name, first, second in constrained_segments
    ):
        continue
    widened_segments.append({
        "net": item.GetNetname(),
        "layer": board.GetLayerName(item.GetLayer()),
        "start_mm": [mm(item.GetStart().x), mm(item.GetStart().y)],
        "end_mm": [mm(item.GetEnd().x), mm(item.GetEnd().y)],
        "old_width_mm": 0.6,
        "new_width_mm": 1.0,
    })
    item.SetWidth(pcbnew.FromMM(1.0))

via_groups = [
    ("+5V", [18.0, 27.385], [17.2, 27.385], 0.6, None),
    ("+5V", [25.5, 37.0], [26.3, 37.0], 0.6, 0.6),
    ("+13V8", [13.0, 42.7], [12.2, 42.7], 1.0, 1.0),
    ("-13V8", [23.5, 41.5], [22.7, 41.5], 1.0, 1.0),
]
via_report = []
for net_name, primary, secondary, front_width, back_width in via_groups:
    if count_vias(net_name, primary) != 1:
        raise ValueError(f"Expected one {net_name} source via at {primary}")
    if count_vias(net_name, secondary) != 0:
        raise ValueError(f"Unexpected existing {net_name} via at {secondary}")
    add_via(net_name, secondary)
    add_segment(net_name, "F.Cu", front_width, primary, secondary)
    if back_width is not None:
        add_segment(net_name, "B.Cu", back_width, primary, secondary)
    via_report.append({
        "net": net_name,
        "positions_mm": [primary, secondary],
        "via_pad_mm": 0.6,
        "via_drill_mm": 0.3,
        "front_link_width_mm": front_width,
        "back_link_width_mm": back_width,
    })

ground_stitch_vias = [[19.8, 27.0]]
for position in ground_stitch_vias:
    if count_vias("GND", position) != 0:
        raise ValueError(f"Unexpected existing GND via at {position}")
    add_via("GND", position)

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "dual-layer-ground-and-power-reinforcement",
    "ground_zones": zone_report,
    "widened_segments": widened_segments,
    "constrained_0.6_mm_segments": [
        {"net": net_name, "start_mm": start, "end_mm": end}
        for net_name, start, end in constrained_segments
    ],
    "power_via_groups": via_report,
    "ground_stitch_vias": [
        {"position_mm": position, "via_pad_mm": 0.6, "via_drill_mm": 0.3}
        for position in ground_stitch_vias
    ],
    "via_current_basis": {
        "source": "TI SLVA959B table 2 (IPC-2152, 1 oz, 10 C rise)",
        "0.3_mm_drill_estimated_current_a": 0.84,
        "mc34063_estimated_peak_current_a": 0.70,
        "implementation": (
            "paired vias at the principal supply and bias-rail layer transitions"
        ),
    },
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
