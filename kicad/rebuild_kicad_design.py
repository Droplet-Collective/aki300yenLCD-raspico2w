from __future__ import annotations

import math
import re
import sys
import uuid
import heapq
from array import array
from pathlib import Path


ROOT = Path(__file__).resolve().parent
SRC_SCH = ROOT / "pico_lta042b010f_carrier.kicad_sch"
SRC_PCB = ROOT / "pico_lta042b010f_carrier.kicad_pcb"
SRC_PRO = ROOT / "pico_lta042b010f_carrier.kicad_pro"
OUT = Path(r"C:\Users\choco\AppData\Local\Temp\kicad-reviewed-candidate")
KI_SYMBOLS = Path(r"C:\Program Files\KiCad\9.0\share\kicad\symbols")
KI_FOOTPRINTS = Path(r"C:\Program Files\KiCad\9.0\share\kicad\footprints")
PROJECT_FOOTPRINTS = ROOT / "footprints.pretty"

# Keep electrical coordinates on the original 1.27 mm grid and translate
# only the plotted sheet objects onto an A4 landscape page at the end.
SHEET_OFFSET_X = -60.96
SHEET_OFFSET_Y = -60.96


def uid() -> str:
    return str(uuid.uuid4())


def fmt(value: float) -> str:
    if abs(value) < 1e-9:
        value = 0.0
    return f"{value:.3f}".rstrip("0").rstrip(".")


def grid(value: float) -> float:
    return round(value / 1.27) * 1.27


def find_balanced(text: str, start: int) -> int:
    if text[start] != "(":
        raise ValueError(f"expected '(' at {start}")
    depth = 0
    quoted = False
    escaped = False
    for index in range(start, len(text)):
        char = text[index]
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
            continue
        if char == '"':
            quoted = True
        elif char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
            if depth == 0:
                return index + 1
    raise ValueError("unbalanced s-expression")


def extract_symbol(text: str, name: str) -> str:
    marker = f'(symbol "{name}"'
    start = text.find(marker)
    if start < 0:
        raise ValueError(f"symbol {name!r} not found")
    return text[start:find_balanced(text, start)]


def tune_pico_symbol_visuals(symbol: str) -> str:
    """Improve Pico pin-name legibility without changing pin endpoints.

    The custom Pico symbol uses 2.54 mm rows with long names on both sides of
    a narrow body.  Keep every pin's connection coordinate and electrical
    metadata unchanged, but widen the body, shorten the graphic pin line to
    the new body edge, and use the same readable 0.8 mm pin-name size as the
    LCD connector symbol.
    """
    symbol = symbol.replace(
        '(rectangle (start -5.08 -26.67) (end 5.08 26.67)',
        '(rectangle (start -6.35 -26.67) (end 6.35 26.67)',
        1,
    )
    symbol = symbol.replace('(length 2.54)', '(length 1.27)')
    symbol = re.sub(
        r'(\(name "[^"]+" \(effects \(font \(size )1 1',
        r'\g<1>0.8 0.8',
        symbol,
    )
    return symbol


AKIZUKI_COMPONENT_FIELDS: dict[str, dict[str, str]] = {
    "R7": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MFU100F0R47B",
        "AkizukiCode": "108800",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g108800/",
        "SelectionNote": "0.47ohm +/-1%; 1W; axial through-hole; vertical mounting",
    },
    "R8": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MFS25F200RB",
        "AkizukiCode": "108526",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g108526/",
        "SelectionNote": "200ohm +/-1%; 1/4W; compact axial through-hole; vertical mounting",
    },
    "R9": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MF25B10KBD",
        "AkizukiCode": "116877",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g116877/",
        "SelectionNote": "10kohm +/-0.1%; 1/4W; axial through-hole; vertical mounting",
    },
    "R10": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MF25B1KBD",
        "AkizukiCode": "116876",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g116876/",
        "SelectionNote": "1kohm +/-0.1%; 1/4W; axial through-hole; vertical mounting",
    },
    "R11": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MF25B10KBD",
        "AkizukiCode": "116877",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g116877/",
        "SelectionNote": "10kohm +/-0.1%; 1/4W; axial through-hole; vertical mounting",
    },
    "R16": {
        "Manufacturer": "FAITHFUL LINK INDUSTRIAL CORP.",
        "MPN": "MF25B10KBD",
        "AkizukiCode": "116877",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g116877/",
        "SelectionNote": "10kohm +/-0.1%; 1/4W; axial through-hole; vertical mounting",
    },
    "L1": {
        "Manufacturer": "Taiyo Yuden",
        "MPN": "NR10050T101M",
        "AkizukiCode": "108325",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g108325/",
        "SelectionNote": "100uH; hand-solder footprint; linked product",
    },
    "C6": {
        "Manufacturer": "Rubycon",
        "MPN": "35ZLH220MEFCCT8X11.5",
        "AkizukiCode": "111758",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g111758/",
        "SelectionNote": "220uF 35V; linked item sold out; current equivalent candidate 102718",
    },
    "C8": {
        "Manufacturer": "Rubycon",
        "MPN": "35PX47MEFC5X11",
        "AkizukiCode": "117887",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117887/",
        "SelectionNote": "47uF 35V; radial through-hole",
    },
    "C7": {
        "Manufacturer": "Murata",
        "MPN": "RDE5C1H471J0P1H03B",
        "AkizukiCode": "131236",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g131236/",
        "SelectionNote": "470pF 50V C0G +/-5%; radial through-hole; 2.5mm pitch",
    },
    "C9": {
        "Manufacturer": "Rubycon",
        "MPN": "50PX10MEFC5X11",
        "AkizukiCode": "117897",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117897/",
        "SelectionNote": "10uF 50V 105C; radial through-hole",
    },
    "C11": {
        "Manufacturer": "Rubycon",
        "MPN": "50PX10MEFC5X11",
        "AkizukiCode": "117897",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117897/",
        "SelectionNote": "10uF 50V 105C; radial through-hole; positive terminal to GND",
    },
    "D4": {
        "Manufacturer": "WUXI XUYANG ELECTRONICS",
        "MPN": "1N5819",
        "AkizukiCode": "117244",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117244/",
        "SelectionNote": "40V 1A Schottky; DO-41 through-hole; vertical mounting",
    },
    "D6": {
        "Manufacturer": "WUXI XUYANG ELECTRONICS",
        "MPN": "1N5819",
        "AkizukiCode": "117244",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117244/",
        "SelectionNote": "40V 1A Schottky; DO-41 through-hole; vertical mounting",
    },
    "D7": {
        "Manufacturer": "WUXI XUYANG ELECTRONICS",
        "MPN": "1N5819",
        "AkizukiCode": "117244",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117244/",
        "SelectionNote": "40V 1A Schottky; DO-41 through-hole; vertical mounting",
    },
    "D5": {
        "Manufacturer": "OptoSupply",
        "MPN": "OSG5TA3Z74A",
        "AkizukiCode": "111635",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g111635/",
        "SelectionNote": "green 3mm bullet LED; radial through-hole; 2.54mm pitch",
    },
    "D8": {
        "Manufacturer": "OptoSupply",
        "MPN": "OSR5JA3Z74A",
        "AkizukiCode": "111577",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g111577/",
        "SelectionNote": "red 3mm bullet LED; radial through-hole; 2.54mm pitch",
    },
    "RV1": {
        "Manufacturer": "SUNTAN TECHNOLOGY CO LTD",
        "MPN": "TSR-065-103-R",
        "AkizukiCode": "106063",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g106063/",
        "SelectionNote": "10kohm +/-30%; 0.1W; top-adjust single-turn trimmer; RM-065 through-hole footprint",
    },
    "J2": {
        "Manufacturer": "Chang Enn Co., Ltd.",
        "MPN": "A295-CTRPB-1",
        "AkizukiCode": "116895",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g116895/",
        "SelectionNote": "compact power-only USB Type-C receptacle; through-hole; no CC contacts; use a USB-A source with an A-to-C cable",
    },
    "J3": {
        "Manufacturer": "Chang Enn",
        "MPN": "PH-1X2SG",
        "AkizukiCode": "108593",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g108593/",
        "SelectionNote": "straight 1x2 2.54mm pin header; through-hole",
    },
    "U3": {
        "Manufacturer": "TAEJIN TECHNOLOGY",
        "MPN": "MC34063AD",
        "AkizukiCode": "117573",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g117573/",
        "SelectionNote": "SOP-8; hand-solderable 1.27mm pitch",
    },
    "U1": {
        "Manufacturer": "Raspberry Pi Foundation",
        "MPN": "SC1633",
        "AkizukiCode": "130330",
        "Supplier": "Akizuki Denshi",
        "SourceURL": "https://akizukidenshi.com/catalog/g/g130330/",
        "SelectionNote": "Raspberry Pi Pico 2 W module",
    },
}


def set_component_fields(block: str, reference: str) -> str:
    """Attach procurement metadata without displaying it on the schematic."""
    fields = AKIZUKI_COMPONENT_FIELDS.get(reference)
    if not fields:
        return block
    for name, value in fields.items():
        value = value.replace('\\', '\\\\').replace('"', '\\"')
        marker = f'(property "{name}"'
        if marker in block:
            block = replace_property(block, name, value)
            continue
        description_start = block.find('(property "Description"')
        if description_start < 0:
            insertion = block.find('\n\t\t(pin ')
            if insertion < 0:
                insertion = block.rfind('\n')
        else:
            insertion = block.rfind('\n', 0, description_start) + 1
        property_text = (
            f'\t\t(property "{name}" "{value}" '
            '(at 0 0 0) (effects (font (size 1.27 1.27)) hide))\n'
        )
        block = block[:insertion] + property_text + block[insertion:]
    return block


def qualify_symbol(block: str, library: str, name: str) -> str:
    return block.replace(f'(symbol "{name}"', f'(symbol "{library}:{name}"', 1)


def top_level_blocks(text: str) -> list[str]:
    blocks: list[str] = []
    index = 0
    while index < len(text):
        if text[index] != "(":
            index += 1
            continue
        end = find_balanced(text, index)
        blocks.append(text[index:end])
        index = end
    return blocks


def round_tail_coordinates(text: str) -> str:
    pattern = re.compile(
        r'(?P<prefix>\((?:at|xy)\s+)'
        r'(?P<x>-?\d+(?:\.\d+)?)\s+'
        r'(?P<y>-?\d+(?:\.\d+)?)(?P<rest>[^\r\n\)]*)'
    )

    def replace(match: re.Match[str]) -> str:
        x = grid(float(match.group("x")))
        y = grid(float(match.group("y")))
        return f"{match.group('prefix')}{fmt(x)} {fmt(y)}{match.group('rest')}"

    return pattern.sub(replace, text)


def get_reference(block: str) -> str | None:
    match = re.search(r'\(property "Reference" "([^"]+)"', block)
    return match.group(1) if match else None


def get_first_at(block: str) -> tuple[float, float, float] | None:
    match = re.search(
        r'\(at\s+(-?\d+(?:\.\d+)?)\s+(-?\d+(?:\.\d+)?)(?:\s+(-?\d+(?:\.\d+)?))?\)',
        block,
    )
    if not match:
        return None
    return float(match.group(1)), float(match.group(2)), float(match.group(3) or 0)


def replace_root_at(block: str, x: float, y: float, rotation: float = 0) -> str:
    pattern = re.compile(
        r'\(at\s+-?\d+(?:\.\d+)?\s+-?\d+(?:\.\d+)?(?:\s+-?\d+(?:\.\d+)?)?\)'
    )
    return pattern.sub(f"(at {fmt(x)} {fmt(y)} {fmt(rotation)})", block, count=1)


def replace_property(block: str, name: str, value: str) -> str:
    pattern = re.compile(rf'(\(property "{re.escape(name)}" )"[^"]*"')
    return pattern.sub(lambda match: f'{match.group(1)}"{value}"', block, count=1)


def absolute_at(local_at: str, x: float, y: float, rotation: float = 0) -> str:
    values = [float(value) for value in local_at.split()]
    lx, ly = values[0], values[1]
    local_rotation = values[2] if len(values) > 2 else 0.0
    radians = math.radians(rotation)
    absolute_x = x + lx * math.cos(radians) - ly * math.sin(radians)
    absolute_y = y + lx * math.sin(radians) + ly * math.cos(radians)
    return f"{fmt(absolute_x)} {fmt(absolute_y)} {fmt(local_rotation + rotation)}"


def absolute_visible_properties(block: str, x: float, y: float, rotation: float) -> str:
    for property_name in ("Reference", "Value"):
        property_start = block.find(f'(property "{property_name}"')
        if property_start < 0:
            continue
        property_end = find_balanced(block, property_start)
        property_block = block[property_start:property_end]
        at_match = re.search(r'\(at\s+(-?\d+(?:\.\d+)?)\s+(-?\d+(?:\.\d+)?)(?:\s+(-?\d+(?:\.\d+)?))?\)', property_block)
        if not at_match:
            continue
        local_at = " ".join(value for value in at_match.groups() if value is not None)
        replacement = f"(at {absolute_at(local_at, x, y, rotation)})"
        property_block = property_block[:at_match.start()] + replacement + property_block[at_match.end():]
        block = block[:property_start] + property_block + block[property_end:]
    return block


# Explicit field placement keeps annotations readable even when a component
# is rotated for the power path.  Displayed fields remain horizontal and are
# placed away from the wire carrying the component pins.
FIELD_LAYOUTS: dict[str, dict[str, tuple[float, float, float]]] = {
    "U1": {"Reference": (179.07, 133.35, 0), "Value": (179.07, 199.39, 0)},
    "J1": {"Reference": (316.88, 119.38, 0), "Value": (316.88, 220.98, 0)},
    "J2": {"Reference": (139.70, 90.17, 0), "Value": (146.05, 119.38, 0)},
    "J3": {"Reference": (279.40, 148.59, 0), "Value": (279.40, 161.29, 0)},
    "U3": {"Reference": (168.91, 88.90, 0), "Value": (179.07, 116.84, 0)},
    "R7": {"Reference": (149.86, 77.47, 0), "Value": (149.86, 74.93, 0)},
    "L1": {"Reference": (163.83, 77.47, 0), "Value": (163.83, 74.93, 0)},
    "D4": {"Reference": (180.34, 77.47, 0), "Value": (180.34, 74.93, 0)},
    "R8": {"Reference": (149.86, 95.25, 0), "Value": (149.86, 92.71, 0)},
    "C6": {"Reference": (124.46, 105.41, 0), "Value": (130.81, 113.03, 0)},
    "C7": {"Reference": (154.94, 112.00, 0), "Value": (154.94, 116.00, 0)},
    "R9": {"Reference": (214.63, 91.44, 0), "Value": (214.63, 96.52, 0)},
    "R10": {"Reference": (202.00, 113.03, 0), "Value": (202.00, 118.11, 0)},
    "C8": {"Reference": (185.42, 91.44, 0), "Value": (185.42, 96.52, 0)},
    "R11": {"Reference": (230.00, 91.44, 0), "Value": (230.00, 96.52, 0)},
    "D5": {"Reference": (230.00, 103.00, 0), "Value": (234.95, 108.00, 0)},
    "C9": {"Reference": (248.00, 92.71, 0), "Value": (248.00, 99.06, 0)},
    "D7": {"Reference": (260.35, 99.06, 0), "Value": (260.35, 96.52, 0)},
    "D6": {"Reference": (253.00, 111.76, 0), "Value": (253.00, 116.84, 0)},
    "C11": {"Reference": (271.78, 111.76, 0), "Value": (271.78, 116.84, 0)},
    "D8": {"Reference": (292.00, 111.76, 0), "Value": (292.00, 116.84, 0)},
    "R16": {"Reference": (292.00, 123.19, 0), "Value": (292.00, 130.81, 0)},
    "RV1": {"Reference": (287.02, 173.99, 0), "Value": (279.40, 195.58, 0)},
}


def set_property_position(
    block: str, property_name: str, x: float, y: float, rotation: float = 0
) -> str:
    property_start = block.find(f'(property "{property_name}"')
    if property_start < 0:
        return block
    property_end = find_balanced(block, property_start)
    property_block = block[property_start:property_end]
    at_match = re.search(
        r'\(at\s+-?\d+(?:\.\d+)?\s+-?\d+(?:\.\d+)?(?:\s+-?\d+(?:\.\d+)?)?\)',
        property_block,
    )
    if not at_match:
        return block
    replacement = f"(at {fmt(x)} {fmt(y)} {fmt(rotation)})"
    property_block = (
        property_block[:at_match.start()]
        + replacement
        + property_block[at_match.end():]
    )
    return block[:property_start] + property_block + block[property_end:]


def apply_field_layout(block: str, reference: str, symbol_rotation: float = 0) -> str:
    for property_name, coordinates in FIELD_LAYOUTS.get(reference, {}).items():
        x, y, layout_rotation = coordinates
        # Symbol properties inherit the instance rotation in KiCad.  Apply
        # the inverse here so every displayed annotation remains horizontal.
        if round(symbol_rotation) % 360 == 180:
            # KiCad already keeps text upright for a 180-degree symbol; do
            # not apply a second half-turn to these fields.
            field_rotation = layout_rotation
        else:
            field_rotation = (layout_rotation - symbol_rotation) % 360
        block = set_property_position(block, property_name, x, y, field_rotation)
    return block


