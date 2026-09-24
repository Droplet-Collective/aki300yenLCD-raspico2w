"""Route the Pico/LCD side while preserving the completed power stage."""

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
footprints = {fp.GetReference(): fp for fp in board.GetFootprints()}

SIGNAL_NETS = {
    "LCD_R0", "LCD_R1", "LCD_R2", "LCD_R3", "LCD_R4", "LCD_R5",
    "LCD_G0", "LCD_G1", "LCD_G2", "LCD_G3", "LCD_G4", "LCD_G5",
    "LCD_B0", "LCD_B1", "LCD_B2", "LCD_B3", "LCD_B4", "LCD_B5",
    "LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC", "VCPP_ADJ", "+3V3",
}
already_routed = sorted({
    item.GetNetname() for item in board.GetTracks()
    if item.GetNetname() in SIGNAL_NETS
})
if already_routed:
    raise ValueError(f"Pico/LCD input already has routed target nets: {already_routed}")


def mm(value):
    return round(pcbnew.ToMM(value), 3)


def pad(ref, number):
    position = footprints[ref].FindPadByNumber(str(number)).GetPosition()
    return [mm(position.x), mm(position.y)]


def point(xy):
    return pcbnew.VECTOR2I(pcbnew.FromMM(xy[0]), pcbnew.FromMM(xy[1]))


def add_segment(net_name, layer_name, width_mm, start, end):
    segment = pcbnew.PCB_TRACK(board)
    segment.SetStart(point(start))
    segment.SetEnd(point(end))
    segment.SetWidth(pcbnew.FromMM(width_mm))
    segment.SetLayer(pcbnew.F_Cu if layer_name == "F.Cu" else pcbnew.B_Cu)
    segment.SetNet(board.GetNetsByName()[net_name])
    board.Add(segment)


def add_route(name, net_name, layer_name, width_mm, points):
    routes.append({
        "name": name,
        "net": net_name,
        "layer": layer_name,
        "width_mm": width_mm,
        "points_mm": points,
    })
    for start, end in zip(points, points[1:]):
        add_segment(net_name, layer_name, width_mm, start, end)


def add_via(name, net_name, position, size_mm=0.6, drill_mm=0.3):
    via = pcbnew.PCB_VIA(board)
    via.SetPosition(point(position))
    via.SetWidth(pcbnew.F_Cu, pcbnew.FromMM(size_mm))
    via.SetWidth(pcbnew.B_Cu, pcbnew.FromMM(size_mm))
    via.SetDrill(pcbnew.FromMM(drill_mm))
    via.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
    via.SetNet(board.GetNetsByName()[net_name])
    board.Add(via)
    vias.append({
        "name": name,
        "net": net_name,
        "position_mm": position,
        "size_mm": size_mm,
        "drill_mm": drill_mm,
    })


routes = []
vias = []

# The 0.5 mm-pitch connector is escaped in two directions.  This avoids the
# unsafe long diagonal fanout that passes adjacent pads and vias.
down_signals = [
    ("LCD_R0", 27, 25, 35.89),
    ("LCD_R1", 26, 24, 38.43),
    ("LCD_R2", 25, 22, 40.97),
    ("LCD_R3", 24, 21, 23.19),
    ("LCD_G0", 20, 17, None),
    ("LCD_G1", 19, 16, None),
    ("LCD_G2", 18, 15, None),
    ("LCD_G3", 17, 14, None),
    ("LCD_VSYNC", 5, 29, 28.27),
    ("LCD_HSYNC", 4, 27, 30.81),
    ("LCD_NCLK", 2, 26, 33.35),
]
right_columns = {
    "LCD_R3": 91.0,
    "LCD_VSYNC": 90.0,
    "LCD_HSYNC": 89.0,
    "LCD_NCLK": 88.0,
    "LCD_R0": 87.0,
    "LCD_R1": 86.0,
    "LCD_R2": 85.0,
}


