"""Route the validated left-side power stage without touching the Pico area."""

import argparse
import hashlib
import json
from pathlib import Path

import pcbnew


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--input', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--report', type=Path)
args = parser.parse_args()

SOURCE = str(args.input)
OUTPUT = str(args.output)
REPORT = args.report

board = pcbnew.LoadBoard(SOURCE)
if len(board.GetTracks()):
    raise ValueError('Routing input must not already contain tracks or vias')
footprints = {fp.GetReference(): fp for fp in board.GetFootprints()}


def pad(ref, number):
    position = footprints[ref].FindPadByNumber(number).GetPosition()
    return [round(pcbnew.ToMM(position.x), 3), round(pcbnew.ToMM(position.y), 3)]


def point(xy):
    return pcbnew.VECTOR2I(pcbnew.FromMM(xy[0]), pcbnew.FromMM(xy[1]))


def add_segment(net_name, layer_name, width_mm, start, end):
    item = pcbnew.PCB_TRACK(board)
    item.SetStart(point(start))
    item.SetEnd(point(end))
    item.SetWidth(pcbnew.FromMM(width_mm))
    item.SetLayer(pcbnew.F_Cu if layer_name == 'F.Cu' else pcbnew.B_Cu)
    item.SetNet(board.GetNetsByName()[net_name])
    board.Add(item)


def add_via(net_name, position):
    item = pcbnew.PCB_VIA(board)
    item.SetPosition(point(position))
    item.SetWidth(pcbnew.F_Cu, pcbnew.FromMM(0.6))
    item.SetWidth(pcbnew.B_Cu, pcbnew.FromMM(0.6))
    item.SetDrill(pcbnew.FromMM(0.3))
    item.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
    item.SetNet(board.GetNetsByName()[net_name])
    board.Add(item)