def properties_are_sheet_absolute(block: str) -> bool:
    """Detect an already-reviewed instance so regeneration is idempotent."""
    for property_name in ("Reference", "Value"):
        property_start = block.find(f'(property "{property_name}"')
        if property_start < 0:
            continue
        property_end = find_balanced(block, property_start)
        property_block = block[property_start:property_end]
        at_match = re.search(
            r'\(at\s+(-?\d+(?:\.\d+)?)\s+(-?\d+(?:\.\d+)?)', property_block
        )
        if at_match and (abs(float(at_match.group(1))) > 50 or abs(float(at_match.group(2))) > 50):
            return True
    return False


def replace_lib_id(block: str, lib_id: str) -> str:
    return re.sub(r'\(lib_id "[^"]+"\)', f'(lib_id "{lib_id}")', block, count=1)


def instance_from_old(
    old: dict[str, str],
    ref: str,
    lib_id: str,
    footprint: str,
    position: tuple[float, float],
    rotation: float,
    value: str | None = None,
) -> str:
    block = replace_lib_id(old[ref], lib_id)
    block = replace_root_at(block, position[0], position[1], rotation)
    # KiCad's root-sheet symbol instance path is the schematic UUID, not the
    # sheet_instances display path "/".  Using the latter makes the exporter
    # render a second copy of each symbol's default fields at the page origin.
    block = block.replace('(path "/"', '(path "/10000000-0000-4000-8000-000000000001"', 1)
    block = replace_property(block, "Footprint", footprint)
    if value is not None:
        block = replace_property(block, "Value", value)
    if not properties_are_sheet_absolute(block):
        block = absolute_visible_properties(block, position[0], position[1], rotation)
    block = apply_field_layout(block, ref, rotation)
    return set_component_fields(block, ref)


def clone_instance_from_old(
    old: dict[str, str],
    source_ref: str,
    ref: str,
    lib_id: str,
    footprint: str,
    position: tuple[float, float],
    rotation: float,
    value: str,
) -> str:
    """Clone a like-for-like symbol while assigning fresh UUIDs and reference."""
    block = old[source_ref]
    block = re.sub(
        r'\(uuid "[^"]+"\)',
        lambda _: f'(uuid "{uid()}")',
        block,
    )
    block = replace_property(block, "Reference", ref)
    block = re.sub(
        r'\(reference "[^"]+"\)',
        f'(reference "{ref}")',
        block,
        count=1,
    )
    return instance_from_old(
        {ref: block}, ref, lib_id, footprint, position, rotation, value
    )


def expand_connector_pins(block: str, count: int) -> str:
    """Grow a cloned generic connector instance to the requested pin count."""
    existing = {
        int(number)
        for number in re.findall(r'\(pin "(\d+)" \(uuid "[^"]+"\)\)', block)
    }
    insertion = block.find("(instances")
    if insertion < 0:
        raise ValueError("connector instance list not found")
    additions = "".join(
        f'\t\t(pin "{number}" (uuid "{uid()}"))\n'
        for number in range(1, count + 1)
        if number not in existing
    )
    return block[:insertion] + additions + block[insertion:]


def resize_connector_pins(block: str, count: int) -> str:
    """Remove obsolete generic-connector pins, then add any missing pins."""
    block = re.sub(
        r'\s*\(pin "(\d+)" \(uuid "[^"]+"\)\)',
        lambda match: "" if int(match.group(1)) > count else match.group(0),
        block,
    )
    return expand_connector_pins(block, count)


def no_connect(x: float, y: float) -> str:
    return f'''\t(no_connect
\t\t(at {fmt(x)} {fmt(y)})
\t\t(uuid "{uid()}")
\t)'''


def wire(x1: float, y1: float, x2: float, y2: float) -> str:
    if abs(x1 - x2) < 1e-9 and abs(y1 - y2) < 1e-9:
        return ""
    return f'''\t(wire
\t\t(pts
\t\t\t(xy {fmt(x1)} {fmt(y1)}) (xy {fmt(x2)} {fmt(y2)})
\t\t)
\t\t(stroke (width 0) (type solid))
\t\t(uuid "{uid()}")
\t)'''


def bus(x1: float, y1: float, x2: float, y2: float) -> str:
    if abs(x1 - x2) < 1e-9 and abs(y1 - y2) < 1e-9:
        return ""
    return f'''\t(bus
\t\t(pts
\t\t\t(xy {fmt(x1)} {fmt(y1)}) (xy {fmt(x2)} {fmt(y2)})
\t\t)
\t\t(stroke (width 0) (type default))
\t\t(uuid "{uid()}")
\t)'''


def bus_entry(x: float, y: float, size_x: float, size_y: float) -> str:
    return f'''\t(bus_entry
\t\t(at {fmt(x)} {fmt(y)})
\t\t(size {fmt(size_x)} {fmt(size_y)})
\t\t(stroke (width 0) (type default))
\t\t(uuid "{uid()}")
\t)'''


def visible_label(
    name: str,
    x: float,
    y: float,
    rotation: float = 0,
    size: float = 0.9,
) -> str:
    justify = "right bottom" if abs(rotation - 180) < 1e-9 else "left bottom"
    return f'''\t(label "{name}"
\t\t(at {fmt(x)} {fmt(y)} {fmt(rotation)})
\t\t(effects (font (size {fmt(size)} {fmt(size)})) (justify {justify}))
\t\t(uuid "{uid()}")
\t)'''


def bus_label(name: str, x: float, y: float, rotation: float = 0) -> str:
    return visible_label(name, x, y, rotation, size=0.9)


def translate_sheet_block(block: str, dx: float, dy: float) -> str:
    """Translate plotted sheet coordinates while leaving local fields at 0,0."""
    pattern = re.compile(
        r'(?P<prefix>\((?:at|xy)\s+)'
        r'(?P<x>-?\d+(?:\.\d+)?)\s+'
        r'(?P<y>-?\d+(?:\.\d+)?)(?P<rest>[^\r\n\)]*)'
    )

    def replace(match: re.Match[str]) -> str:
        x = float(match.group("x"))
        y = float(match.group("y"))
        # Instance metadata uses local (0, 0) coordinates and must not move.
        if abs(x) < 50 and abs(y) < 50:
            return match.group(0)
        return (
            f'{match.group("prefix")}{fmt(x + dx)} {fmt(y + dy)}'
            f'{match.group("rest")}'
        )

    return pattern.sub(replace, block)


def junction(x: float, y: float) -> str:
    return f'''\t(junction
\t\t(at {fmt(x)} {fmt(y)})
\t\t(diameter 1.016)
\t\t(color 0 0 0 0)
\t\t(uuid "{uid()}")
\t)'''


def parse_wire_segments(wire_blocks: list[str]) -> list[tuple[float, float, float, float]]:
    segments: list[tuple[float, float, float, float]] = []
    point_pattern = re.compile(r'\(xy\s+(-?\d+(?:\.\d+)?)\s+(-?\d+(?:\.\d+)?)\)')
    for block in wire_blocks:
        points = point_pattern.findall(block)
        if len(points) != 2:
            continue
        (x1, y1), (x2, y2) = ((float(x), float(y)) for x, y in points)
        if abs(x1 - x2) < 1e-9 and abs(y1 - y2) < 1e-9:
            continue
        segments.append((x1, y1, x2, y2))
    return segments


def point_on_segment_interior(
    x: float,
    y: float,
    segment: tuple[float, float, float, float],
    epsilon: float = 1e-6,
) -> bool:
    x1, y1, x2, y2 = segment
    cross = (x - x1) * (y2 - y1) - (y - y1) * (x2 - x1)
    if abs(cross) > epsilon:
        return False
    if abs(x1 - x2) >= abs(y1 - y2):
        return min(x1, x2) + epsilon < x < max(x1, x2) - epsilon and abs(y - y1) <= epsilon
    return min(y1, y2) + epsilon < y < max(y1, y2) - epsilon and abs(x - x1) <= epsilon


def junction_wire_profile(
    x: float,
    y: float,
    segments: list[tuple[float, float, float, float]],
    epsilon: float = 1e-6,
) -> tuple[int, int]:
    endpoints = 0
    interiors = 0
    for x1, y1, x2, y2 in segments:
        at_endpoint = (abs(x - x1) <= epsilon and abs(y - y1) <= epsilon) or (
            abs(x - x2) <= epsilon and abs(y - y2) <= epsilon
        )
        if at_endpoint:
            endpoints += 1
        elif point_on_segment_interior(x, y, (x1, y1, x2, y2), epsilon):
            interiors += 1
    return endpoints, interiors


def junction_is_structurally_needed(
    endpoints: int, interiors: int, pin_endpoint: bool = False
) -> bool:
    """Keep wire-wire branches, never a dot that only terminates at a pin."""
    if pin_endpoint:
        return False
    return interiors >= 1 and (endpoints >= 1 or interiors >= 2)


def symbol_pin_local_points(symbol_block: str) -> list[tuple[float, float]]:
    points: list[tuple[float, float]] = []
    for match in re.finditer(r'\(pin\s+', symbol_block):
        pin_start = match.start()
        pin_block = symbol_block[pin_start:find_balanced(symbol_block, pin_start)]
        at = get_first_at(pin_block)
        if at:
            points.append((at[0], at[1]))
    return points


def absolute_pin_at(
    local_x: float, local_y: float, x: float, y: float, rotation: float
) -> tuple[float, float]:
    """Convert KiCad symbol-local pin coordinates to sheet coordinates.

    Symbol libraries use a y-up local coordinate system while schematic sheet
    coordinates are y-down.  This is intentionally separate from
    ``absolute_at``, which is used for property text positions.
    """
    radians = math.radians(rotation)
    absolute_x = x + local_x * math.cos(radians) - local_y * math.sin(radians)
    absolute_y = y - (local_x * math.sin(radians) + local_y * math.cos(radians))
    return absolute_x, absolute_y


def collect_pin_endpoints(symbol_library: str, instances: list[str]) -> set[tuple[float, float]]:
    endpoints: set[tuple[float, float]] = set()
    for instance in instances:
        lib_match = re.search(r'\(lib_id "([^"]+)"\)', instance)
        at = get_first_at(instance)
        if not lib_match or not at:
            continue
        try:
            symbol_block = extract_symbol(symbol_library, lib_match.group(1))
        except ValueError:
            continue
        x, y, rotation = at
        for local_x, local_y in symbol_pin_local_points(symbol_block):
            absolute_x, absolute_y = absolute_pin_at(
                local_x, local_y, x, y, rotation
            )
            endpoints.add((round(absolute_x, 6), round(absolute_y, 6)))
    return endpoints


def label(name: str, x: float, y: float, rotation: float = 0) -> str:
    return f'''\t(label "{name}"
\t\t(at {fmt(x)} {fmt(y)} {fmt(rotation)})
\t\t(effects (font (size 0.9 0.9)))
\t\t(uuid "{uid()}")
\t)'''


def hidden_label(name: str, x: float, y: float, rotation: float = 0) -> str:
    # These labels bridge legacy pin-to-wire coordinate differences while the
    # visible drawing remains wire-first.  KiCad 9 CLI plotting can include
    # hidden local labels, so use a sub-pixel font as a compatibility fallback.
    return f'''\t(label "{name}"
\t\t(at {fmt(x)} {fmt(y)} {fmt(rotation)})
\t\t(effects (font (size 0.01 0.01)) (hide yes))
\t\t(uuid "{uid()}")
\t)'''


def power_symbol(
    lib_id: str, reference: str, x: float, y: float, hide_value: bool = False
) -> str:
    if lib_id == "power:GND":
        ref_at, value, value_at = "0 -6.35 0", "GND", "0 -3.81 0"
    elif lib_id == "power:+5V":
        ref_at, value, value_at = "0 -3.81 0", "+5V", "0 3.56 0"
    elif lib_id == "power:+3V3":
        ref_at, value, value_at = "0 -3.81 0", "+3V3", "0 3.56 0"
    elif lib_id == "power:PWR_FLAG":
        ref_at, value, value_at = "0 1.91 0", "PWR_FLAG", "0 3.81 0"
    else:
        raise ValueError(lib_id)
    ref_at = absolute_at(ref_at, x, y)
    value_at = absolute_at(value_at, x, y)
    value_effects = "(effects (font (size 1.27 1.27))"
    if hide_value:
        value_effects += " hide"
    value_effects += ")"
    return f'''\t(symbol (lib_id "{lib_id}") (at {fmt(x)} {fmt(y)} 0) (unit 1)
\t\t(exclude_from_sim no) (in_bom no) (on_board yes) (dnp no)
\t\t(uuid "{uid()}")
\t\t(property "Reference" "{reference}" (at {ref_at}) (effects (font (size 1.27 1.27)) hide))
\t\t(property "Value" "{value}" (at {value_at}) {value_effects})
\t\t(property "Footprint" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
\t\t(property "Datasheet" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
\t\t(property "Description" "" (at 0 0 0) (effects (font (size 1.27 1.27)) hide))
\t\t(pin "1" (uuid "{uid()}"))
\t\t(instances (project "pico_lta042b010f_carrier" (path "/10000000-0000-4000-8000-000000000001" (reference "{reference}") (unit 1))))
\t)'''


def parse_pad_blocks(block: str) -> list[tuple[int, int, str]]:
    result: list[tuple[int, int, str]] = []
    for match in re.finditer(r'\(pad\s+"[^"]*"', block):
        start = match.start()
        end = find_balanced(block, start)
        result.append((start, end, block[start:end]))
    return result


def pad_net_map(block: str) -> dict[str, dict[str, str]]:
    mapping: dict[str, dict[str, str]] = {}
    for _, _, pad in parse_pad_blocks(block):
        name_match = re.search(r'\(pad\s+"([^"]*)"', pad)
        net_match = re.search(r'\(net\s+(\d+)\s+"([^"]*)"\)', pad)
        if not name_match or not net_match:
            continue
        pin_match = re.search(r'\(pinfunction\s+"([^"]*)"\)', pad)
        type_match = re.search(r'\(pintype\s+"([^"]*)"\)', pad)
        mapping[name_match.group(1)] = {
            "number": net_match.group(1),
            "name": net_match.group(2),
            "pinfunction": pin_match.group(1) if pin_match else "",
            "pintype": type_match.group(1) if type_match else "passive",
        }
    return mapping


def annotate_footprint(footprint: str, nets: dict[str, dict[str, str]]) -> str:
    pads = parse_pad_blocks(footprint)
    for start, end, pad in reversed(pads):
        match = re.search(r'\(pad\s+"([^"]*)"', pad)
        if not match:
            continue
        info = nets.get(match.group(1))
        if not info or "(net " in pad:
            continue
        layers = re.search(r'\(layers[^\n]*\)', pad)
        if not layers:
            continue
        annotation = f'\n\t\t(net {info["number"]} "{info["name"]}")'
        if info.get("pinfunction"):
            annotation += f' (pinfunction "{info["pinfunction"]}")'
        if info.get("pintype"):
            annotation += f' (pintype "{info["pintype"]}")'
        insert_at = layers.end()
        pad = pad[:insert_at] + annotation + pad[insert_at:]
        footprint = footprint[:start] + pad + footprint[end:]
    return footprint


def make_footprint(
    source: Path,
    library_name: str,
    reference: str,
    value: str,
    position: tuple[float, float],
    nets: dict[str, dict[str, str]],
    rotation: float = 0,
) -> str:
    footprint = source.read_text(encoding="utf-8")
    first_name = re.search(r'^\(footprint "([^"]+)"', footprint, re.MULTILINE)
    if not first_name:
        raise ValueError(f"footprint name missing in {source}")
    footprint = (
        footprint[: first_name.start()]
        + f'(footprint "{library_name}:{first_name.group(1)}"'
        + footprint[first_name.end() :]
    )
    footprint = re.sub(r'\(layer "[^"]+"\)', '(layer "F.Cu")', footprint, count=1)
    layer_end = re.search(r'\(layer "F\.Cu"\)', footprint)
    if not layer_end:
        raise ValueError(f"layer missing in {source}")
    footprint = footprint[:layer_end.end()] + f'\n\t(at {fmt(position[0])} {fmt(position[1])} {fmt(rotation)})' + footprint[layer_end.end():]
    footprint = re.sub(r'(\(property "Reference" )"[^"]+"', rf'\1"{reference}"', footprint, count=1)
    footprint = re.sub(r'(\(property "Value" )"[^"]+"', rf'\1"{value}"', footprint, count=1)
    # Library footprints carry UUIDs that are unique only inside the library
    # file.  Cloning those UUIDs into several board footprints makes KiCad DRC
    # associate a pad or courtyard with the wrong instance.  Derive stable,
    # per-reference UUIDs for every cloned primitive instead.
    def clone_uuid(match: re.Match[str]) -> str:
        cloned = uuid.uuid5(
            uuid.NAMESPACE_URL,
            f"pico-footprint-{reference}-{match.group(1)}",
        )
        return f'(uuid "{cloned}")'

    footprint = re.sub(r'\(uuid "([^"]+)"\)', clone_uuid, footprint)
    # Embedded library keepouts use footprint-local coordinates.  A footprint
    # copied into a board does not apply its placement to those polygon points
    # automatically, so translate the points while preserving the keepout.
    cursor = 0
    while True:
        zone_match = re.search(r'^\t\(zone\b', footprint[cursor:], re.MULTILINE)
        if not zone_match:
            break
        zone_start = cursor + zone_match.start()
        zone_end = find_balanced(footprint, zone_start + 1)
        zone = footprint[zone_start:zone_end]

        # This library polygon describes the Pico RF extension below the
        # module. With the present board orientation it is wholly outside
        # the 69.4 mm board edge. Keeping an off-board polygon in the board
        # file causes false pad/keepout DRC errors at the edge without adding
        # any in-board protection; the actual in-board antenna keepout below
        # is retained.
        zone_name = re.search(r'\(name "([^"]*)"\)', zone)
        if zone_name and zone_name.group(1) == "RF Copper Keep Out":
            # Preserve the RF restriction for tracks/vias/copper pour. The
            # library polygon also overlaps two no-net mechanical pads at the
            # module edge; pads are intentionally allowed there because they
            # are not an antenna copper escape.
            zone = zone.replace("(pads not_allowed)", "(pads allowed)")
        if zone_name and zone_name.group(1) == "RF Copper Keep Out":
            raw_points = [
                (float(x), float(y))
                for x, y in re.findall(r'\(xy\s+([^ )]+)\s+([^ )]+)\)', zone)
            ]
            translated_y = []
            for x, y in raw_points:
                _, ry = ArtworkRouter._rotate(x, y, rotation)
                translated_y.append(position[1] + ry)
            if translated_y and min(translated_y) >= 69.4:
                footprint = footprint[:zone_start] + footprint[zone_end:]
                continue

        def translate_zone_point(match: re.Match[str]) -> str:
            x, y = float(match.group(1)), float(match.group(2))
            rx, ry = ArtworkRouter._rotate(x, y, rotation)
            return f'(xy {fmt(position[0] + rx)} {fmt(position[1] + ry)})'

        zone = re.sub(r'\(xy\s+([^ )]+)\s+([^ )]+)\)', translate_zone_point, zone)
        footprint = footprint[:zone_start] + zone + footprint[zone_end:]
        cursor = zone_start + len(zone)
    return annotate_footprint(footprint, nets)