def finish_right_signal(net_name, channel_x, turn_y, target):
    turn = [channel_x, turn_y]
    add_via(f"{net_name}-turn-via", net_name, turn)
    entry_y = 29.8 if net_name == "LCD_HSYNC" else turn_y
    right_entry = [82.25, entry_y]
    entry_points = [turn, [66.4, turn_y], right_entry]
    if entry_y != turn_y:
        entry_points = [turn, [66.4, turn_y], [67.0, entry_y], right_entry]
    add_route(f"{net_name}-pico-gap", net_name, "F.Cu", 0.2,
              entry_points)
    add_via(f"{net_name}-right-via", net_name, right_entry)
    outside_x = right_columns[net_name]
    outside_target = [outside_x, target[1]]
    add_route(f"{net_name}-outside-channel", net_name, "B.Cu", 0.2,
              [right_entry, [outside_x, turn_y], outside_target])
    add_via(f"{net_name}-target-via", net_name, outside_target)
    add_route(f"{net_name}-u1-entry", net_name, "F.Cu", 0.2,
              [outside_target, target])


for index, (net_name, j1_pad, u1_pad, right_gap) in enumerate(down_signals):
    source = pad("J1", j1_pad)
    target = pad("U1", u1_pad)
    # Keep the lower fanout comfortably inside the routed board edge.  The
    # closest track centre is 1.5 mm from the edge, leaving 1.4 mm of copper
    # clearance for a 0.2 mm trace.
    lane_y = round(50.5 - 0.55 * index, 3)
    channel_x = round(48.8 + 0.8 * index, 3)
    breakout = [channel_x, lane_y]
    add_route(f"{net_name}-j1-breakout", net_name, "F.Cu", 0.2,
              [source, [source[0], lane_y], breakout])
    add_via(f"{net_name}-breakout-via", net_name, breakout, 0.5, 0.3)
    turn_y = right_gap if right_gap is not None else target[1]
    add_route(f"{net_name}-channel", net_name, "B.Cu", 0.2,
              [breakout, [channel_x, turn_y]])
    if right_gap is None:
        turn = [channel_x, turn_y]
        add_via(f"{net_name}-turn-via", net_name, turn)
        add_route(f"{net_name}-u1-entry", net_name, "F.Cu", 0.2,
                  [turn, target])
    else:
        finish_right_signal(net_name, channel_x, turn_y, target)

up_signals = [
    ("LCD_G4", 16, 12, 15.57, 63.0),
    ("LCD_G5", 15, 11, 18.11, 62.2),
    ("LCD_B0", 13, 10, 20.65, 61.4),
    ("LCD_B1", 12, 9, 23.80, 60.6),
    ("LCD_B2", 11, 7, 26.4, 59.8),
    ("LCD_B3", 10, 6, 32.0, 59.0),
    ("LCD_B4", 9, 5, 34.0, 58.2),
    ("LCD_B5", 8, 4, 35.25, 57.4),
]
for net_name, j1_pad, u1_pad, lane_y, channel_x in up_signals:
    source = pad("J1", j1_pad)
    target = pad("U1", u1_pad)
    breakout = [channel_x, lane_y]
    add_route(f"{net_name}-j1-breakout", net_name, "F.Cu", 0.2,
              [source, [source[0], lane_y], breakout])
    add_via(f"{net_name}-breakout-via", net_name, breakout, 0.5, 0.3)
    turn = [channel_x, target[1]]
    add_route(f"{net_name}-channel", net_name, "B.Cu", 0.2,
              [breakout, turn])
    add_via(f"{net_name}-turn-via", net_name, turn)
    add_route(f"{net_name}-u1-entry", net_name, "F.Cu", 0.2,
              [turn, target])