routes = [
    # Hot boost loop: keep the switch node compact and entirely on the front.
    ('switch-power-loop', 'SW_NODE', 'F.Cu', 1.0,
     [pad('L1', '2'), [12.55, 20.79], pad('D4', '2'), [10.3, 22.6],
      [10.3, 24.845], pad('U3', '1')]),
    ('switch-charge-pump', 'SW_NODE', 'F.Cu', 0.6,
     [pad('U3', '1'), [10.0, 24.845], [9.5, 26.0], [9.5, 28.0],
      [10.0, 29.5], [10.0, 35.5], pad('C9', '1')]),
    ('peak-current-path', 'IPK_SENSE', 'F.Cu', 1.0,
     [pad('L1', '1'), pad('R7', '2'), [18.3, 26.115], pad('U3', '7')]),
    ('boost-output-hot-loop', '+13V8', 'F.Cu', 1.0,
     [pad('D4', '1'), pad('C8', '1')]),

    # 5 V distribution.  The regulator pin changes layer at a dedicated via.
    ('usb-input-trunk', '+5V', 'B.Cu', 1.0,
     [pad('J2', '1'), [8.0, 25.0], [13.5, 25.0]]),
    ('input-capacitor', '+5V', 'B.Cu', 1.0,
     [[13.5, 25.0], [14.5, 26.5], [14.5, 27.3], [9.5, 27.3],
      [9.5, 38.0], [12.0, 39.0], pad('C6', '1')]),
    ('input-current-limit', '+5V', 'B.Cu', 1.0,
     [[13.5, 25.0], [11.5, 25.0], [11.5, 19.0], [24.5, 19.0], pad('R7', '1')]),
    ('input-regulator-vcc', '+5V', 'B.Cu', 1.0,
     [[14.5, 26.5], [14.5, 27.385], [18.0, 27.385]]),
    ('input-regulator-vcc-neck', '+5V', 'F.Cu', 0.6,
     [[18.0, 27.385], pad('U3', '6')]),
    ('input-regulator-vcc-via-pair-front', '+5V', 'F.Cu', 0.6,
     [[18.0, 27.385], [17.2, 27.385]]),
    ('driver-bias', '+5V', 'B.Cu', 0.3,
     [pad('R7', '1'), [25.8, 22.75], [25.8, 30.8], pad('R8', '1')]),
    ('input-test-point-back', '+5V', 'B.Cu', 0.6,
     [pad('C6', '1'), [12.0, 40.0], [9.5, 47.0]]),
    ('input-test-point-front', '+5V', 'F.Cu', 0.6,
     [[9.5, 47.0], pad('TP1', '1')]),

    # Positive output distribution, away from the switch node on the back.
    ('positive-output-terminal', '+13V8', 'B.Cu', 1.0,
     [pad('C8', '1'), [5.3, 18.5], [5.3, 38.75], pad('J3', '1')]),
    ('positive-output-test', '+13V8', 'F.Cu', 1.0,
     [pad('J3', '1'), [4.5, 43.5], [12.75, 43.5], pad('TP2', '1')]),
    ('positive-output-sense-front', '+13V8', 'F.Cu', 1.0,
     [pad('D4', '1'), [7.0, 16.0], [7.0, 11.0], [18.0, 11.0], [18.0, 20.5]]),
    ('positive-output-sense-back-a', '+13V8', 'B.Cu', 1.0,
     [[18.0, 20.5], [22.5, 20.5], [22.5, 24.5], [23.0, 24.5]]),
    ('positive-output-sense-bridge', '+13V8', 'F.Cu', 1.0,
     [[23.0, 24.5], [23.0, 27.0]]),
    ('positive-output-sense-back-b', '+13V8', 'B.Cu', 1.0,
     [[23.0, 27.0], [23.0, 32.5], pad('R9', '1')]),
    ('positive-indicator-feed', '+13V8', 'F.Cu', 1.0,
     [pad('C8', '1'), pad('R11', '1')]),

    # Quiet control traces remain on the front and avoid the power loops.
    ('timing-control', 'TC_TIMING', 'F.Cu', 0.25,
     [pad('U3', '3'), [12.8, 27.385], [12.8, 31.5], pad('C7', '1')]),
    ('feedback-upper', 'VFB', 'F.Cu', 0.25,
     [pad('U3', '5'), [17.8, 29.5], pad('R9', '2')]),
    ('feedback-lower', 'VFB', 'F.Cu', 0.25,
     [pad('R9', '2'), [20.0, 34.0], [20.0, 36.0], pad('R10', '1')]),
    ('driver-discharge-front', 'DRIVER_DC', 'F.Cu', 0.3,
     [pad('U3', '8'), [17.5, 24.845]]),
    ('driver-discharge-back', 'DRIVER_DC', 'B.Cu', 0.3,
     [[17.5, 24.845], [18.5, 25.5], [24.0, 25.5], [24.0, 33.34], pad('R8', '2')]),

    # Charge pump and negative output.
    ('charge-pump-clamp', 'CPUMP_MID', 'F.Cu', 0.6,
     [pad('C9', '2'), pad('D6', '2')]),
    ('charge-pump-rectifier', 'CPUMP_MID', 'F.Cu', 0.6,
     [pad('C9', '2'), [11.0, 37.5], [14.5, 35.5], pad('D7', '2')]),
    ('negative-output-capacitor', '-13V8', 'B.Cu', 1.0,
     [pad('D7', '1'), [15.75, 29.0], pad('C11', '2')]),
    ('negative-indicator-feed', '-13V8', 'F.Cu', 1.0,
     [pad('C11', '2'), [24.8, 29.0], [26.45, 27.35], [26.45, 11.6],
      [23.4, 11.6], [23.4, 10.8], pad('D8', '1')]),
    ('negative-test-point', '-13V8', 'F.Cu', 1.0,
     [pad('C11', '2'), [23.5, 29.0], [23.5, 43.0], [16.0, 46.0], pad('TP3', '1')]),
    ('negative-led', 'NEG_LED_N', 'F.Cu', 0.3,
     [pad('D8', '2'), pad('R16', '1')]),
    ('positive-led', 'PWR_LED_P', 'F.Cu', 0.3,
     [pad('R11', '2'), pad('D5', '2')]),
    ('ground-test-point-back', 'GND', 'B.Cu', 0.6,
     [pad('C6', '2'), [21.0, 44.5], [19.25, 47.0]]),
    ('ground-test-point-front', 'GND', 'F.Cu', 0.6,
     [[19.25, 47.0], pad('TP4', '1')]),

]