def artwork_uid(kind: str, index: int) -> str:
    return str(uuid.uuid5(uuid.NAMESPACE_URL, f"pico-artwork-{kind}-{index}"))


def parse_routing_pads(footprints: list[str]) -> list[dict[str, object]]:
    """Extract absolute pad centers and copper geometry from rendered footprints.

    The artwork generator deliberately works from the same rendered footprint
    text that is written into the board.  That keeps routing endpoints tied to
    the actual KiCad library geometry instead of duplicating pad coordinates in
    a second hand-maintained table.
    """
    pads: list[dict[str, object]] = []
    for block in footprints:
        ref_match = re.search(r'\(property "Reference" "([^"]+)"', block)
        at_match = re.search(r'\n\s*\(at ([^ )]+) ([^ )]+)(?: ([^ )]+))?\)', block)
        if not ref_match or not at_match:
            continue
        ref = ref_match.group(1)
        fx, fy, frot = float(at_match.group(1)), float(at_match.group(2)), float(at_match.group(3) or 0)
        for _, _, pad in parse_pad_blocks(block):
            name_match = re.search(r'\(pad "([^"]*)"', pad)
            pad_at = re.search(r'\(at ([^ )]+) ([^ )]+)(?: ([^ )]+))?\)', pad)
            size_match = re.search(r'\(size ([^ )]+) ([^ )]+)\)', pad)
            if not name_match or not pad_at or not size_match:
                continue
            net_match = re.search(r'\(net (\d+) "([^"]*)"\)', pad)
            layers_match = re.search(r'\(layers ([^\)]*)\)', pad, re.S)
            if not layers_match:
                continue
            px, py, prot = float(pad_at.group(1)), float(pad_at.group(2)), float(pad_at.group(3) or 0)
            sx, sy = float(size_match.group(1)), float(size_match.group(2))
            fangle = math.radians(-frot)
            gx = fx + px * math.cos(fangle) - py * math.sin(fangle)
            gy = fy + px * math.sin(fangle) + py * math.cos(fangle)
            layers_text = layers_match.group(1)
            layers = set(re.findall(r'"([^"]+)"', layers_text))
            if "*.Cu" in layers:
                copper_layers = {"F.Cu", "B.Cu"}
            else:
                copper_layers = {layer for layer in ("F.Cu", "B.Cu") if layer in layers}
            pads.append(
                {
                    "ref": ref,
                    "pad": name_match.group(1),
                    "x": gx,
                    "y": gy,
                    "size_x": sx,
                    "size_y": sy,
                    "angle": frot + prot,
                    "layers": copper_layers,
                    "net_num": int(net_match.group(1)) if net_match else 0,
                    "net": net_match.group(2) if net_match else "",
                    "through": "thru_hole" in pad,
                }
            )
    return pads


def parse_footprint_keepouts(footprints: list[str]) -> list[tuple[str, list[tuple[float, float]], tuple[int, ...]]]:
    """Read translated copper keepout polygons embedded in footprints."""
    keepouts: list[tuple[str, list[tuple[float, float]], tuple[int, ...]]] = []
    for block in footprints:
        cursor = 0
        while True:
            zone_match = re.search(r'^\t\(zone\b', block[cursor:], re.MULTILINE)
            if not zone_match:
                break
            zone_start = cursor + zone_match.start()
            zone_end = find_balanced(block, zone_start + 1)
            zone = block[zone_start:zone_end]
            cursor = zone_end
            if "(keepout" not in zone:
                continue
            name_match = re.search(r'\(name "([^"]*)"\)', zone)
            points = [(float(x), float(y)) for x, y in re.findall(r'\(xy\s+([^ )]+)\s+([^ )]+)\)', zone)]
            if len(points) < 3:
                continue
            layers: set[int] = set()
            layers_match = re.search(r'\(layers\s+([^\)]*)\)', zone, re.S)
            if layers_match:
                layer_names = set(re.findall(r'"([^"]+)"', layers_match.group(1)))
                if "F.Cu" in layer_names or "*.Cu" in layer_names:
                    layers.add(0)
                if "B.Cu" in layer_names or "*.Cu" in layer_names:
                    layers.add(1)
            if layers:
                keepouts.append((name_match.group(1) if name_match else "embedded keepout", points, tuple(sorted(layers))))
    return keepouts


class ArtworkRouter:
    """Conservative deterministic two-layer router for this carrier board."""

    GRID = 0.25
    WIDTH = 481
    HEIGHT = 281
    CELLS = WIDTH * HEIGHT
    # Match the board's fabrication constraints: standard through vias are
    # 0.5 mm diameter with a 0.3 mm drill.  They are placed after the FFC
    # escape, never directly in the 0.5 mm contact pitch.
    VIA_DIAMETER = 0.5
    VIA_DRILL = 0.3
    STATIC_MARGIN = 0.5
    # A 0.25 mm routing grid can otherwise let a second route pass through a
    # neighbouring cell corner even though the resulting copper is inside
    # KiCad's 0.20 mm clearance. Keep one extra grid cell as a safety margin.
    # Keep the router's clearance model exact at the connector escape.  The
    # 0.5 mm pitch leaves 0.3 mm edge-to-edge between 0.2 mm traces, already
    # above the board's 0.2 mm clearance rule; an extra grid-cell margin would
    # incorrectly treat adjacent legal escapes as blocked.
    TRACK_SAFETY_MARGIN = 0.0
    TRACK_OBSTACLE_RADIUS = 0.75

    def __init__(
        self,
        pads: list[dict[str, object]],
        net_numbers: dict[str, int],
        keepouts: list[tuple[str, list[tuple[float, float]], tuple[int, ...]]] | None = None,
    ) -> None:
        self.pads = pads
        self.net_numbers = net_numbers
        self.pad_blocked = [bytearray(self.CELLS), bytearray(self.CELLS)]
        self.via_blocked = [bytearray(self.CELLS), bytearray(self.CELLS)]
        self.keepout = [bytearray(self.CELLS), bytearray(self.CELLS)]
        self.track_owner = [array("i", [0]) * self.CELLS, array("i", [0]) * self.CELLS]
        self.own_pad_cells: dict[str, list[set[int]]] = {}
        self.objects: list[str] = []
        self.segment_count = 0
        self.via_count = 0
        for pad in pads:
            self._mark_pad(pad)
        # The Pico 2 W antenna is at the lower end of this board orientation.
        # Keep both copper layers clear in the antenna envelope.
        self.mark_keepout(48.5, 61.5, 63.5, 69.5, (0, 1))
        for _, points, layers in keepouts or []:
            self.mark_polygon_keepout(points, layers)

    def _index(self, ix: int, iy: int) -> int:
        return iy * self.WIDTH + ix

    def _grid(self, value: float) -> int:
        return max(0, min(self.WIDTH - 1, int(round(value / self.GRID))))

    @staticmethod
    def _rotate(x: float, y: float, angle: float) -> tuple[float, float]:
        # KiCad's board coordinates use positive Y down, so a positive
        # footprint rotation is clockwise in the file's Cartesian transform.
        a = math.radians(-angle)
        return x * math.cos(a) - y * math.sin(a), x * math.sin(a) + y * math.cos(a)

    def _inside_pad(self, pad: dict[str, object], x: float, y: float, margin: float) -> bool:
        dx = x - float(pad["x"])
        dy = y - float(pad["y"])
        lx, ly = self._rotate(dx, dy, float(pad["angle"]))
        return (
            abs(lx) <= float(pad["size_x"]) / 2 + margin
            and abs(ly) <= float(pad["size_y"]) / 2 + margin
        )

    def _mark_pad(self, pad: dict[str, object]) -> None:
        sx, sy = float(pad["size_x"]), float(pad["size_y"])
        radius = math.hypot(sx, sy) / 2 + self.STATIC_MARGIN
        x, y = float(pad["x"]), float(pad["y"])
        xmin, xmax = self._grid(x - radius), self._grid(x + radius)
        ymin, ymax = max(0, self._grid(y - radius)), min(self.HEIGHT - 1, self._grid(y + radius))
        net = str(pad["net"])
        if net:
            self.own_pad_cells.setdefault(net, [set(), set()])
        for iy in range(ymin, ymax + 1):
            for ix in range(xmin, xmax + 1):
                gx, gy = ix * self.GRID, iy * self.GRID
                if not self._inside_pad(pad, gx, gy, self.STATIC_MARGIN):
                    continue
                idx = self._index(ix, iy)
                if net:
                    for layer in (0, 1):
                        self.own_pad_cells[net][layer].add(idx)
                layers = pad["layers"]
                if "F.Cu" in layers:
                    self.pad_blocked[0][idx] = 1
                if "B.Cu" in layers:
                    self.pad_blocked[1][idx] = 1
                if bool(pad["through"]):
                    self.via_blocked[0][idx] = 1
                    self.via_blocked[1][idx] = 1
                elif "F.Cu" in layers:
                    self.via_blocked[0][idx] = 1

    def mark_keepout(self, xmin: float, ymin: float, xmax: float, ymax: float, layers: tuple[int, ...]) -> None:
        for iy in range(max(0, self._grid(ymin)), min(self.HEIGHT - 1, self._grid(ymax)) + 1):
            for ix in range(max(0, self._grid(xmin)), min(self.WIDTH - 1, self._grid(xmax)) + 1):
                x, y = ix * self.GRID, iy * self.GRID
                if xmin <= x <= xmax and ymin <= y <= ymax:
                    idx = self._index(ix, iy)
                    for layer in layers:
                        self.keepout[layer][idx] = 1
                        self.via_blocked[layer][idx] = 1

    def mark_polygon_keepout(self, points: list[tuple[float, float]], layers: tuple[int, ...]) -> None:
        xmin = max(0, self._grid(min(x for x, _ in points)))
        xmax = min(self.WIDTH - 1, self._grid(max(x for x, _ in points)))
        ymin = max(0, self._grid(min(y for _, y in points)))
        ymax = min(self.HEIGHT - 1, self._grid(max(y for _, y in points)))

        def inside(x: float, y: float) -> bool:
            result = False
            previous_x, previous_y = points[-1]
            for current_x, current_y in points:
                crosses = (current_y > y) != (previous_y > y)
                if crosses:
                    at_x = (previous_x - current_x) * (y - current_y) / (previous_y - current_y) + current_x
                    if x < at_x:
                        result = not result
                previous_x, previous_y = current_x, current_y
            return result

        for iy in range(ymin, ymax + 1):
            for ix in range(xmin, xmax + 1):
                if not inside(ix * self.GRID, iy * self.GRID):
                    continue
                idx = self._index(ix, iy)
                for layer in layers:
                    self.keepout[layer][idx] = 1
                    self.via_blocked[layer][idx] = 1

    def _owner(self, net: str) -> int:
        return self.net_numbers.get(net, 1000 + sum(ord(ch) for ch in net))

    def _is_blocked(self, layer: int, ix: int, iy: int, net: str) -> bool:
        if ix <= 1 or iy <= 1 or ix >= self.WIDTH - 2 or iy >= self.HEIGHT - 2:
            return True
        idx = self._index(ix, iy)
        if self.keepout[layer][idx]:
            return True
        if self.track_owner[layer][idx] not in (0, self._owner(net)):
            return True
        if self.pad_blocked[layer][idx] and idx not in self.own_pad_cells.get(net, [set(), set()])[layer]:
            return True
        return False

    def _via_allowed(
        self,
        ix: int,
        iy: int,
        net: str,
        via_min_x: float | None,
        via_max_x: float | None,
        via_min_y: float | None,
    ) -> bool:
        x = ix * self.GRID
        y = iy * self.GRID
        if via_min_x is not None and x < via_min_x:
            return False
        if via_max_x is not None and x > via_max_x:
            return False
        if via_min_y is not None and y < via_min_y:
            return False
        idx = self._index(ix, iy)
        owner = self._owner(net)
        return (
            not self.via_blocked[0][idx]
            and not self.via_blocked[1][idx]
            and self.track_owner[0][idx] in (0, owner)
            and self.track_owner[1][idx] in (0, owner)
            and not self.keepout[0][idx]
            and not self.keepout[1][idx]
        )

    def _heuristic(self, ix: int, iy: int, layer: int, gx: int, gy: int) -> float:
        return abs(ix - gx) + abs(iy - gy) + (0.05 if layer else 0.0)

    def _mark_radius(self, x: float, y: float, layer: int, radius: float, owner: int) -> None:
        xmin, xmax = max(0, self._grid(x - radius)), min(self.WIDTH - 1, self._grid(x + radius))
        ymin, ymax = max(0, self._grid(y - radius)), min(self.HEIGHT - 1, self._grid(y + radius))
        for iy in range(ymin, ymax + 1):
            for ix in range(xmin, xmax + 1):
                px, py = ix * self.GRID, iy * self.GRID
                if math.hypot(px - x, py - y) <= radius:
                    idx = self._index(ix, iy)
                    existing = self.track_owner[layer][idx]
                    if existing in (0, owner):
                        self.track_owner[layer][idx] = owner
                    else:
                        # Keep collisions visible to later route searches;
                        # one integer owner must not be overwritten by a
                        # different net merely because the clearance halos
                        # overlap on the coarse routing grid.
                        self.track_owner[layer][idx] = -1

    def _mark_track(
        self,
        x1: float,
        y1: float,
        x2: float,
        y2: float,
        layer: int,
        owner: int,
        radius: float | None = None,
    ) -> None:
        radius = self.TRACK_OBSTACLE_RADIUS if radius is None else radius
        xmin = max(0, self._grid(min(x1, x2) - radius))
        xmax = min(self.WIDTH - 1, self._grid(max(x1, x2) + radius))
        ymin = max(0, self._grid(min(y1, y2) - radius))
        ymax = min(self.HEIGHT - 1, self._grid(max(y1, y2) + radius))
        for iy in range(ymin, ymax + 1):
            for ix in range(xmin, xmax + 1):
                px, py = ix * self.GRID, iy * self.GRID
                if x1 == x2:
                    distance = abs(px - x1) if min(y1, y2) <= py <= max(y1, y2) else math.inf
                elif y1 == y2:
                    distance = abs(py - y1) if min(x1, x2) <= px <= max(x1, x2) else math.inf
                else:
                    vx, vy = x2 - x1, y2 - y1
                    length2 = vx * vx + vy * vy
                    t = max(0.0, min(1.0, ((px - x1) * vx + (py - y1) * vy) / length2))
                    qx, qy = x1 + t * vx, y1 + t * vy
                    distance = math.hypot(px - qx, py - qy)
                if distance <= radius:
                    idx = self._index(ix, iy)
                    existing = self.track_owner[layer][idx]
                    if existing in (0, owner):
                        self.track_owner[layer][idx] = owner
                    else:
                        self.track_owner[layer][idx] = -1

    def add_fixed_segment(self, net: str, start: tuple[float, float], end: tuple[float, float], layer: str = "F.Cu", width: float = 0.3) -> None:
        layer_index = 0 if layer == "F.Cu" else 1
        number = self.net_numbers[net]
        self.objects.append(
            f'\t(segment (start {fmt(start[0])} {fmt(start[1])}) (end {fmt(end[0])} {fmt(end[1])}) '
            f'(width {fmt(width)}) (layer "{layer}") (net {number}) (uuid "{artwork_uid("segment", len(self.objects))}"))'
        )
        self._mark_track(
            start[0], start[1], end[0], end[1], layer_index, self._owner(net),
            width / 2 + 0.2 + self.TRACK_SAFETY_MARGIN,
        )
        self.segment_count += 1

    def add_fixed_via(self, net: str, point: tuple[float, float]) -> None:
        number = self.net_numbers[net]
        self.objects.append(
            f'\t(via (at {fmt(point[0])} {fmt(point[1])}) (size {fmt(self.VIA_DIAMETER)}) '
            f'(drill {fmt(self.VIA_DRILL)}) (layers "F.Cu" "B.Cu") (net {number}) '
            f'(uuid "{artwork_uid("via", len(self.objects))}"))'
        )
        for layer in (0, 1):
            self._mark_radius(point[0], point[1], layer, self.TRACK_OBSTACLE_RADIUS, self._owner(net))
        self.via_count += 1

    def _add_segment(self, net: str, start: tuple[float, float], end: tuple[float, float], layer: int, width: float) -> None:
        if math.hypot(start[0] - end[0], start[1] - end[1]) < 1e-6:
            return
        layer_name = "F.Cu" if layer == 0 else "B.Cu"
        number = self.net_numbers[net]
        self.objects.append(
            f'\t(segment (start {fmt(start[0])} {fmt(start[1])}) (end {fmt(end[0])} {fmt(end[1])}) '
            f'(width {fmt(width)}) (layer "{layer_name}") (net {number}) (uuid "{artwork_uid("segment", len(self.objects))}"))'
        )
        self._mark_track(
            start[0], start[1], end[0], end[1], layer, self._owner(net),
            width / 2 + 0.2 + self.TRACK_SAFETY_MARGIN,
        )
        self.segment_count += 1

    def _add_via(self, net: str, point: tuple[float, float]) -> None:
        number = self.net_numbers[net]
        self.objects.append(
            f'\t(via (at {fmt(point[0])} {fmt(point[1])}) (size {fmt(self.VIA_DIAMETER)}) '
            f'(drill {fmt(self.VIA_DRILL)}) (layers "F.Cu" "B.Cu") (net {number}) '
            f'(uuid "{artwork_uid("via", len(self.objects))}"))'
        )
        for layer in (0, 1):
            self._mark_radius(point[0], point[1], layer, self.TRACK_OBSTACLE_RADIUS, self._owner(net))
        self.via_count += 1

    def route_pair(
        self,
        net: str,
        start: tuple[float, float],
        goal: tuple[float, float],
        width: float,
        preferred_layer: int,
        via_cost: float,
        via_min_x: float | None = None,
        via_max_x: float | None = None,
        via_min_y: float | None = None,
        start_layer: int = 0,
        goal_layer: int = 0,
    ) -> None:
        sx, sy = self._grid(start[0]), self._grid(start[1])
        gx, gy = self._grid(goal[0]), self._grid(goal[1])
        start_state = (start_layer, sx, sy)
        queue: list[tuple[float, float, int, int, int]] = []
        heapq.heappush(queue, (self._heuristic(sx, sy, start_layer, gx, gy), 0.0, start_layer, sx, sy))
        costs: dict[tuple[int, int, int], float] = {start_state: 0.0}
        parents: dict[tuple[int, int, int], tuple[int, int, int] | None] = {start_state: None}
        visited = 0
        final: tuple[int, int, int] | None = None
        while queue and visited < 500000:
            _, current_cost, layer, ix, iy = heapq.heappop(queue)
            state = (layer, ix, iy)
            if current_cost != costs.get(state):
                continue
            visited += 1
            if ix == gx and iy == gy and layer == goal_layer:
                final = state
                break
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = ix + dx, iy + dy
                if not (0 <= nx < self.WIDTH and 0 <= ny < self.HEIGHT):
                    continue
                if self._is_blocked(layer, nx, ny, net) and not (nx == gx and ny == gy and layer == goal_layer):
                    continue
                step = 1.0 + (0.04 if layer != preferred_layer else 0.0)
                next_state = (layer, nx, ny)
                new_cost = current_cost + step
                if new_cost < costs.get(next_state, math.inf):
                    costs[next_state] = new_cost
                    parents[next_state] = state
                    heapq.heappush(queue, (new_cost + self._heuristic(nx, ny, layer, gx, gy), new_cost, layer, nx, ny))
            if self._via_allowed(ix, iy, net, via_min_x, via_max_x, via_min_y):
                other = 1 - layer
                next_state = (other, ix, iy)
                new_cost = current_cost + via_cost
                if new_cost < costs.get(next_state, math.inf):
                    costs[next_state] = new_cost
                    parents[next_state] = state
                    heapq.heappush(queue, (new_cost + self._heuristic(ix, iy, other, gx, gy), new_cost, other, ix, iy))
        if final is None:
            raise RuntimeError(f"route failed for {net}: {start} -> {goal} after {visited} nodes")
        states: list[tuple[int, int, int]] = []
        cursor: tuple[int, int, int] | None = final
        while cursor is not None:
            states.append(cursor)
            cursor = parents[cursor]
        states.reverse()

        # Collapse the 0.25 mm A* walk into orthogonal runs. The geometry is
        # unchanged, but the generated PCB remains readable and hand-editable
        # instead of containing thousands of one-cell segments.
        current_layer = start_layer
        run_start = start
        run_end = start
        previous_direction: tuple[int, int] | None = None

        def flush_run() -> None:
            nonlocal run_start, run_end, previous_direction
            self._add_segment(net, run_start, run_end, current_layer, width)
            run_start = run_end
            previous_direction = None

        for layer, ix, iy in states[1:]:
            point = (ix * self.GRID, iy * self.GRID)
            if layer != current_layer:
                flush_run()
                self._add_via(net, point)
                current_layer = layer
                run_start = point
                run_end = point
                previous_direction = None
                continue
            direction = (
                0 if abs(point[0] - run_end[0]) < 1e-9 else (1 if point[0] > run_end[0] else -1),
                0 if abs(point[1] - run_end[1]) < 1e-9 else (1 if point[1] > run_end[1] else -1),
            )
            if previous_direction is not None and direction != previous_direction:
                flush_run()
            run_end = point
            previous_direction = direction
        flush_run()
        self._add_segment(net, run_end, goal, current_layer, width)

    def route_net(
        self,
        net: str,
        endpoints: list[tuple[str, str, float, float]],
        root: tuple[str, str] | None,
        width: float,
        preferred_layer: int,
        via_cost: float,
        via_min_x: float | None = None,
        via_max_x: float | None = None,
        via_min_y: float | None = None,
    ) -> None:
        if len(endpoints) < 2:
            return
        if root:
            root_item = next((item for item in endpoints if item[:2] == root), endpoints[0])
        else:
            root_item = endpoints[0]
        remaining = [item for item in endpoints if item != root_item]
        remaining.sort(key=lambda item: math.hypot(item[2] - root_item[2], item[3] - root_item[3]))
        for _, _, x, y in remaining:
            self.route_pair(
                net,
                (root_item[2], root_item[3]),
                (x, y),
                width,
                preferred_layer,
                via_cost,
                via_min_x,
                via_max_x,
                via_min_y,
            )

    def add_gnd_stubs(self, endpoints: list[tuple[str, str, float, float]]) -> None:
        """Bring SMD GND pads to the B.Cu plane with short top-side stubs."""
        for ref, _, x, y in endpoints:
            if ref == "U1" and x < 50:
                via = (44.8, y)
            elif ref == "U1" and x > 60:
                via = (67.2, y)
            elif ref == "U3":
                via = (80.8, y)
            elif ref == "D5":
                via = (110.2, y)
            elif ref == "R10":
                via = (102.2, y)
            elif ref == "R16":
                via = (108.2, y)
            else:
                continue
            self.add_fixed_segment("GND", (x, y), via, "F.Cu", 0.3)
            self.add_fixed_via("GND", via)

    def output(self) -> str:
        return "\n".join(self.objects)


