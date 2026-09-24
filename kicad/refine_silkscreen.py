"""Clean up front silkscreen warnings and add board identification text."""

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


def position(item):
    item_position = item.GetPosition()
    return [mm(item_position.x), mm(item_position.y)]


def move_reference(reference, expected, target):
    footprint = board.FindFootprintByReference(reference)
    if footprint is None:
        raise ValueError(f"Missing footprint {reference}")
    field = footprint.Reference()
    if field.GetLayer() != pcbnew.F_SilkS or not field.IsVisible():
        raise ValueError(f"Expected visible F.SilkS reference for {reference}")
    if position(field) != expected:
        raise ValueError(
            f"Expected {reference} reference at {expected}, found {position(field)}"
        )
    field.SetPosition(point(target))
    return {
        "reference": reference,
        "before_mm": expected,
        "after_mm": target,
    }


reference_moves = [
    move_reference("H1", [4.0, -0.2], [4.0, 8.0]),
    move_reference("H4", [96.0, -0.2], [96.0, 8.0]),
]

d6 = board.FindFootprintByReference("D6")
if d6 is None:
    raise ValueError("Missing footprint D6")
d6_k_matches = [
    item
    for item in d6.GraphicalItems()
    if item.Type() == pcbnew.PCB_TEXT_T
    and item.GetLayer() == pcbnew.F_SilkS
    and item.GetText() == "K"
    and position(item) == [10.85, 41.25]
]
if len(d6_k_matches) != 1:
    raise ValueError(f"Expected one D6 F.SilkS K marker, found {len(d6_k_matches)}")
d6_k_matches[0].SetPosition(point([10.85, 39.0]))

identification = [
    {
        "text": "PICO2W-LTA042B010F",
        "position_mm": [37.5, 14.0],
        "size_mm": 1.2,
        "thickness_mm": 0.18,
    },
    {
        "text": "REV A  2026-09-17",
        "position_mm": [37.5, 16.0],
        "size_mm": 1.0,
        "thickness_mm": 0.15,
    },
]

existing_board_silk_texts = {
    item.GetText()
    for item in board.GetDrawings()
    if item.Type() == pcbnew.PCB_TEXT_T
    and item.GetLayer() in (pcbnew.F_SilkS, pcbnew.B_SilkS)
}
duplicates = existing_board_silk_texts.intersection(
    entry["text"] for entry in identification
)
if duplicates:
    raise ValueError(f"Identification text already exists: {sorted(duplicates)}")

for entry in identification:
    text = pcbnew.PCB_TEXT(board)
    text.SetText(entry["text"])
    text.SetPosition(point(entry["position_mm"]))
    text.SetLayer(pcbnew.F_SilkS)
    text.SetTextSize(
        pcbnew.VECTOR2I(
            pcbnew.FromMM(entry["size_mm"]),
            pcbnew.FromMM(entry["size_mm"]),
        )
    )
    text.SetTextThickness(pcbnew.FromMM(entry["thickness_mm"]))
    text.SetHorizJustify(pcbnew.GR_TEXT_H_ALIGN_CENTER)
    text.SetVertJustify(pcbnew.GR_TEXT_V_ALIGN_CENTER)
    board.Add(text)

args.output.parent.mkdir(parents=True, exist_ok=True)
pcbnew.SaveBoard(str(args.output), board)

report = {
    "input": str(args.input),
    "output": str(args.output),
    "input_sha256": hashlib.sha256(args.input.read_bytes()).hexdigest(),
    "scope": "front-silkscreen-cleanup-and-board-identification",
    "footprint_placement_changed": False,
    "reference_moves": reference_moves,
    "d6_k_marker": {
        "before_mm": [10.85, 41.25],
        "after_mm": [10.85, 39.0],
    },
    "identification": identification,
}
if args.report:
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with args.report.open("w", encoding="utf-8") as handle:
        json.dump(report, handle, indent=2, ensure_ascii=False)
        handle.write("\n")