vias = [
    ('regulator-vcc-via', '+5V', [18.0, 27.385]),
    ('regulator-vcc-via-pair', '+5V', [17.2, 27.385]),
    ('driver-via', 'DRIVER_DC', [17.5, 24.845]),
    ('positive-sense-via-a', '+13V8', [18.0, 20.5]),
    ('positive-sense-via-b', '+13V8', [23.0, 24.5]),
    ('positive-sense-via-c', '+13V8', [23.0, 27.0]),
    ('input-test-via', '+5V', [9.5, 47.0]),
    ('ground-test-via', 'GND', [19.25, 47.0]),
    ('power-ground-via', 'GND', pad('U3', '2')),
    ('quiet-ground-via', 'GND', pad('U3', '4')),
    ('c11-ground-stitch', 'GND', [19.8, 27.0]),
]

for _, net_name, layer_name, width_mm, points in routes:
    for start, end in zip(points, points[1:]):
        add_segment(net_name, layer_name, width_mm, start, end)

for _, net_name, position in vias:
    add_via(net_name, position)

# Rotate C11's thermal spokes so the widened +13 V sense route still leaves
# two B.Cu spokes, while retaining the 0.30 mm thermal gap and spoke width.
footprints['C11'].FindPadByNumber('1').SetThermalSpokeAngleDegrees(45.0)

# Local two-layer ground plane: it ends before J1 and therefore does not enter
# the Pico/LCD area.  Pads use thermal spokes; vias remain direct connections.
power_ground_polygon = [[0.6, 0.6], [24.9, 0.6], [24.9, 51.4], [0.6, 51.4]]
power_ground_zones = []
for layer, name in (
    (pcbnew.B_Cu, 'POWER_STAGE_GND'),
    (pcbnew.F_Cu, 'POWER_STAGE_GND_F'),
):
    zone = pcbnew.ZONE(board)
    zone.SetLayer(layer)
    zone.SetNetCode(board.GetNetsByName()['GND'].GetNetCode())
    zone.SetZoneName(name)
    zone.SetLocalClearance(pcbnew.FromMM(0.3))
    zone.SetMinThickness(pcbnew.FromMM(0.25))
    zone.SetPadConnection(pcbnew.ZONE_CONNECTION_THERMAL)
    zone.SetThermalReliefGap(pcbnew.FromMM(0.3))
    zone.SetThermalReliefSpokeWidth(pcbnew.FromMM(0.3))
    zone.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
    outline = zone.Outline()
    outline.NewOutline()
    for xy in power_ground_polygon:
        outline.Append(point(xy))
    board.Add(zone)
    power_ground_zones.append({
        'name': name,
        'layer': board.GetLayerName(layer),
        'polygon_mm': power_ground_polygon,
    })

pcbnew.ZONE_FILLER(board).Fill(board.Zones())
args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(OUTPUT, board)

report = {
    'source': SOURCE,
    'output': OUTPUT,
    'input_sha256': hashlib.sha256(args.input.read_bytes()).hexdigest(),
    'scope': 'power-stage-only',
    'ground_zones': power_ground_zones,
    'routes': [
        {'name': name, 'net': net, 'layer': layer, 'width_mm': width, 'points_mm': points}
        for name, net, layer, width, points in routes
    ],
    'vias': [
        {'name': name, 'net': net, 'position_mm': position}
        for name, net, position in vias
    ],
}
if REPORT:
    REPORT.parent.mkdir(parents=True, exist_ok=True)
    with REPORT.open('w', encoding='utf-8') as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