def make_copper_zone(net_number: int, net_name: str, layer: str, points: list[tuple[float, float]], clearance: float, name: str) -> str:
    point_text = " ".join(f"(xy {fmt(x)} {fmt(y)})" for x, y in points)
    return (
        f'\t(zone\n'
        f'\t\t(net {net_number})\n'
        f'\t\t(net_name "{net_name}")\n'
        f'\t\t(layer "{layer}")\n'
        f'\t\t(uuid "{artwork_uid("zone", name)}")\n'
        f'\t\t(name "{name}")\n'
        f'\t\t(hatch edge 0.5)\n'
        f'\t\t(connect_pads (clearance {fmt(clearance)}))\n'
        f'\t\t(min_thickness 0.25)\n'
        f'\t\t(filled_areas_thickness no)\n'
        f'\t\t(fill yes (thermal_gap {fmt(clearance)}) (thermal_bridge_width {fmt(clearance)}))\n'
        f'\t\t(polygon (pts {point_text}))\n'
        f'\t)\n'
    )


def make_copper_keepout(name: str, points: list[tuple[float, float]]) -> str:
    point_text = " ".join(f"(xy {fmt(x)} {fmt(y)})" for x, y in points)
    return (
        f'\t(zone\n'
        f'\t\t(net 0)\n'
        f'\t\t(net_name "")\n'
        f'\t\t(layers "F.Cu" "B.Cu")\n'
        f'\t\t(uuid "{artwork_uid("keepout", name)}")\n'
        f'\t\t(name "{name}")\n'
        f'\t\t(hatch full 0.5)\n'
        f'\t\t(connect_pads (clearance 0))\n'
        f'\t\t(min_thickness 0.25)\n'
        f'\t\t(filled_areas_thickness no)\n'
        f'\t\t(keepout (tracks not_allowed) (vias not_allowed) (pads not_allowed) (copperpour not_allowed) (footprints allowed))\n'
        f'\t\t(placement (enabled no) (sheetname ""))\n'
        f'\t\t(fill (thermal_gap 0.5) (thermal_bridge_width 0.5))\n'
        f'\t\t(polygon (pts {point_text}))\n'
        f'\t)\n'
    )