for net_name, j1_pad, u1_pad, source_x, lane_y, bridge_x, dogleg_y, channel_x in (
    ("LCD_R4", 23, 20, 34.0, 12.1, 54.2, 13.2, 64.6),
    ("LCD_R5", 22, 19, 35.0, 12.7, 53.4, 14.9, 63.8),
):
    source = pad("J1", j1_pad)
    target = pad("U1", u1_pad)
    source_via = [source_x, 37.0]
    bridge_left = [source_x, lane_y]
    bridge_right = [bridge_x, lane_y]
    target_via = [channel_x, target[1]]
    add_route(f"{net_name}-j1-escape", net_name, "F.Cu", 0.2,
              [source, [source[0], 43.0], source_via])
    add_via(f"{net_name}-source-via", net_name, source_via)
    add_route(f"{net_name}-left-channel", net_name, "B.Cu", 0.2,
              [source_via, bridge_left])
    add_via(f"{net_name}-bridge-left-via", net_name, bridge_left)
    add_route(f"{net_name}-notch-bridge", net_name, "F.Cu", 0.2,
              [bridge_left, bridge_right])
    add_via(f"{net_name}-bridge-right-via", net_name, bridge_right)
    jump_left = 56.0 if net_name == "LCD_R4" else 56.2
    jump_right = 60.0 if net_name == "LCD_R4" else 60.2
    add_route(f"{net_name}-right-channel-left", net_name, "B.Cu", 0.2,
              [bridge_right, [bridge_x, dogleg_y], [jump_left, dogleg_y]])
    add_via(f"{net_name}-jump-left-via", net_name, [jump_left, dogleg_y])
    add_route(f"{net_name}-channel-jump", net_name, "F.Cu", 0.2,
              [[jump_left, dogleg_y], [jump_right, dogleg_y]])
    add_via(f"{net_name}-jump-right-via", net_name, [jump_right, dogleg_y])
    add_route(f"{net_name}-right-channel-right", net_name, "B.Cu", 0.2,
              [[jump_right, dogleg_y], [channel_x, dogleg_y], target_via])
    add_via(f"{net_name}-target-via", net_name, target_via)
    add_route(f"{net_name}-u1-entry", net_name, "F.Cu", 0.2,
              [target_via, target])

# J1 power/control escapes use the opposite side of the connector from the
# display bus.  Wide trunks are narrowed only for the short Pico-header gaps.
add_route("vcpp-j1-escape", "VCPP_ADJ", "F.Cu", 0.2,
          [pad("J1", 29), [32.5, 43.0], [27.5, 37.0]])
add_via("vcpp-source-via", "VCPP_ADJ", [27.5, 37.0])
add_route("vcpp-left", "VCPP_ADJ", "B.Cu", 0.25,
          [[27.5, 37.0], [27.5, 13.6]])
add_via("vcpp-bridge-left-via", "VCPP_ADJ", [27.5, 13.6])
add_route("vcpp-notch-bridge", "VCPP_ADJ", "F.Cu", 0.25,
          [[27.5, 13.6], [52.0, 13.6]])
add_via("vcpp-bridge-right-via", "VCPP_ADJ", [52.0, 13.6])
add_route("vcpp-to-trimmer", "VCPP_ADJ", "B.Cu", 0.25,
          [[52.0, 13.6], [52.0, 8.76], pad("RV1", 2)])

for name, j1_pad, position in (
    ("3v3-j1-a", 28, [30.0, 37.0]),
    ("3v3-j1-b", 21, [37.0, 37.0]),
):
    source = pad("J1", j1_pad)
    add_route(f"{name}-escape", "+3V3", "F.Cu", 0.2,
              [source, [source[0], 43.0], position])
add_route("3v3-j1-join-a", "+3V3", "F.Cu", 0.6,
          [[30.0, 37.0], [30.0, 34.0], [37.0, 34.0], [37.0, 37.0]])
add_via("3v3-trunk-via", "+3V3", [37.0, 34.0])
add_route("3v3-trimmer-trunk", "+3V3", "B.Cu", 0.6,
          [[37.0, 34.0], [44.0, 34.0], [44.0, 11.8],
           [51.3, 11.8], [51.3, 5.41], [53.5, 5.41], pad("RV1", 1)])
add_route("3v3-pico-right", "+3V3", "B.Cu", 0.6,
          [[53.5, 5.41], [61.0, 5.41], [61.0, 8.0],
           [88.0, 8.0], [88.0, 11.76], [85.9, 11.76]])
add_via("3v3-u1-via", "+3V3", [85.9, 11.76])
add_route("3v3-u1-entry", "+3V3", "F.Cu", 0.6,
          [[85.9, 11.76], pad("U1", 36)])