def build_schematic() -> None:
    source = SRC_SCH.read_text(encoding="utf-8")
    source_is_reviewed = '(paper "A4")' in source
    lib_start = source.index('(lib_symbols')
    lib_end = find_balanced(source, lib_start)
    lib_block = source[lib_start:lib_end]
    tail = round_tail_coordinates(source[lib_end:])

    standard_sources = {
        ("Device", "R"): (KI_SYMBOLS / "Device.kicad_sym", "R"),
        ("Device", "L"): (KI_SYMBOLS / "Device.kicad_sym", "L"),
        ("Device", "D_Schottky"): (KI_SYMBOLS / "Device.kicad_sym", "D_Schottky"),
        ("Device", "C"): (KI_SYMBOLS / "Device.kicad_sym", "C"),
        ("Device", "C_Polarized"): (KI_SYMBOLS / "Device.kicad_sym", "C_Polarized"),
        ("Device", "LED"): (KI_SYMBOLS / "Device.kicad_sym", "LED"),
        ("Device", "R_Potentiometer"): (KI_SYMBOLS / "Device.kicad_sym", "R_Potentiometer"),
        ("Connector_Generic", "Conn_01x02"): (KI_SYMBOLS / "Connector_Generic.kicad_sym", "Conn_01x02"),
        ("Connector_Generic", "Conn_01x07"): (KI_SYMBOLS / "Connector_Generic.kicad_sym", "Conn_01x07"),
        ("Regulator_Switching", "MC33063AD"): (KI_SYMBOLS / "Regulator_Switching.kicad_sym", "MC33063AD"),
        ("power", "+5V"): (KI_SYMBOLS / "power.kicad_sym", "+5V"),
        ("power", "+3V3"): (KI_SYMBOLS / "power.kicad_sym", "+3V3"),
        ("power", "GND"): (KI_SYMBOLS / "power.kicad_sym", "GND"),
        ("power", "PWR_FLAG"): (KI_SYMBOLS / "power.kicad_sym", "PWR_FLAG"),
    }
    standard_blocks: list[str] = []
    cached_files: dict[Path, str] = {}
    for (library, name), (path, source_name) in standard_sources.items():
        text = cached_files.setdefault(path, path.read_text(encoding="utf-8"))
        qualified = qualify_symbol(extract_symbol(text, source_name), library, source_name)
        if library == "Regulator_Switching" and source_name == "MC33063AD":
            qualified = qualified.replace('(pin open_emitter line', '(pin passive line')
        standard_blocks.append(qualified)
    custom_pico_sch = tune_pico_symbol_visuals(extract_symbol(lib_block, "Custom:Pico_2W_40P"))
    custom_lcd_sch = extract_symbol(lib_block, "Custom:LTA042B010F_FFC36")
    custom_lcd_sch = custom_lcd_sch.replace('(pin power_in line', '(pin passive line')
    # Keep only the two custom symbols still used by the reviewed design.
    # The old provisional POWER_* definitions were unused and caused KiCad's
    # native exporter to plot their default fields at the sheet origin.
    new_lib = "(lib_symbols\n" + "\n\n".join([custom_pico_sch, custom_lcd_sch, *standard_blocks]) + "\n)"
    # The original hand-built schematic was also plotting the default
    # Reference/Value text from embedded library definitions at the sheet
    # origin.  Instance properties below carry the real displayed fields;
    # hide only the library defaults so the native KiCad export stays clean.
    for property_name in ("Reference", "Value"):
        search_from = 0
        marker = f'(property "{property_name}"'
        while True:
            property_start = new_lib.find(marker, search_from)
            if property_start < 0:
                break
            property_end = find_balanced(new_lib, property_start)
            property_block = new_lib[property_start:property_end]
            if "(hide yes)" not in property_block:
                effects_start = property_block.find("(effects")
                if effects_start >= 0:
                    effects_end = find_balanced(property_block, effects_start)
                    property_block = property_block[:effects_end - 1] + "\n\t\t\t\t\t(hide yes)\n\t\t\t\t" + property_block[effects_end - 1:]
                    new_lib = new_lib[:property_start] + property_block + new_lib[property_end:]
                    property_end = property_start + len(property_block)
            search_from = property_end

    objects = top_level_blocks(tail)
    old: dict[str, str] = {}
    sheet_instance = None
    for block in objects:
        ref = get_reference(block)
        if ref:
            old[ref] = block
        if block.startswith("(sheet_instances"):
            sheet_instance = block
    required = ["U1", "J1", "J2", "J3", "U3", "R7", "L1", "D4", "RV1"]
    if not all(ref in old for ref in required):
        raise ValueError("required schematic instances not found")

    root: list[str] = []
    root.append(instance_from_old(old, "U1", "Custom:Pico_2W_40P", "Module:RaspberryPi_Pico_W_SMD_HandSolder", (grid(179.38), grid(168.42)), 0, "RaspberryPi_Pico_2W"))
    j1_position = (grid(317.50), grid(168.42))
    old_j1_position = (grid(239.38), grid(168.42))
    j1_delta = (j1_position[0] - old_j1_position[0], j1_position[1] - old_j1_position[1])
    old_j1_pin_x = old_j1_position[0] - 7.62
    new_j1_pin_x = j1_position[0] - 7.62

    def j1_label_state(logical_x: float, logical_y: float) -> str | None:
        if logical_y < 120:
            return None
        if abs(logical_x - old_j1_pin_x) <= 0.65:
            return "old"
        if abs(logical_x - new_j1_pin_x) <= 0.65:
            return "new"
        return None

    root.append(instance_from_old(old, "J1", "Custom:LTA042B010F_FFC36", "Connector_FFC-FPC:TE_3-1734839-6_1x36-1MP_P0.5mm_Horizontal", j1_position, 0, "LTA042B010F_FFC36"))

    lcd_signal_names = {
        *(f"LCD_R{index}" for index in range(6)),
        *(f"LCD_G{index}" for index in range(6)),
        *(f"LCD_B{index}" for index in range(6)),
        "LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC",
    }

    j2 = instance_from_old(
        old,
        "J2",
        "Connector_Generic:Conn_01x02",
        "Custom:A295_CTRPB_1",
        (139.70, 100.33),
        0,
        "USB-C 5V INPUT",
    )
    root.append(resize_connector_pins(j2, 2))

    instances = [
        ("J3", "Connector_Generic:Conn_01x02", "Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical", (279.40, 153.67), 0, "13V8 OUT"),
        ("U3", "Regulator_Switching:MC33063AD", "Package_SO:SOIC-8_3.9x4.9mm_P1.27mm", (168.91, 101.60), 0, "MC34063AD"),
        ("R7", "Device:R", "Resistor_THT:R_Axial_DIN0411_L9.9mm_D3.6mm_P5.08mm_Vertical", (149.86, 82.55), 270, "0.47R 1%"),
        ("L1", "Device:L", "Inductor_SMD:L_Taiyo-Yuden_NR-10050_9.8x10.0mm_HandSoldering", (163.83, 82.55), 270, "100uH"),
        ("D4", "Device:D_Schottky", "Diode_THT:D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp", (180.34, 82.55), 180, "1N5819"),
        ("R8", "Device:R", "Resistor_THT:R_Axial_DIN0204_L3.6mm_D1.6mm_P2.54mm_Vertical", (149.86, 99.06), 90, "200 1%"),
        ("C6", "Device:C_Polarized", "Capacitor_THT:CP_Radial_D8.0mm_P3.50mm", (130.81, 105.41), 0, "220uF 35V"),
        ("C7", "Device:C", "Capacitor_THT:C_Disc_D5.0mm_W2.5mm_P2.50mm", (149.86, 115.57), 0, "470pF 50V C0G"),
        ("R9", "Device:R", "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical", (208.28, 93.98), 0, "10k 0.1%"),
        ("R10", "Device:R", "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical", (208.28, 115.57), 0, "1k 0.1%"),
        ("C8", "Device:C_Polarized", "Capacitor_THT:CP_Radial_D5.0mm_P2.00mm", (190.50, 93.98), 0, "47uF 35V"),
        ("R11", "Device:R", "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical", (224.79, 93.98), 0, "10k 0.1%"),
        ("D5", "Device:LED", "LED_THT:LED_D3.0mm", (224.79, 106.68), 90, "GREEN LED 3mm"),
        ("C9", "Device:C_Polarized", "Capacitor_THT:CP_Radial_D5.0mm_P2.00mm", (242.57, 96.52), 0, "10uF 50V"),
        ("D7", "Device:D_Schottky", "Diode_THT:D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp", (260.35, 104.14), 180, "1N5819"),
        ("D6", "Device:D_Schottky", "Diode_THT:D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp", (247.65, 114.30), 90, "1N5819"),
        ("C11", "Device:C_Polarized", "Capacitor_THT:CP_Radial_D5.0mm_P2.00mm", (278.13, 114.30), 180, "10uF 50V"),
        ("D8", "Device:LED", "LED_THT:LED_D3.0mm", (299.72, 114.30), 270, "RED LED 3mm"),
        ("R16", "Device:R", "Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical", (299.72, 127.00), 0, "10k 0.1%"),
        ("RV1", "Device:R_Potentiometer", "Potentiometer_THT:Potentiometer_Runtron_RM-065_Vertical", (279.40, 180.34), 0, "10k TRIM"),
    ]
    for ref, lib_id, footprint, position, rotation, value in instances:
        root.append(instance_from_old(old, ref, lib_id, footprint, position, rotation, value))
    pin_endpoints = collect_pin_endpoints(new_lib, root)

    for block in objects:
        if block.startswith("(no_connect"):
            at = get_first_at(block)
            if not at:
                continue
            logical_x = at[0] - SHEET_OFFSET_X if source_is_reviewed else at[0]
            logical_y = at[1] - SHEET_OFFSET_Y if source_is_reviewed else at[1]
            j1_state = j1_label_state(logical_x, logical_y)
            if (160 <= logical_x <= 190 and logical_y >= 140) or j1_state:
                translated = translate_sheet_block(block, -SHEET_OFFSET_X, -SHEET_OFFSET_Y) if source_is_reviewed else block
                if j1_state == "old":
                    translated = translate_sheet_block(translated, j1_delta[0], j1_delta[1])
                root.append(translated)
        elif block.startswith("(label"):
            at = get_first_at(block)
            if not at:
                continue
            x, y, _ = at
            logical_x = x - SHEET_OFFSET_X if source_is_reviewed else x
            logical_y = y - SHEET_OFFSET_Y if source_is_reviewed else y
            j1_state = j1_label_state(logical_x, logical_y)
            if (160 <= logical_x <= 190 and logical_y >= 140) or j1_state:
                signal_match = re.match(r'\(label "([^"]+)"', block)
                if signal_match and (
                    signal_match.group(1) in lcd_signal_names
                    or signal_match.group(1).startswith("LCD_")
                ):
                    # The legacy drawing put every LCD signal label directly
                    # on a symbol pin.  Those labels are rebuilt below as
                    # short wires into native bus entries.
                    continue
                translated = translate_sheet_block(block, -SHEET_OFFSET_X, -SHEET_OFFSET_Y) if source_is_reviewed else block
                if j1_state == "old":
                    translated = translate_sheet_block(translated, j1_delta[0], j1_delta[1])
                root.append(translated)

    def logical_point(x: float, y: float) -> tuple[float, float]:
        return grid(x - SHEET_OFFSET_X), grid(y - SHEET_OFFSET_Y)

    def add_bus_bundle(
        blocks: list[str],
        name: str,
        pin_x: float,
        signals: list[tuple[str, float]],
        bus_x: float,
        side: str,
    ) -> None:
        """Add a local vertical RGB bus fan-out without changing net names.

        KiCad buses are visual grouping objects. Each member still needs a
        real wire and a member label, so hidden member labels are placed at
        the wire ends while the visible bus label communicates the group.
        The three RGB buses are kept as separate local corridors next to the
        Pico/LCD symbols; this avoids crossing the power block or title block
        on the single A4 sheet.
        """
        if not signals:
            return
        logical_pin_x, _ = logical_point(pin_x, signals[0][1])
        signal_points = [
            (signal, logical_point(pin_x, y)[1]) for signal, y in signals
        ]
        logical_bus_x, _ = logical_point(bus_x, signals[0][1])
        if side == "left":
            entry_x = bus_x + 2.54
            label_rotation = 180
            label_end_x = bus_x - 7.62
            entry_size = (-2.54, -2.54)
        elif side == "right":
            entry_x = bus_x - 2.54
            label_rotation = 0
            label_end_x = bus_x + 7.62
            entry_size = (2.54, -2.54)
        else:
            raise ValueError(f"unsupported bus side: {side}")
        logical_entry_x, _ = logical_point(entry_x, signals[0][1])
        logical_label_end_x, _ = logical_point(label_end_x, signals[0][1])
        ys = [y for _, y in signal_points]
        bus_y_start = min(ys) - 2.54
        bus_y_end = max(ys) - 2.54
        blocks.append(bus(logical_bus_x, bus_y_start, logical_bus_x, bus_y_end))
        blocks.append(
            bus(logical_bus_x, bus_y_end, logical_label_end_x, bus_y_end)
        )
        blocks.append(bus_label(name, logical_bus_x, bus_y_end, label_rotation))
        for signal, y in signal_points:
            blocks.append(wire(logical_pin_x, y, logical_entry_x, y))
            blocks.append(
                bus_entry(logical_entry_x, y, entry_size[0], entry_size[1])
            )
            blocks.append(hidden_label(signal, logical_entry_x, y, label_rotation))

    # Coordinates below are the visible coordinates from the reviewed A4
    # schematic. They are converted back to the generator's pre-page-offset
    # coordinate system by logical_point().
    lcd_signal_blocks: list[str] = []
    u1_left_bundles = [
        (
            "LCD_R[0..5]",
            110.49,
            [("LCD_R0", 101.60), ("LCD_R3", 106.68), ("LCD_R5", 109.22)],
            99.06,
        ),
        (
            "LCD_G[0..5]",
            110.49,
            [("LCD_G0", 111.76), ("LCD_G2", 114.30), ("LCD_G5", 119.38)],
            96.52,
        ),
        (
            "LCD_B[0..5]",
            110.49,
            [("LCD_B1", 121.92), ("LCD_B2", 124.46), ("LCD_B4", 127.00)],
            93.98,
        ),
    ]
    u1_right_bundles = [
        (
            "LCD_R[0..5]",
            125.73,
            [("LCD_R1", 104.14), ("LCD_R2", 106.68), ("LCD_R4", 109.22)],
            135.89,
        ),
        (
            "LCD_G[0..5]",
            125.73,
            [("LCD_G1", 114.30), ("LCD_G3", 116.84), ("LCD_G4", 119.38)],
            140.97,
        ),
        (
            "LCD_B[0..5]",
            125.73,
            [("LCD_B0", 121.92), ("LCD_B3", 127.00), ("LCD_B5", 129.54)],
            146.05,
        ),
    ]
    j1_bundles = [
        (
            "LCD_R[0..5]",
            248.92,
            [(f"LCD_R{index}", 86.36 + index * 2.54) for index in range(6)],
            241.30,
        ),
        (
            "LCD_G[0..5]",
            248.92,
            [(f"LCD_G{index}", 104.14 + index * 2.54) for index in range(6)],
            238.76,
        ),
        (
            "LCD_B[0..5]",
            248.92,
            [(f"LCD_B{index}", 121.92 + index * 2.54) for index in range(6)],
            236.22,
        ),
    ]
    for name, pin_x, signals, bus_x in u1_left_bundles:
        add_bus_bundle(lcd_signal_blocks, name, pin_x, signals, bus_x, "left")
    for name, pin_x, signals, bus_x in u1_right_bundles:
        add_bus_bundle(lcd_signal_blocks, name, pin_x, signals, bus_x, "right")
    for name, pin_x, signals, bus_x in j1_bundles:
        add_bus_bundle(lcd_signal_blocks, name, pin_x, signals, bus_x, "left")

    def add_control_stub(
        pin_x: float,
        pin_y: float,
        wire_x: float,
        name: str,
        side: str,
    ) -> None:
        logical_pin_x, logical_y = logical_point(pin_x, pin_y)
        logical_wire_x, _ = logical_point(wire_x, pin_y)
        rotation = 180 if side == "left" else 0
        lcd_signal_blocks.extend(
            [
                wire(logical_pin_x, logical_y, logical_wire_x, logical_y),
                hidden_label(name, logical_wire_x, logical_y, rotation),
                visible_label(name, logical_wire_x, logical_y, rotation, size=0.8),
            ]
        )

    for pin_y, name in [(96.52, "LCD_VSYNC"), (99.06, "LCD_HSYNC")]:
        add_control_stub(110.49, pin_y, 102.87, name, "left")
    add_control_stub(125.73, 101.60, 133.35, "LCD_NCLK", "right")
    for pin_y, name in [(142.24, "LCD_VSYNC"), (144.78, "LCD_HSYNC"), (149.86, "LCD_NCLK")]:
        add_control_stub(248.92, pin_y, 238.76, name, "left")
    root.extend(block for block in lcd_signal_blocks if block)

    gnd_y, plus_y = 139.70, 82.55
    wires = [
        # J2 is the Akizuki 116895 power-only receptacle. Pin 1 is VBUS and
        # pin 2 is GND. The part has no CC contacts, so it is for a USB-A
        # source with an A-to-C cable, not a C-to-C source.
        wire(134.62, 100.33, 127.00, 100.33),
        wire(127.00, 100.33, 127.00, 96.52),
        wire(127.00, 96.52, 127.00, 82.55),
        wire(127.00, 82.55, 146.05, 82.55),

        # +5 bus to U3 Vin; R8 driver resistor; C6
        wire(127.00, 96.52, 154.94, 96.52), wire(158.75, 96.52, 154.94, 96.52),
        # R8 pin 1 joins the +5 bus with one short branch.  Its pin 2 path
        # leaves U3 to the left and loops above the symbol, never through it.
        wire(146.05, 99.06, 146.05, 96.52),
        wire(153.67, 99.06, 153.67, 91.44),
        wire(153.67, 91.44, 187.96, 91.44),
        wire(187.96, 91.44, 187.96, 99.06),
        wire(187.96, 99.06, 179.07, 99.06),
        # C6 is placed below/left of J2 so its +5 pin is reached from the
        # input rail without crossing the connector body.
        wire(130.81, 109.22, 130.81, gnd_y),

        # R7/L1 current path, IPK sense.  Route the sense line above and to
        # the right of U3; do not share the +5V Vin corridor or cross C8.
        wire(153.67, 82.55, 160.02, 82.55), wire(167.64, 82.55, 176.53, 82.55),
        wire(156.21, 82.55, 156.21, 88.90), wire(156.21, 88.90, 194.31, 88.90),
        wire(194.31, 88.90, 194.31, 100.33), wire(194.31, 100.33, 181.61, 100.33),
        wire(181.61, 100.33, 181.61, 96.52), wire(181.61, 96.52, 179.07, 96.52),

        # U3 TC, GND, switch emitter
        wire(149.86, 111.76, 149.86, 106.68), wire(154.94, 106.68, 149.86, 106.68),
        wire(158.75, 106.68, 154.94, 106.68), wire(149.86, 119.38, 149.86, gnd_y),
        wire(168.91, 114.30, 168.91, 118.11), wire(168.91, 118.11, 168.91, gnd_y),
        wire(179.07, 106.68, 182.88, 106.68), wire(182.88, 106.68, 187.96, 106.68),
        wire(187.96, 106.68, 187.96, gnd_y),

        # U3 VFB and feedback divider
        wire(179.07, 109.22, 182.88, 109.22), wire(182.88, 109.22, 195.58, 109.22),
        wire(195.58, 109.22, 195.58, 116.84), wire(195.58, 116.84, 208.28, 116.84),
        wire(208.28, 97.79, 208.28, 111.76), wire(208.28, 111.76, 208.28, 116.84),
        wire(208.28, 119.38, 208.28, gnd_y),

        # +13 rail and positive LED. Stop the rail at the last actual branch;
        # the previous extension to x=299.72 was a visually dangling wire.
        wire(184.15, 82.55, 224.79, 82.55), wire(190.50, 90.17, 190.50, 82.55),
        wire(190.50, 97.79, 190.50, gnd_y), wire(208.28, 90.17, 208.28, 82.55),
        wire(224.79, 90.17, 224.79, 82.55), wire(224.79, 97.79, 224.79, 102.87),
        wire(224.79, 110.49, 224.79, gnd_y),

        # C9 is AC-coupled from SW_NODE.  Route it above the +13V8 rail so
        # the two nets never cross, then keep the charge-pump midpoint local.
        wire(172.72, 82.55, 172.72, 76.20),
        wire(172.72, 76.20, 242.57, 76.20),
        wire(242.57, 76.20, 242.57, 92.71),
        wire(242.57, 100.33, 242.57, 104.14),
        wire(242.57, 104.14, 256.54, 104.14), wire(247.65, 104.14, 247.65, 110.49),
        wire(247.65, 118.11, 247.65, gnd_y), wire(264.16, 104.14, 299.72, 104.14),
        wire(278.13, 104.14, 278.13, 110.49), wire(278.13, 118.11, 278.13, gnd_y),
        wire(299.72, 104.14, 299.72, 110.49), wire(299.72, 118.11, 299.72, 123.19),
        wire(299.72, 130.81, 299.72, gnd_y),

        # External connector and trim
        wire(274.32, 153.67, 270.51, 153.67),
        wire(274.32, 156.21, 266.70, 156.21), wire(266.70, 156.21, 266.70, gnd_y),
        wire(279.40, 168.91, 279.40, 176.53),
        # RV1 pin 3 is below the +3V3 branch.  Terminate it at a local GND
        # symbol below the pot instead of routing upward through +3V3.
        wire(279.40, 184.15, 279.40, 190.50), wire(283.21, 180.34, 287.02, 180.34),
        wire(125.73, gnd_y, 299.72, gnd_y),
    ]
    root.extend(wire_block for wire_block in wires if wire_block)
    wire_segments = parse_wire_segments(wires)
    seen_junctions: set[tuple[float, float]] = set()
    kept_junctions: list[tuple[float, float]] = []
    removed_junctions: list[tuple[float, float]] = []
    four_way_junctions: list[tuple[float, float]] = []
    candidate_count = 0

    def add_junction_candidate(x: float, y: float) -> None:
        nonlocal candidate_count
        candidate_count += 1
        coordinate = (round(x, 6), round(y, 6))
        if coordinate in seen_junctions:
            removed_junctions.append(coordinate)
            return
        seen_junctions.add(coordinate)
        endpoints, interiors = junction_wire_profile(x, y, wire_segments)
        if 2 * interiors + endpoints >= 4:
            four_way_junctions.append(coordinate)
        if junction_is_structurally_needed(
            endpoints, interiors, coordinate in pin_endpoints
        ):
            root.append(junction(x, y))
            kept_junctions.append(coordinate)
        else:
            removed_junctions.append(coordinate)

    for x, y in [
        (127.00, 82.55), (130.81, 82.55), (127.00, 96.52), (127.00, 99.06),
        (127.00, 100.33), (130.81, 100.33), (125.73, gnd_y),
        (130.81, gnd_y), (134.62, gnd_y),
        (134.62, 100.33), (134.62, 102.87), (130.81, 101.60),
        (146.05, 96.52), (146.05, 99.06), (156.21, 82.55), (156.21, 88.90),
        (181.61, 96.52), (181.61, 100.33), (194.31, 100.33),
        (154.94, 106.68), (149.86, 106.68), (149.86, gnd_y),
        (168.91, 118.11), (168.91, gnd_y), (182.88, 106.68), (187.96, 106.68),
        (187.96, gnd_y), (182.88, 109.22), (195.58, 109.22), (195.58, 116.84),
        (208.28, 116.84), (208.28, gnd_y), (184.15, 82.55), (190.50, 82.55),
        (190.50, gnd_y), (208.28, 82.55), (224.79, 82.55), (224.79, 102.87),
        (224.79, gnd_y), (172.72, 82.55), (242.57, 104.14), (247.65, 104.14),
        (247.65, gnd_y), (256.54, 104.14), (264.16, 104.14), (278.13, 104.14),
        (278.13, gnd_y), (299.72, 104.14), (299.72, 110.49), (299.72, 118.11),
        (299.72, 123.19), (299.72, gnd_y), (270.51, 153.67), (270.51, 151.13),
        (279.40, 176.53), (279.40, 184.15), (279.40, 190.50), (283.21, 180.34),
        (266.70, 156.21), (266.70, gnd_y),
        (130.81, 82.55), (125.73, gnd_y), (196.85, 82.55), (285.75, 104.14),
    ]:
        add_junction_candidate(x, y)
    for x, y in [
        (146.05, 82.55), (153.67, 82.55), (160.02, 82.55), (167.64, 82.55),
        (176.53, 82.55), (190.50, 90.17), (208.28, 90.17),
        (224.79, 90.17), (242.57, 92.71), (154.94, 96.52), (158.75, 96.52),
        (179.07, 96.52), (130.81, 97.79), (190.50, 97.79), (208.28, 97.79),
        (224.79, 97.79), (134.62, 99.06), (146.05, 99.06), (175.26, 99.06),
        (179.07, 99.06), (242.57, 100.33), (144.78, 101.60), (158.75, 106.68),
        (179.07, 106.68), (144.78, 109.22), (179.07, 109.22), (224.79, 110.49),
        (247.65, 110.49), (278.13, 110.49), (149.86, 111.76), (208.28, 111.76),
        (168.91, 114.30), (247.65, 118.11), (278.13, 118.11), (149.86, 119.38),
        (208.28, 119.38), (299.72, 130.81), (144.78, 139.70), (274.32, 151.13),
        (274.32, 153.67), (287.02, 180.34), (134.62, 102.87), (130.81, 102.87),
    ]:
        add_junction_candidate(x, y)

    OUT.mkdir(parents=True, exist_ok=True)
    report = [
        "KiCad schematic junction cleanup report",
        f"candidate entries: {candidate_count}",
        f"unique coordinates: {len(seen_junctions)}",
        f"kept structural/terminal junctions: {len(kept_junctions)}",
        f"removed duplicate/endpoint/collinear junctions: {len(removed_junctions)}",
        f"four-way junctions retained for review: {len(four_way_junctions)}",
        f"component-pin endpoint junctions kept: {sum(coordinate in pin_endpoints for coordinate in kept_junctions)}",
        "",
        "Rule: retain wire-wire T/cross branches; do not place dots on component pin endpoints.",
        "Signal wiring is preserved while redundant J2/R8/R9 routing is removed and U3/C8 are not crossed.",
        "J2 pin 1 is +5V VBUS and pin 2 is GND; Akizuki 116895 has no CC contacts and therefore requires a USB-A source with an A-to-C cable.",
        "IPK sense is routed through a separate upper/right corridor instead of sharing the +5V Vin line.",
        "Internal helper labels remain electrically present at sub-pixel size only where direct wiring is not yet practical.",
        "",
        "Retained four-way coordinates:",
        *(
            f"  {x + SHEET_OFFSET_X:.3f}, {y + SHEET_OFFSET_Y:.3f}"
            for x, y in four_way_junctions
        ),
    ]
    (OUT / "pico_lta042b010f_carrier-junction-cleanup.txt").write_text("\n".join(report) + "\n", encoding="utf-8")

    root.extend([
        # Power labels are kept only at the rails and off-board interfaces;
        # the component-to-component nets are visibly wired.
        hidden_label("+5V", 127.00, 82.55),
        hidden_label("IPK_SENSE", 179.07, 96.52),
        hidden_label("TC_TIMING", 149.86, 106.68), hidden_label("+13V8", 190.50, 82.55),
        hidden_label("VFB", 195.58, 116.84), hidden_label("PWR_LED_P", 224.79, 102.87),
        hidden_label("CPUMP_MID", 242.57, 104.14), hidden_label("NEG_LED_N", 299.72, 123.19),
        hidden_label("+5V", 158.75, 96.52), hidden_label("IPK_SENSE", 153.67, 82.55),
        hidden_label("IPK_SENSE", 160.02, 82.55), hidden_label("SW_NODE", 167.64, 82.55),
        hidden_label("+13V8", 184.15, 82.55), hidden_label("GND", 168.91, 114.30),
        hidden_label("GND", 179.07, 106.68), hidden_label("GND", 149.86, 119.38),
        hidden_label("GND", 190.50, 97.79), hidden_label("+13V8", 208.28, 90.17),
        hidden_label("GND", 208.28, 119.38), hidden_label("+13V8", 224.79, 90.17),
        hidden_label("GND", 224.79, 110.49), hidden_label("SW_NODE", 242.57, 92.71),
        hidden_label("CPUMP_MID", 247.65, 110.49), hidden_label("GND", 247.65, 118.11),
        hidden_label("-13V8", 264.16, 104.14), hidden_label("-13V8", 278.13, 110.49),
        hidden_label("GND", 278.13, 118.11), hidden_label("GND", 274.32, 156.21),
        hidden_label("GND", 279.40, 184.15), hidden_label("DRIVER_DC", 153.67, 99.06),
        hidden_label("GND", 134.62, 102.87),
        hidden_label("GND", 130.81, 109.22), hidden_label("+5V", 146.05, 99.06),
        hidden_label("+5V", 130.81, 101.60),
        hidden_label("GND", 299.72, 130.81), hidden_label("+5V", 130.81, 82.55),
        label("+3V3", 279.40, 168.91), label("GND", 125.73, gnd_y),
        label("GND", 279.40, 190.50),
        label("+13V8", 196.85, plus_y, 180),
        label("-13V8", 285.75, 104.14, 180), hidden_label("SW_NODE", 176.53, 82.55, 180),
        hidden_label("SW_NODE", 179.07, 101.60), label("+13V8", 270.51, 153.67),
        label("VCPP_ADJ", 287.02, 180.34),
    ])
    root.extend([
        power_symbol("power:+5V", "#PWR0101", 127.00, 82.55),
        power_symbol("power:+3V3", "#PWR0103", 279.40, 176.53, hide_value=True),
        power_symbol("power:PWR_FLAG", "#FLG0101", 130.81, 82.55, hide_value=True),
        power_symbol("power:PWR_FLAG", "#FLG0102", 125.73, gnd_y, hide_value=True),
        power_symbol("power:PWR_FLAG", "#FLG0103", 196.85, 82.55, hide_value=True),
        power_symbol("power:PWR_FLAG", "#FLG0104", 285.75, 104.14, hide_value=True),
    ])
    # Center the assembled sheet objects on a readable A4 landscape page.
    root = [translate_sheet_block(block, SHEET_OFFSET_X, SHEET_OFFSET_Y) for block in root]
    if not sheet_instance:
        raise ValueError("sheet_instances missing")
    root.append(sheet_instance)

    header = source[:lib_start]
    header = header.replace('(paper "A3")', '(paper "A4")')
    header = header.replace('"Codex draft - verify against prototype"', '"Codex reviewed KiCad draft"')
    header = header.replace('"FFC connector contact side and exact part number are provisional"', '"Pico 2 W hand-solder footprint and TE FFC footprint selected from KiCad libraries"')
    schematic = header + new_lib + "\n" + "\n\n".join(root) + "\n)\n"
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "pico_lta042b010f_carrier.kicad_sch").write_text(schematic, encoding="utf-8")

    custom_pico = tune_pico_symbol_visuals(extract_symbol(lib_block, "Custom:Pico_2W_40P")).replace('(symbol "Custom:Pico_2W_40P"', '(symbol "Pico_2W_40P"', 1)
    custom_lcd = extract_symbol(lib_block, "Custom:LTA042B010F_FFC36").replace('(symbol "Custom:LTA042B010F_FFC36"', '(symbol "LTA042B010F_FFC36"', 1)
    custom_lcd = custom_lcd.replace('(pin power_in line', '(pin passive line')
    custom_lib = '(kicad_symbol_lib (version 20241209) (generator "kicad_symbol_editor") (generator_version "9.0")\n' + custom_pico + "\n" + custom_lcd + "\n)\n"
    (OUT / "pico_lta042b010f_carrier.kicad_sym").write_text(custom_lib, encoding="utf-8")
    (OUT / "sym-lib-table").write_text('(sym_lib_table\n  (version 7)\n  (lib (name "Custom")(type "KiCad")(uri "${KIPRJMOD}/pico_lta042b010f_carrier.kicad_sym")(options "")(descr "Project-specific Pico and LCD symbols"))\n)\n', encoding="utf-8")
    (OUT / "fp-lib-table").write_text(
        (ROOT / "fp-lib-table").read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    out_footprints = OUT / "footprints.pretty"
    out_footprints.mkdir(parents=True, exist_ok=True)
    for footprint in PROJECT_FOOTPRINTS.glob("*.kicad_mod"):
        (out_footprints / footprint.name).write_text(
            footprint.read_text(encoding="utf-8"),
            encoding="utf-8",
        )
    out_models = OUT / "3dmodels"
    out_models.mkdir(parents=True, exist_ok=True)
    for model in (ROOT / "3dmodels").glob("*"):
        if model.is_file():
            (out_models / model.name).write_bytes(model.read_bytes())

    (OUT / "pico_lta042b010f_carrier.kicad_pro").write_text(SRC_PRO.read_text(encoding="utf-8"), encoding="utf-8")


def build_pcb(with_artwork: bool = True) -> None:
    source = SRC_PCB.read_text(encoding="utf-8")
    objects = top_level_blocks(source[source.index('(net 0 "")'):])
    footprints: dict[str, str] = {}
    for block in objects:
        if block.startswith("(footprint"):
            ref_match = re.search(r'\(property "Reference" "([^"]+)"', block)
            if ref_match:
                footprints[ref_match.group(1)] = block
    net_names = {match.group(2): match.group(1) for match in re.finditer(r'\(net\s+(\d+)\s+"([^"]*)"\)', source)}
    explicit = {
        "D4": {"1": ("+13V8", "K"), "2": ("SW_NODE", "A")}, "D6": {"1": ("GND", "K"), "2": ("CPUMP_MID", "A")}, "D7": {"1": ("-13V8", "K"), "2": ("CPUMP_MID", "A")}, "D5": {"1": ("GND", "K"), "2": ("PWR_LED_P", "A")}, "D8": {"1": ("-13V8", "K"), "2": ("NEG_LED_N", "A")},
        "C9": {"1": ("SW_NODE", "+"), "2": ("CPUMP_MID", "-")},
        "J2": {"1": ("+5V", "VBUS"), "2": ("GND", "GND")},
    }

    def nets_for(ref: str) -> dict[str, dict[str, str]]:
        if ref in explicit:
            return {pad: {"number": net_names[net], "name": net, "pinfunction": function, "pintype": "passive"} for pad, (net, function) in explicit[ref].items()}
        testpoint_nets = {"TP1": "+5V", "TP2": "+13V8", "TP3": "-13V8", "TP4": "GND"}
        if ref in testpoint_nets:
            net = testpoint_nets[ref]
            return {"1": {"number": net_names[net], "name": net, "pinfunction": net, "pintype": "passive"}}
        return pad_net_map(footprints.get(ref, ""))

    standard = {
        "J1": (KI_FOOTPRINTS / "Connector_FFC-FPC.pretty/TE_3-1734839-6_1x36-1MP_P0.5mm_Horizontal.kicad_mod", "Connector_FFC-FPC", "LTA042B010F_FFC_36P_0.5mm", (20, 9), 0),
        # Leave a continuous routing aisle between the Pico and the switching
        # section.  This also gives the four right-side red LCD bits a clean
        # B.Cu drop without running through the input capacitor.
        # Placement-first candidate: keep the LCD connector and Pico on the
        # left, reserve a continuous routing aisle, and make the complete
        # switching/charge-pump section a compact right-side block.
        "U1": (KI_FOOTPRINTS / "Module.pretty/RaspberryPi_Pico_W_SMD_HandSolder.kicad_mod", "Module", "RaspberryPi_Pico_2W", (51, 38), 0),
        "J2": (PROJECT_FOOTPRINTS / "A295_CTRPB_1.kicad_mod", "Custom", "USB-C 5V INPUT", (3.4, 28.0), 90),
        "J3": (KI_FOOTPRINTS / "Connector_PinHeader_2.54mm.pretty/PinHeader_1x02_P2.54mm_Vertical.kicad_mod", "Connector_PinHeader_2.54mm", "CCFL_INVERTER_13V8", (110, 8), 0),
        "RV1": (KI_FOOTPRINTS / "Potentiometer_THT.pretty/Potentiometer_Runtron_RM-065_Vertical.kicad_mod", "Potentiometer_THT", "10k_CONTRAST_TRIM", (44, -54), 0),
        "U3": (KI_FOOTPRINTS / "Package_SO.pretty/SOIC-8_3.9x4.9mm_P1.27mm.kicad_mod", "Package_SO", "MC34063AD", (88, 24), 0),
        "R7": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0411_L9.9mm_D3.6mm_P5.08mm_Vertical.kicad_mod", "Resistor_THT", "0.47R 1%", (70, 15), 0),
        "R8": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0204_L3.6mm_D1.6mm_P2.54mm_Vertical.kicad_mod", "Resistor_THT", "200 1%", (55, -68), 0),
        "R9": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical.kicad_mod", "Resistor_THT", "10k 0.1%", (64, -67), 90),
        "R10": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical.kicad_mod", "Resistor_THT", "1k 0.1%", (64, -54), 90),
        "R11": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical.kicad_mod", "Resistor_THT", "10k 0.1%", (88, -89), 90),
        "R16": (KI_FOOTPRINTS / "Resistor_THT.pretty/R_Axial_DIN0207_L6.3mm_D2.5mm_P2.54mm_Vertical.kicad_mod", "Resistor_THT", "10k 0.1%", (78, -56), 90),
        "L1": (KI_FOOTPRINTS / "Inductor_SMD.pretty/L_Taiyo-Yuden_NR-10050_9.8x10.0mm_HandSoldering.kicad_mod", "Inductor_SMD", "100uH", (89, 15), 0), "D4": (KI_FOOTPRINTS / "Diode_THT.pretty/D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp.kicad_mod", "Diode_THT", "1N5819", (106, 22), 180), "D6": (KI_FOOTPRINTS / "Diode_THT.pretty/D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp.kicad_mod", "Diode_THT", "1N5819", (102, 50), 90), "D7": (KI_FOOTPRINTS / "Diode_THT.pretty/D_DO-41_SOD81_P2.54mm_Vertical_KathodeUp.kicad_mod", "Diode_THT", "1N5819", (115, 35), 180), "D5": (KI_FOOTPRINTS / "LED_THT.pretty/LED_D3.0mm.kicad_mod", "LED_THT", "GREEN LED 3mm", (116, 29), 90), "D8": (KI_FOOTPRINTS / "LED_THT.pretty/LED_D3.0mm.kicad_mod", "LED_THT", "RED LED 3mm", (106, 61), 90),
        "C6": (KI_FOOTPRINTS / "Capacitor_THT.pretty/CP_Radial_D8.0mm_P3.50mm.kicad_mod", "Capacitor_THT", "220uF 35V", (78, 25), 0), "C8": (KI_FOOTPRINTS / "Capacitor_THT.pretty/CP_Radial_D5.0mm_P2.00mm.kicad_mod", "Capacitor_THT", "47uF 35V", (110, 23), 0), "C9": (KI_FOOTPRINTS / "Capacitor_THT.pretty/CP_Radial_D5.0mm_P2.00mm.kicad_mod", "Capacitor_THT", "10uF 50V", (101, 29), 0), "C11": (KI_FOOTPRINTS / "Capacitor_THT.pretty/CP_Radial_D5.0mm_P2.00mm.kicad_mod", "Capacitor_THT", "10uF 50V (+ to GND)", (115, 43), 0), "C7": (KI_FOOTPRINTS / "Capacitor_THT.pretty/C_Disc_D5.0mm_W2.5mm_P2.50mm.kicad_mod", "Capacitor_THT", "470pF 50V C0G", (82, 32), 0),
    }
    for index, ref in enumerate(["TP1", "TP2", "TP3", "TP4"]):
        standard[ref] = (KI_FOOTPRINTS / "TestPoint.pretty/TestPoint_Pad_D2.0mm.kicad_mod", "TestPoint", {"TP1": "+5V", "TP2": "+13V8", "TP3": "-13V8", "TP4": "GND"}[ref], (10, 65), 0)
    testpoint_positions = {"TP1": (10, 65), "TP2": (14, 65), "TP3": (18, 65), "TP4": (22, 65)}
    for ref, position in testpoint_positions.items():
        path, library, value, _, rotation = standard[ref]
        standard[ref] = (path, library, value, position, rotation)
    for ref, position in {"H1": (4, 5), "H2": (115, 5), "H3": (5, 65), "H4": (115, 65)}.items():
        standard[ref] = (KI_FOOTPRINTS / "MountingHole.pretty/MountingHole_3.2mm_M3.kicad_mod", "MountingHole", "MountingHole", position, 0)

    rendered: list[str] = []
    for ref, (path, library, value, position, rotation) in standard.items():
        if not path.exists():
            raise FileNotFoundError(path)
        rendered.append(make_footprint(path, library, ref, value, position, nets_for(ref), rotation))

    if not with_artwork:
        prefix = source[: source.index('(footprint')]
        graphics_positions = [source.find('(gr_rect'), source.find('(gr_line'), source.find('(gr_arc')]
        graphics_positions = [position for position in graphics_positions if position >= 0]
        if not graphics_positions:
            raise ValueError("board edge graphics not found")
        suffix = source[min(graphics_positions):]
        pcb = prefix + "\n\n".join(rendered) + "\n\n" + suffix
        OUT.mkdir(parents=True, exist_ok=True)
        (OUT / "pico_lta042b010f_carrier.kicad_pcb").write_text(pcb, encoding="utf-8")
        print("placement-only: 0 segments, 0 vias")
        return

    # Generate the first real artwork pass from the actual rendered pad
    # geometry.  Routing from the rendered footprints avoids a second,
    # hand-maintained list of pad coordinates drifting away from KiCad.
    routing_pads = parse_routing_pads(rendered)
    net_numbers = {name: int(number) for name, number in net_names.items()}
    router = ArtworkRouter(routing_pads, net_numbers, parse_footprint_keepouts(rendered))
    endpoint_map: dict[str, list[tuple[str, str, float, float]]] = {}
    for pad in routing_pads:
        net = str(pad["net"])
        if net:
            endpoint_map.setdefault(net, []).append(
                (str(pad["ref"]), str(pad["pad"]), float(pad["x"]), float(pad["y"]))
            )

    # FFC contacts are a dense 0.5 mm row.  Give every non-ground contact a
    # short, straight fan-out before the general router starts; otherwise the
    # conservative clearance model quite correctly treats the neighboring
    # contact pads as a solid wall.  The logical endpoint is moved to the end
    # of that fan-out while the generated segment still terminates on the pad.
    # Connector escape slots are monotonic in pad-number order. Keeping the
    # slot table explicit makes the dense fanout auditable and reproducible.
    connector_escape_slots = {
        pad: slot
        for slot, pad in enumerate([
            2, 4, 5, 8, 9, 10, 11, 12, 13, 15, 16, 17, 18, 19, 20,
            21, 22, 23, 24, 25, 26, 27, 28, 29, 31, 33, 35,
        ])
    }
    # Source vias stay clear of the twelve left-side B/G bus levels.  The
    # 1.0-mm minimum spacing is conservative for the 0.6-mm hand-solder via.
    connector_escape_y = [
        12.0, 13.0, 14.0, 15.0, 16.0, 17.0, 18.0, 19.0, 20.0,
        21.0, 22.0, 23.0, 24.0, 25.0, 26.0, 25.5, 31.25, 33.75,
        36.25, 37.25, 38.25, 41.25, 54.0, 43.75, 62.0, 64.0, 67.0,
    ]
    def fanout_y(net: str, pad_number: str) -> float:
        number = int(pad_number)
        if net.startswith("LCD_B") and 8 <= number <= 13:
            return 18.0 + (number - 8) * 2.0
        if net.startswith("LCD_B") and 8 <= number <= 13:
            return 12.0 + (number - 8) * 2.0
        if net.startswith("LCD_G") and 15 <= number <= 20:
            return 30.0 + (number - 15) * 2.0
        if net.startswith("LCD_G") and 15 <= number <= 20:
            return 24.0 + (number - 15) * 2.0
        if net.startswith("LCD_R") and 22 <= number <= 27:
            return 44.0 + (number - 22) * 2.0
        if net.startswith("LCD_R") and 22 <= number <= 27:
            # The red bus exits on the right side in a non-monotonic pin
            # order. Keep connector-side escapes in a predictable order so
            # the long B.Cu bus needs fewer crossovers.
            red_y = {
                "LCD_R0": 36.0, "LCD_R1": 38.0, "LCD_R2": 40.0,
                "LCD_R3": 42.0, "LCD_R4": 44.0, "LCD_R5": 46.0,
            }
            return red_y[net]
        if net in {"LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC"}:
            # The right-side timing pins run upward from NCLK to VSYNC.
            timing_y = {"LCD_NCLK": 52.0, "LCD_HSYNC": 50.0, "LCD_VSYNC": 48.0}
            return timing_y[net]
        # Connector power and contrast contacts are routed after LCD signals.
        power_y = {
            # Power escapes stay close to J1; their long distribution legs
            # start on B.Cu instead of becoming large F.Cu diagonals across
            # the signal area.
            "+3V3": 14.0,
            "VCPP_ADJ": 16.0,
            "+5V": 18.0,
            "-13V8": 20.0,
            "+13V8": 22.0,
        }
        return power_y.get(net, 13.0)

    def fanout_x(net: str, pad_number: str, pad_x: float) -> float:
        number = int(pad_number)
        if net.startswith("LCD_B") and 8 <= number <= 13:
            return 16.0 + (number - 8) * 1.25
        if net.startswith("LCD_B") and 8 <= number <= 13:
            return 14.0 + (number - 8) * 1.5
        if net.startswith("LCD_G") and 15 <= number <= 20:
            return 21.25 + (number - 15) * 1.25
        if net.startswith("LCD_G") and 15 <= number <= 20:
            return 18.0 + (number - 15) * 1.5
        if net.startswith("LCD_R") and 22 <= number <= 27:
            return 30.0 + (number - 22) * 1.25
        if net.startswith("LCD_R") and 22 <= number <= 27:
            return 22.0 + (27 - number) * 1.5
        if net in {"LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC"}:
            timing_x = {"LCD_NCLK": 10.0, "LCD_HSYNC": 12.0, "LCD_VSYNC": 14.0}
            return timing_x[net]
        power_x = {
            "+3V3": {"21": 31.0, "28": 34.0},
            "VCPP_ADJ": {"29": 36.0},
            "+5V": {"31": 38.0},
            "-13V8": {"33": 40.0},
            "+13V8": {"35": 42.0},
        }
        return power_x.get(net, {}).get(pad_number, pad_x)

    source_vias: dict[tuple[str, str], tuple[float, float]] = {}
    for net, endpoints in endpoint_map.items():
        for index, (ref, pad_number, x, y) in enumerate(endpoints):
            if ref != "J1" or net == "GND":
                continue
            # Keep the source pad here; the short escape is added immediately
            # before that net is routed.  Pre-adding every escape would create
            # an artificial wall for the next A* route in the dense connector
            # area.
            source_vias[(net, pad_number)] = (x, y)

    # The plane handles the bulk of the return current.  These short F.Cu
    # stubs bring isolated SMD ground pads to B.Cu without routing a noisy
    # ground daisy-chain through the switching area.
    router.add_gnd_stubs(endpoint_map.get("GND", []))

    root_by_net = {
        "+5V": ("C6", "1"),
        "+13V8": ("C8", "1"),
        "-13V8": ("C11", "2"),
        "+3V3": ("U1", "36"),
        "IPK_SENSE": ("L1", "1"),
        "SW_NODE": ("L1", "2"),
        "CPUMP_MID": ("C9", "2"),
        "DRIVER_DC": ("U3", "8"),
        "TC_TIMING": ("U3", "3"),
        "VFB": ("U3", "5"),
        "PWR_LED_P": ("R11", "2"),
        "NEG_LED_N": ("D8", "2"),
        "VCPP_ADJ": ("RV1", "2"),
    }
    power_order = [
        "IPK_SENSE", "SW_NODE", "CPUMP_MID", "DRIVER_DC", "TC_TIMING",
        "VFB", "PWR_LED_P", "NEG_LED_N",
    ]
    lcd_order = [
        "LCD_G0", "LCD_G1", "LCD_G2", "LCD_G3", "LCD_G4", "LCD_G5",
        "LCD_B0", "LCD_B1", "LCD_B2", "LCD_B3", "LCD_B4", "LCD_B5",
        "LCD_R0", "LCD_R1", "LCD_R2", "LCD_R3", "LCD_R4", "LCD_R5",
        "LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC",
    ]
    # Route the quiet Pico rail before the high-current +5V distribution so
    # its local anchor is not boxed in by the later wide power trunk.
    connector_power_order = ["+3V3", "+5V", "+13V8", "-13V8", "VCPP_ADJ"]
    hot_nets = {"IPK_SENSE", "SW_NODE", "CPUMP_MID", "DRIVER_DC", "TC_TIMING", "VFB"}
    wide_nets = {"+5V", "+13V8", "-13V8"}
    quiet_two_layer_nets = {"+3V3", "PWR_LED_P", "NEG_LED_N", "VCPP_ADJ"}
    direct_lcd_nets = {
        "LCD_B5", "LCD_B4", "LCD_B3", "LCD_B2", "LCD_B1", "LCD_B0",
        "LCD_G5", "LCD_G4", "LCD_G3", "LCD_G2", "LCD_G1", "LCD_G0",
    }

    def u1_anchor(item: tuple[str, str, float, float]) -> tuple[float, float]:
        # Keep the destination via outside the Pico pad body; the final
        # F.Cu segment is a short, explicit dogbone into the SMD pad.
        pico_x = float(standard["U1"][3][0])
        return (pico_x - 12.5 if item[2] < pico_x else pico_x + 14.0, item[3])

    def route_lcd_from_vias(
        net: str,
        start_item: tuple[str, str, float, float],
        target_item: tuple[str, str, float, float],
        width: float,
    ) -> None:
        if net.startswith("LCD_R"):
            # Red starts leave through the free top margin.  Let the
            # clearance-aware router choose the layer change and final aisle;
            # a fixed full-height red trunk would cross later top fan-outs.
            pad_start = source_vias[(net, start_item[1])]
            top_y = {
                "LCD_R5": 3.0,
                "LCD_R4": 3.5,
                "LCD_R3": 4.0,
                "LCD_R2": 4.5,
                "LCD_R1": 5.0,
                "LCD_R0": 5.5,
            }[net]
            router.add_fixed_segment(
                net, pad_start, (pad_start[0], top_y), "F.Cu", width
            )
            target = u1_anchor(target_item)
            router.add_fixed_via(net, target)
            router.add_fixed_segment(
                net, target, (target_item[2], target_item[3]), "F.Cu", width
            )
            router.route_pair(
                net,
                (pad_start[0], top_y),
                target,
                width,
                preferred_layer=1,
                via_cost=18.0,
                via_min_x=28.0,
                via_max_x=72.0,
                via_min_y=8.0,
                start_layer=0,
                goal_layer=1,
            )
            return
            # The red bank terminates on both sides of the Pico.  Use ordered
            # top-edge fan-out lanes, then keep the right-side drops on B.Cu
            # (outside the SMD pad field) and the left-side drops on F.Cu
            # (above the parallel B/G bus).  This is a deterministic escape
            # rather than a late A* route through already occupied corridors.
            pad_start = source_vias[(net, start_item[1])]
            top_y = {
                "LCD_R5": 3.0,
                "LCD_R4": 3.5,
                "LCD_R3": 4.0,
                "LCD_R2": 4.5,
                "LCD_R1": 5.0,
                "LCD_R0": 5.5,
            }[net]
            target = u1_anchor(target_item)
            router.add_fixed_via(net, target)
            router.add_fixed_segment(
                net, target, (target_item[2], target_item[3]), "F.Cu", width
            )
            router.add_fixed_segment(
                net, pad_start, (pad_start[0], top_y), "F.Cu", width
            )
            if target_item[2] < 50.0:
                corridor_x = {"LCD_R5": 38.5, "LCD_R4": 40.0}[net]
                router.add_fixed_segment(
                    net, (pad_start[0], top_y), (corridor_x, top_y), "F.Cu", width
                )
                router.add_fixed_segment(
                    net, (corridor_x, top_y), (corridor_x, target[1]), "F.Cu", width
                )
                router.add_fixed_segment(
                    net, (corridor_x, target[1]), target, "F.Cu", width
                )
            else:
                corridor_x = {
                    "LCD_R0": 64.5,
                    "LCD_R1": 66.0,
                    "LCD_R2": 68.0,
                    "LCD_R3": 70.0,
                }[net]
                top_via = (corridor_x, top_y)
                router.add_fixed_segment(
                    net, (pad_start[0], top_y), top_via, "F.Cu", width
                )
                router.add_fixed_via(net, top_via)
                router.add_fixed_segment(net, top_via, (corridor_x, target[1]), "B.Cu", width)
                router.add_fixed_segment(net, (corridor_x, target[1]), target, "B.Cu", width)
            return
        if net in {"LCD_NCLK", "LCD_HSYNC", "LCD_VSYNC"}:
            # Timing lines use the three remaining top-edge lanes.  They stay
            # on F.Cu so they do not cross the B.Cu red drops, then approach
            # the Pico's right edge through a dedicated vertical aisle.
            pad_start = source_vias[(net, start_item[1])]
            top_y = {
                "LCD_NCLK": 6.0,
                "LCD_HSYNC": 6.5,
                "LCD_VSYNC": 7.0,
            }[net]
            corridor_x = {
                "LCD_NCLK": 62.0,
                "LCD_HSYNC": 63.5,
                "LCD_VSYNC": 65.0,
            }[net]
            target = u1_anchor(target_item)
            router.add_fixed_via(net, target)
            router.add_fixed_segment(
                net, target, (target_item[2], target_item[3]), "F.Cu", width
            )
            router.add_fixed_segment(
                net, pad_start, (pad_start[0], top_y), "F.Cu", width
            )
            router.add_fixed_segment(
                net, (pad_start[0], top_y), (corridor_x, top_y), "F.Cu", width
            )
            router.add_fixed_segment(
                net, (corridor_x, top_y), (corridor_x, target[1]), "F.Cu", width
            )
            router.add_fixed_segment(
                net, (corridor_x, target[1]), target, "F.Cu", width
            )
            return
        pad_start = source_vias[(net, start_item[1])]
        escape = (
            pad_start[0],
            10.0 + connector_escape_slots[int(start_item[1])] * 0.75,
        )
        router.add_fixed_segment(net, pad_start, escape, "F.Cu", 0.15)
        start = escape
        target = u1_anchor(target_item)
        router.add_fixed_via(net, target)
        router.add_fixed_segment(net, target, (target_item[2], target_item[3]), "F.Cu", width)
        # Start at the actual FFC pad.  A* chooses the first legal layer
        # change in open board area; a through-via row is not manufacturable at
        # the connector's 0.5 mm pitch.
        router.route_pair(
            net,
            start,
            target,
            width,
            preferred_layer=0 if net.startswith("LCD_R") else 1,
            via_cost=3000.0 if net.startswith("LCD_R") else 18.0,
            via_min_x=20.0,
            via_max_x=72.0,
            via_min_y=10.0,
            start_layer=0,
            goal_layer=0 if net.startswith("LCD_R") else 1,
        )
        return
        if net in {"LCD_VSYNC", "LCD_HSYNC", "LCD_NCLK"}:
            # Timing lines leave the connector on three reserved upper lanes
            # and descend at the right side of the Pico. Keeping this block
            # before the RGB buses prevents the timing trunk from becoming a
            # late A* obstacle that is impossible to escape around.
            timing = {
                "LCD_VSYNC": ((71.0, 14.0), 71.0),
                "LCD_HSYNC": ((73.0, 13.0), 73.0),
                "LCD_NCLK": ((74.0, 12.0), 74.0),
            }
            corner, corridor_x = timing[net]
            router.add_fixed_segment(net, start, corner, "B.Cu", width)
            router.add_fixed_segment(net, corner, (corridor_x, target[1]), "B.Cu", width)
            router.add_fixed_segment(net, (corridor_x, target[1]), target, "B.Cu", width)
            return
        if net.startswith("LCD_B") or net.startswith("LCD_G"):
            # The left-side Pico pads have a monotonic vertical order.  Give
            # each source a second via at its destination height: the source
            # leg is vertical on F.Cu and the long bus is horizontal on B.Cu.
            # This keeps the dense connector escape and the left bus planar,
            # without diagonal fan-outs or same-layer crossovers.
            landing = (start[0], target_item[3])
            router.add_fixed_via(net, landing)
            router.add_fixed_segment(net, start, landing, "F.Cu", width)
            router.add_fixed_segment(net, landing, target, "B.Cu", width)
            return
        if net.startswith("LCD_R"):
            # The red contacts terminate on both sides of the Pico.  Keep the
            # two left-side drops on their destination heights, and use four
            # nested B.Cu corridors for the right-side drops.  The corridors
            # approach from below the module's antenna keepout and their
            # intervals are ordered so they cannot cross one another.
            if net == "LCD_R0":
                # R0 shares a height with the lower timing/RGB area.  Change
                # layer at a point just outside the left bus, then approach
                # the right-side pad on F.Cu so it does not cross NCLK.
                landing = (45.0, 54.5)
                router.add_fixed_via(net, landing)
                router.add_fixed_segment(net, start, (45.0, start[1]), "B.Cu", width)
                router.add_fixed_segment(net, (45.0, start[1]), landing, "B.Cu", width)
                router.add_fixed_segment(net, landing, (78.0, 54.5), "F.Cu", width)
                router.add_fixed_segment(net, (78.0, 54.5), (78.0, target[1]), "F.Cu", width)
                router.add_fixed_segment(net, (78.0, target[1]), target, "F.Cu", width)
                return
            if target_item[2] < 50.0:
                landing_y = target_item[3]
                corridor_x = 43.5
            else:
                landing_y = {
                    "LCD_R3": 61.0,
                    "LCD_R2": 59.0,
                    "LCD_R1": 56.5,
                }[net]
                corridor_x = {
                    "LCD_R3": 75.0,
                    "LCD_R2": 76.0,
                    "LCD_R1": 77.0,
                }[net]
            landing = (start[0], landing_y)
            router.add_fixed_via(net, landing)
            router.add_fixed_segment(net, start, landing, "F.Cu", width)
            if target_item[2] < 50.0:
                router.add_fixed_segment(net, landing, target, "B.Cu", width)
            else:
                router.add_fixed_segment(net, landing, (corridor_x, landing_y), "B.Cu", width)
                router.add_fixed_segment(net, (corridor_x, landing_y), (corridor_x, target[1]), "B.Cu", width)
                router.add_fixed_segment(net, (corridor_x, target[1]), target, "B.Cu", width)
            return
        # The connector escape is already a clean F.Cu vertical stub ending
        # in a dedicated via. Keep the long LCD runs on B.Cu so they can pass
        # under the Pico body without crossing the other signal fan-outs.
        # A* handles the ordering and avoids previously committed runs;
        # this is deliberately preferable to hand-drawn diagonals in the
        # 0.5-mm-pitch connector area.
        router.route_pair(
            net,
            start,
            target,
            width,
            preferred_layer=1,
            via_cost=2000.0,
            via_min_x=30.0,
            via_max_x=72.0,
            via_min_y=11.5,
            start_layer=1,
            goal_layer=1,
        )

    def fixed_path(net: str, points: list[tuple[float, float]], layer: str, width: float) -> None:
        for start, end in zip(points, points[1:]):
            router.add_fixed_segment(net, start, end, layer, width)

    def fixed_via_path(net: str, pad: tuple[float, float], via: tuple[float, float], width: float) -> None:
        router.add_fixed_segment(net, pad, via, "F.Cu", width)
        router.add_fixed_via(net, via)

    # VSYNC is the lowest of the right-edge timing destinations. Reserve it
    # before the quiet +3V3 perimeter branch so that branch cannot seal its
    # final B.Cu approach.
    # Complete the parallel LCD bus while both copper layers are still mostly
    # open.  The switching/power routes are added afterward as fixed local
    # geometry, so they cannot seal the signal escape corridors first.
    route_order = lcd_order + power_order + ["+5V", "+3V3", "+13V8", "-13V8", "VCPP_ADJ"]
    for net in route_order:
        endpoints = endpoint_map.get(net, [])
        if len(endpoints) < 2:
            continue
        is_lcd = net.startswith("LCD_")
        # The 0.5 mm-pitch LCD bus is routed as 0.15 mm signal traces.  This
        # is a conservative hand-assembly-friendly width for short 3.3 V
        # logic runs and preserves the required 0.2 mm copper clearance.
        width = 0.15 if is_lcd else 0.3
        if net in hot_nets:
            width = 0.35
        elif net in wide_nets:
            width = 0.6
        if is_lcd:
            start = next(item for item in endpoints if item[0] == "J1")
            goal = next(item for item in endpoints if item[0] == "U1")
            route_lcd_from_vias(net, start, goal, width)
            continue
        if net == "IPK_SENSE":
            # Keep the current-sense connection away from the SW node and
            # bring it into L1 from the quiet upper side of the footprint.
            fixed_path(net, [(87.475, 34.365), (91.0, 34.365), (91.0, 30.0), (80.3, 30.0), (80.3, 22.0)], "F.Cu", 0.35)
            fixed_path(net, [(82.16, 12.0), (82.16, 18.75), (80.3, 18.75), (80.3, 22.0)], "F.Cu", 0.35)
            continue
        if net == "SW_NODE":
            # The high di/dt loop is kept short on F.Cu: L1 -> D4 is a
            # straight run, and U3 pin 1 approaches L1 from below.
            fixed_path(net, [(89.7, 22.0), (93.0, 22.0)], "F.Cu", 0.5)
            fixed_path(net, [(82.525, 33.095), (84.5, 33.095), (84.5, 27.0), (89.7, 27.0), (89.7, 22.0)], "F.Cu", 0.5)
            continue
        if net == "CPUMP_MID":
            # C9, D6 and D7 form the charge-pump loop. They are through-hole
            # pads, so the short top-side connections do not need vias.
            fixed_path(net, [(98.0, 28.0), (105.0, 31.84)], "F.Cu", 0.35)
            fixed_path(net, [(98.0, 28.0), (118.16, 22.0)], "F.Cu", 0.35)
            continue
        if net == "TC_TIMING":
            timing_via = (78.0, 35.635)
            fixed_via_path(net, (82.525, 35.635), timing_via, 0.3)
            fixed_path(net, [timing_via, (78.0, 50.0), (72.0, 50.0)], "B.Cu", 0.3)
            continue
        if net == "VFB":
            # Route the feedback divider on B.Cu in its own corridor, away
            # from the SW/output loop and the positive rail.
            u3_via = (90.5, 37.5)
            r9_via = (99.0, 34.087)
            r10_via = (99.0, 48.913)
            fixed_via_path(net, (87.475, 36.905), u3_via, 0.3)
            fixed_via_path(net, (101.0, 34.087), r9_via, 0.3)
            fixed_via_path(net, (101.0, 48.913), r10_via, 0.3)
            fixed_path(net, [u3_via, (90.5, 42.0), (99.0, 42.0), r9_via], "B.Cu", 0.3)
            fixed_path(net, [(99.0, 42.0), r10_via], "B.Cu", 0.3)
            continue
        if net == "PWR_LED_P":
            # Go around the adjacent +13V8 pad on R11 rather than passing
            # through the two-pad footprint.
            fixed_path(net, [(109.0, 34.087), (111.0, 34.087), (111.0, 44.062), (109.0, 44.062)], "F.Cu", 0.3)
            continue
        if net == "NEG_LED_N":
            # Likewise route around D8's -13V8 pad and the R16 ground pad.
            fixed_path(net, [(107.0, 54.062), (109.5, 54.062), (109.5, 63.913), (107.0, 63.913)], "F.Cu", 0.3)
            continue
        if net == "+3V3":
            # +3V3 is a quiet rail: distribute it on B.Cu from the Pico-side
            # anchor, with both J1 contacts retained as separate branches.
            root = next(item for item in endpoints if item[:2] == ("U1", "36"))
            root_anchor = (70.0, 30.0)
            router.add_fixed_via(net, root_anchor)
            router.add_fixed_segment(net, root_anchor, (root[2], root[3]), "F.Cu", width)
            # The two J1 contacts are deliberately taken through separate
            # left-side B.Cu corridors.  Their levels are chosen between the
            # future B/G buses, so the signal artwork can remain monotonic.
            fixed_path(
                net,
                [root_anchor, (68.0, 30.0), (68.0, 25.5), source_vias[(net, "21")]],
                "B.Cu",
                width,
            )
            fixed_path(
                net,
                [root_anchor, (68.0, 30.0), (68.0, 54.0), source_vias[(net, "28")]],
                "B.Cu",
                width,
            )
            continue
        if net == "+5V":
            # Use the bulk input capacitor as the B.Cu distribution point.
            # SMD consumers leave the component side through dedicated
            # dogbone vias; connector/test-point/THT pads can join directly
            # on B.Cu. This avoids forcing a long route through the SOIC pin
            # field.
            root = next(item for item in endpoints if item[:2] == ("C6", "1"))
            width = 0.6

            def add_power_smd_anchor(item: tuple[str, str, float, float], anchor: tuple[float, float]) -> tuple[float, float]:
                router.add_fixed_via(net, anchor)
                router.add_fixed_segment(net, anchor, (item[2], item[3]), "F.Cu", width)
                return anchor

            targets: list[tuple[float, float]] = []
            for item in endpoints:
                if item[:2] == ("U1", "40"):
                    targets.append(add_power_smd_anchor(item, (68.5, 19.87)))
                elif item[:2] == ("U3", "6"):
                    targets.append(add_power_smd_anchor(item, (90.0, 35.635)))
                elif item[:2] == ("R8", "1"):
                    targets.append(add_power_smd_anchor(item, (80.0, 38.5)))
                elif item[0] == "J1":
                    # The short J1 fanout already connects the pad to its
                    # escape via. Approach that via on B.Cu so this wide
                    # branch stays off the dense F.Cu connector fan-out.
                    source_target = source_vias[(net, item[1])]
                else:
                    targets.append((item[2], item[3]))
            # The J1 branch uses an upper F.Cu perimeter and changes to B.Cu
            # only at the far-left side. This keeps the 0.6-mm rail out of
            # the LCD corridors and away from the connector fan-out tracks.
            if source_target is not None:
                # The connector escape is already tied to both copper layers
                # by its via. Use a dedicated B.Cu corridor above the Pico
                # antenna keepout, then drop on the left side of the LCD bus.
                # This keeps the wide input branch deterministic and away
                # from the later signal fan-outs.
                fixed_path(net, [(root[2], root[3]), (72.0, 6.0),
                                 (10.0, 6.0), (10.0, 62.0)], "B.Cu", width)
                router.add_fixed_via(net, (10.0, 62.0))
                fixed_path(net, [(10.0, 62.0), source_target], "B.Cu", width)
            for target in targets:
                if target == (root[2], 12.0):
                    continue
                router.route_pair(
                    net,
                    (root[2], root[3]),
                    target,
                    width,
                    preferred_layer=1,
                    via_cost=1500.0,
                    start_layer=1,
                    goal_layer=1,
                )
            continue
        if net == "+13V8":
            # Keep the output rail under the same clearance-aware router as
            # the rest of the board. D4 is the B.Cu root; the two divider/LED
            # pads leave F.Cu through dedicated vias, while THT pads and the
            # J1 escape are direct B.Cu destinations.
            root = (103.16, 22.0)
            targets: list[tuple[float, float]] = []
            for item in endpoints:
                if item[:2] == ("R9", "1"):
                    anchor = (103.5, 35.5)
                    fixed_via_path(net, (item[2], item[3]), anchor, width)
                    targets.append(anchor)
                elif item[:2] == ("R11", "1"):
                    anchor = (107.0, 35.5)
                    fixed_via_path(net, (item[2], item[3]), anchor, width)
                    targets.append(anchor)
                elif item[0] in {"J1", "TP2"}:
                    # These two endpoints use the dedicated left perimeter
                    # branch below, rather than a star route through the RGB
                    # signal field.
                    continue
                elif item[:2] not in {("D4", "1"), ("C9", "1"), ("C8", "1")}:
                    targets.append((item[2], item[3]))
            for target in targets:
                router.route_pair(
                    net,
                    root,
                    target,
                    width,
                    preferred_layer=1,
                    via_cost=15.0,
                    start_layer=1,
                    goal_layer=1,
                )
            router.route_pair(net, root, (98.0, 30.0), width, preferred_layer=0, via_cost=15.0, start_layer=0, goal_layer=0)
            router.route_pair(net, (98.0, 30.0), (95.0, 38.0), width, preferred_layer=0, via_cost=15.0, start_layer=0, goal_layer=0)
            # Feed the connector and the +13V8 test point from the top-left
            # perimeter. The branch stays above the antenna envelope while
            # crossing the board, then uses a quiet lower-left corridor.
            fixed_path(
                net,
                [root, (103.16, 10.0), (8.0, 10.0), (8.0, 67.0),
                 (20.0, 67.0), (20.0, 65.0)],
                "B.Cu",
                width,
            )
            fixed_path(net, [(20.0, 67.0), source_vias[(net, "35")]], "B.Cu", width)
            continue
        if net == "-13V8":
            # Keep the negative rail on the far-right side. C11 is the
            # B.Cu distribution root; D8 leaves through one dedicated via,
            # while D7, the J1 escape and TP3 are routed as separate branches.
            root = (115.0, 45.0)
            root_exit = (116.0, 45.0)
            router.add_fixed_segment(net, root, root_exit, "B.Cu", width)
            d8 = next(item for item in endpoints if item[:2] == ("D8", "1"))
            d8_anchor = (109.5, 55.938)
            fixed_via_path(net, (d8[2], d8[3]), d8_anchor, width)
            targets = [
                next(item for item in endpoints if item[:2] == ("D7", "1")),
                d8_anchor,
            ]
            for target_item in targets:
                target = (target_item[2], target_item[3]) if isinstance(target_item, tuple) and len(target_item) == 4 else target_item
                router.route_pair(
                    net,
                    root_exit,
                    target,
                    width,
                    preferred_layer=1,
                    via_cost=15.0,
                    start_layer=1,
                    goal_layer=1,
                )
            # The connector and TP3 are brought from a separate far-left
            # perimeter branch. Leaving C11 to the right is essential because
            # its adjacent pad 1 is GND.
            fixed_path(
                net,
                [root_exit, (116.0, 8.0), (6.0, 8.0), (6.0, 64.0),
                 (30.0, 64.0), (30.0, 65.0)],
                "B.Cu",
                width,
            )
            fixed_path(net, [(30.0, 64.0), source_vias[(net, "33")]], "B.Cu", width)
            continue
        if net == "VCPP_ADJ":
            # Contrast control is a quiet signal. Use a perimeter B.Cu route
            # and approach RV1 pad 2 vertically, avoiding the neighbouring
            # +3V3 and GND pads.
            vcpp_source = source_vias[(net, "29")]
            fixed_path(
                net,
                [vcpp_source, (25.25, 5.0), (77.5, 5.0), (77.5, 37.0),
                 (75.46, 37.0), (75.46, 60.0)],
                "F.Cu",
                0.3,
            )
            continue
        if net == "DRIVER_DC":
            # U3 pin 8 to the R8 driver resistor is a short local connection.
            # Bring both ends to B.Cu outside the SOIC pin field; routing a
            # direct F.Cu diagonal through the pin row would cross SW_NODE.
            u3 = next(item for item in endpoints if item[:2] == ("U3", "8"))
            r8 = next(item for item in endpoints if item[:2] == ("R8", "2"))
            u3_via = (90.0, 33.095)
            r8_via = (78.5, 33.0)
            fixed_via_path(net, (u3[2], u3[3]), u3_via, width)
            fixed_via_path(net, (r8[2], r8[3]), r8_via, width)
            fixed_path(net, [u3_via, r8_via], "B.Cu", width)
            continue
        router.route_net(
            net,
            endpoints,
            root_by_net.get(net),
            width,
            preferred_layer=1 if (is_lcd or net in quiet_two_layer_nets) else 0,
            via_cost=6.0 if (is_lcd or net in quiet_two_layer_nets) else 50.0,
            via_min_x=18.0 if is_lcd else None,
            via_max_x=71.0 if is_lcd else None,
            via_min_y=11.5 if is_lcd else None,
        )

    prefix = source[: source.index('(footprint')]
    graphics_positions = [source.find('(gr_rect'), source.find('(gr_line'), source.find('(gr_arc')]
    graphics_positions = [position for position in graphics_positions if position >= 0]
    if not graphics_positions:
        raise ValueError("board edge graphics not found")
    suffix = source[min(graphics_positions):]
    # Keep the board annotation legible and above the project's 0.8 mm
    # silkscreen minimum while retaining the native KiCad graphics.
    suffix = suffix.replace('(size 0.65 0.65)', '(size 0.8 0.8)').replace('(size 0.6 0.6)', '(size 0.8 0.8)')
    suffix = suffix.replace(
        "PICO 2W / LTA042B010F RGB666 + BIPOLAR POWER - PLACEMENT DRAFT",
        "PICO 2W / LTA042B010F RGB666 + BIPOLAR POWER",
    )
    suffix = suffix.replace("NO COPPER ROUTING YET", "2-LAYER HAND-SOLDER ROUTING")
    zone = make_copper_zone(
        net_numbers["GND"],
        "GND",
        "B.Cu",
        [(0.6, 0.6), (119.4, 0.6), (119.4, 69.4), (0.6, 69.4)],
        0.3,
        "GND_PLANE",
    )
    antenna_keepout = make_copper_keepout(
        "Pico 2W antenna copper keepout",
        [(43.5, 61.5), (58.5, 61.5), (58.5, 69.4), (43.5, 69.4)],
    )
    artwork = router.output() + "\n\n" + zone + "\n" + antenna_keepout
    pcb = prefix + "\n\n".join(rendered) + "\n\n" + artwork + "\n" + suffix
    print(f"artwork: {router.segment_count} segments, {router.via_count} vias")
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "pico_lta042b010f_carrier.kicad_pcb").write_text(pcb, encoding="utf-8")