add_route("5v-j1-escape", "+5V", "F.Cu", 0.2,
          [pad("J1", 31), [31.5, 43.0], [25.5, 37.0]])
add_via("5v-source-via", "+5V", [25.5, 37.0])
add_via("5v-source-via-pair", "+5V", [26.3, 37.0])
add_route("5v-source-via-pair-front", "+5V", "F.Cu", 0.6,
          [[25.5, 37.0], [26.3, 37.0]])
add_route("5v-source-via-pair-back", "+5V", "B.Cu", 0.6,
          [[25.5, 37.0], [26.3, 37.0]])
add_route("5v-power-stage-join", "+5V", "B.Cu", 0.6,
          [[25.5, 37.0], [26.8, 35.0], [26.8, 32.0], [25.8, 30.8]])
add_route("5v-pico-channel", "+5V", "F.Cu", 0.6,
          [[25.5, 37.0], [28.5, 34.0], [28.5, 14.5]])
add_via("5v-pico-upper-via", "+5V", [28.5, 14.5])
add_route("5v-pico-notch-crossing", "+5V", "B.Cu", 0.6,
          [[28.5, 14.5], [28.5, 11.3]])
add_via("5v-pico-lower-via", "+5V", [28.5, 11.3])
add_route("5v-pico-left", "+5V", "F.Cu", 0.6,
          [[28.5, 11.3], [51.5, 11.3], [51.5, 10.49], [63.0, 10.49]])
add_route("5v-pico-neck", "+5V", "F.Cu", 0.3,
          [[63.0, 10.49], [67.0, 10.49]])
add_via("5v-pico-inner-via", "+5V", [67.0, 10.49])
add_route("5v-u1", "+5V", "B.Cu", 0.6,
          [[67.0, 10.49], [86.8, 10.49]])
add_via("5v-u1-right-via", "+5V", [86.8, 10.49])
add_route("5v-u1-entry", "+5V", "F.Cu", 0.6,
          [[86.8, 10.49], [90.0, 7.0], [90.0, 1.6], pad("U1", 40)])

# LCD bias rails leave their 0.5 mm-pitch pads with a short narrow escape.  The
# back-layer routes immediately diverge and remain 1.0 mm through to the stage.
add_route("13v8-j1-neck", "+13V8", "F.Cu", 0.3,
          [pad("J1", 35), [29.5, 42.7], [29.3, 42.5]])
add_via("13v8-j1-via", "+13V8", [29.3, 42.5])
add_route("13v8-return-trunk", "+13V8", "B.Cu", 1.0,
          [[29.3, 42.5], [22.5, 42.9], [15.0, 41.8], [13.0, 42.7]])
add_via("13v8-stage-via", "+13V8", [13.0, 42.7])
add_via("13v8-stage-via-pair", "+13V8", [12.2, 42.7])
add_route("13v8-stage-via-pair-back", "+13V8", "B.Cu", 1.0,
          [[13.0, 42.7], [12.2, 42.7]])
add_route("13v8-stage-via-pair-front", "+13V8", "F.Cu", 1.0,
          [[13.0, 42.7], [12.2, 42.7]])
add_route("13v8-stage-join", "+13V8", "F.Cu", 1.0,
          [[13.0, 42.7], [12.75, 43.5]])

add_route("neg13v8-j1-neck", "-13V8", "F.Cu", 0.3,
          [pad("J1", 33), [30.5, 42.9]])
add_via("neg13v8-j1-via", "-13V8", [30.5, 42.9])
add_route("neg13v8-return-trunk", "-13V8", "B.Cu", 1.0,
          [[30.5, 42.9], [30.8, 41.1], [23.5, 41.5]])
add_via("neg13v8-stage-via", "-13V8", [23.5, 41.5])
add_via("neg13v8-stage-via-pair", "-13V8", [22.7, 41.5])
add_route("neg13v8-stage-via-pair-front", "-13V8", "F.Cu", 1.0,
          [[23.5, 41.5], [22.7, 41.5]])
add_route("neg13v8-stage-via-pair-back", "-13V8", "B.Cu", 1.0,
          [[23.5, 41.5], [22.7, 41.5]])