def apply_generated_component_footprints(
    references: set[str],
    added_references: set[str] | None = None,
    removed_references: set[str] | None = None,
    obsolete_net_names: set[str] | None = None,
) -> None:
    """Replace only selected PCB footprint instances from the reviewed candidate.

    This preserves board graphics, zones, and unrelated manual PCB edits while
    applying the exact library geometry, pad stack, 3D model, and placement that
    were just generated for the requested component substitutions.
    """

    def footprint_spans(board: str) -> list[tuple[int, int, str]]:
        spans: list[tuple[int, int, str]] = []
        cursor = 0
        while True:
            start = board.find('(footprint "', cursor)
            if start < 0:
                return spans
            end = find_balanced(board, start)
            block = board[start:end]
            reference = re.search(r'\(property "Reference" "([^"]+)"', block)
            if reference:
                spans.append((start, end, reference.group(1)))
            cursor = end

    added_references = added_references or set()
    removed_references = removed_references or set()
    obsolete_net_names = obsolete_net_names or set()
    requested = references | added_references
    candidate_path = OUT / "pico_lta042b010f_carrier.kicad_pcb"
    candidate = candidate_path.read_text(encoding="utf-8")
    candidate_blocks = {
        reference: candidate[start:end]
        for start, end, reference in footprint_spans(candidate)
        if reference in requested
    }
    target = SRC_PCB.read_text(encoding="utf-8")
    target_spans = footprint_spans(target)
    target_references = {reference for _, _, reference in target_spans}
    missing = requested - candidate_blocks.keys() | references - target_references
    if missing:
        raise ValueError(f"footprints not found for replacement: {sorted(missing)}")

    replace_references = references | (added_references & target_references)
    insert_references = added_references - target_references
    result: list[str] = []
    cursor = 0
    for start, end, reference in target_spans:
        result.append(target[cursor:start])
        if reference not in removed_references:
            result.append(
                candidate_blocks[reference]
                if reference in replace_references
                else target[start:end]
            )
        cursor = end
    result.append(target[cursor:])
    output = "".join(result)
    if insert_references:
        first_footprint = output.index('(footprint "')
        insertion = "\n\n".join(
            candidate_blocks[reference] for reference in sorted(insert_references)
        )
        output = output[:first_footprint] + insertion + "\n\n" + output[first_footprint:]
    for net_name in sorted(obsolete_net_names):
        if output.count(f'"{net_name}"') == 1:
            output = re.sub(
                rf'\s*\(net\s+\d+\s+"{re.escape(net_name)}"\)',
                "",
                output,
                count=1,
            )
    SRC_PCB.write_text(output, encoding="utf-8")
    changed = requested | removed_references
    print(f"applied component footprints: {', '.join(sorted(changed))}")


if __name__ == "__main__":
    build_schematic()
    build_pcb(with_artwork="--placement-only" not in sys.argv)
    if "--apply-component-footprints" in sys.argv:
        apply_generated_component_footprints(
            {"J2", "R7", "R8", "R9", "R10", "R11", "R16", "D4", "D6", "D7"},
            removed_references={"R17", "R18"},
            obsolete_net_names={"USB_CC1", "USB_CC2"},
        )
    print(OUT)