add_route("neg13v8-stage-join", "-13V8", "F.Cu", 1.0,
          [[23.5, 41.5], [23.5, 43.0]])

# J1 ground pads escape away from the signal fanout into a logic-side B.Cu
# plane.  Adjacent pads use staggered vias to preserve hole clearance.
ground_escapes = [
    ("gnd-j1-36", 36, [29.0, 47.0]),
    ("gnd-j1-34", 34, [30.0, 48.0]),
    ("gnd-j1-32", 32, [31.0, 47.0]),
    ("gnd-j1-30", 30, [32.0, 48.0]),
]
seen_ground_vias = set()
for name, j1_pad, position in ground_escapes:
    source = pad("J1", j1_pad)
    points = [source, position]
    add_route(name, "GND", "F.Cu", 0.2, points)
    key = tuple(position)
    if key not in seen_ground_vias:
        add_via(f"{name}-via", "GND", position, 0.5, 0.3)
        seen_ground_vias.add(key)

for name, j1_pad, position in (
    ("gnd-j1-14", 14, [47.0, 19.4]),
    ("gnd-j1-7", 7, [47.0, 38.0]),
    ("gnd-j1-6", 6, [47.0, 39.4]),
    ("gnd-j1-3", 3, [44.0, 41.0]),
    ("gnd-j1-1", 1, [45.2, 40.0]),
):
    source = pad("J1", j1_pad)
    if j1_pad == 14:
        points = [source, [source[0], 19.4], position]
    elif j1_pad in (7, 6):
        points = [source, [source[0], position[1]], position]
    elif j1_pad == 3:
        points = [source, [source[0], 41.0], position]
    else:
        points = [source, [source[0], 40.0], position]
    add_route(name, "GND", "F.Cu", 0.2,
              points)
    add_via(f"{name}-via", "GND", position, 0.5, 0.3)

for name, u1_pad, position in (
    ("gnd-u1-left-3", 3, [67.0, 6.68]),
    ("gnd-u1-left-8", 8, [67.0, 19.38]),
    ("gnd-u1-left-13", 13, [67.0, 32.08]),
    ("gnd-u1-left-18", 18, [67.0, 44.78]),
    ("gnd-u1-right-38", 38, [85.9, 6.68]),
    ("gnd-u1-right-33", 33, [85.9, 19.38]),
    ("gnd-u1-right-28", 28, [85.9, 32.08]),
    ("gnd-u1-right-23", 23, [85.9, 44.78]),
):
    add_route(name, "GND", "F.Cu", 0.4, [pad("U1", u1_pad), position])
    add_via(f"{name}-via", "GND", position)

add_route("gnd-logic-bridge", "GND", "F.Cu", 0.4,
          [[47.0, 19.4], [63.0, 19.4], pad("U1", 8)])

logic_ground_polygon = [[24.5, 1.0], [99.0, 1.0], [99.0, 51.0], [24.5, 51.0]]
logic_ground_zones = []
for layer, name in (
    (pcbnew.B_Cu, "LOGIC_GND"),
    (pcbnew.F_Cu, "LOGIC_GND_F"),
):
    logic_ground = pcbnew.ZONE(board)
    logic_ground.SetLayer(layer)
    logic_ground.SetNetCode(board.GetNetsByName()["GND"].GetNetCode())
    logic_ground.SetZoneName(name)
    logic_ground.SetLocalClearance(pcbnew.FromMM(0.3))
    logic_ground.SetMinThickness(pcbnew.FromMM(0.25))
    logic_ground.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
    logic_ground.SetThermalReliefGap(pcbnew.FromMM(0.3))
    logic_ground.SetThermalReliefSpokeWidth(pcbnew.FromMM(0.3))
    logic_ground.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
    outline = logic_ground.Outline()
    outline.NewOutline()
    for xy in logic_ground_polygon:
        outline.Append(point(xy))
    board.Add(logic_ground)
    logic_ground_zones.append({
        "name": name,
        "layer": board.GetLayerName(layer),
        "polygon_mm": logic_ground_polygon,
    })

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "pico-lcd-routing",
    "routes": routes,
    "vias": vias,
    "ground_zones": logic_ground_zones,
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
