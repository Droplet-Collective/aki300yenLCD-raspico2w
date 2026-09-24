"""Find a courtyard-safe red-zone placement with SciPy's MILP solver.

Geometry is exported by ``optimize_power_placement.py`` from KiCad itself.
The solver handles only axis-aligned courtyard rectangles and intentionally
uses a 0.20 mm assembly gap; electrical link quality is refined separately.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
from scipy.optimize import Bounds, LinearConstraint, milp
from scipy.sparse import lil_matrix


REGION = (0.4, 0.4, 26.6, 51.6)
CLEARANCE = 0.20
BIG_M = 80.0
LINKS = [
    # MC34063 high-di/dt current loops dominate the placement objective.
    ("U3.1", "D4.2", 28), ("D4.1", "C8.1", 24), ("C8.2", "U3.2", 24),
    ("L1.2", "U3.1", 36), ("R7.2", "U3.7", 18), ("R7.2", "L1.1", 32),
    ("C6.1", "U3.6", 12), ("C6.1", "R7.1", 14), ("C6.2", "U3.2", 12),
    ("C7.1", "U3.3", 8), ("C7.2", "U3.4", 5),
    ("R9.2", "U3.5", 10), ("R10.1", "U3.5", 10), ("R9.2", "R10.1", 6),
    ("R10.2", "U3.4", 1), ("R9.1", "C8.1", 1),
    ("R8.2", "U3.8", 3), ("R8.1", "C6.1", 1),
    ("J2.1", "C6.1", 5), ("J2.2", "C6.2", 4),
    ("J3.1", "C8.1", 1), ("J3.2", "C8.2", 1),
    ("C9.1", "U3.1", 14), ("C9.2", "D6.2", 10),
    ("C9.2", "D7.2", 10), ("D7.1", "C11.2", 10),
    ("D6.1", "C11.1", 2), ("R16.1", "D8.2", 2),
    ("R11.2", "D5.2", 2),
]
FIXED_POSITIONS = {
    "J2": (3.4, 28.0, 90),
    "J3": (2.25, 38.75, 180),
    "TP1": (9.5, 50.0, 90),
    "TP2": (12.75, 50.0, 90),
    "TP3": (16.0, 50.0, 90),
    "TP4": (19.25, 50.0, 90),
    "H1": (4.0, 4.0, 0),
    "H2": (4.0, 48.0, 0),
    "J1": (37.75, 42.65, 180),
}
LOCKED = set(FIXED_POSITIONS)
EXCLUDED: set[str] = set()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--geometry", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--time-limit", type=float, default=180.0)
    args = parser.parse_args()

    geometry = json.loads(args.geometry.read_text(encoding="utf-8"))
    movable = sorted(set(geometry) - LOCKED - EXCLUDED)

    lower: list[float] = []
    upper: list[float] = []
    integrality: list[int] = []
    objective: list[float] = []

    def variable(lb: float, ub: float, integer: bool = False, cost: float = 0.0) -> int:
        index = len(lower)
        lower.append(lb)
        upper.append(ub)
        integrality.append(1 if integer else 0)
        objective.append(cost)
        return index

    variables: dict[str, tuple[int, int, int]] = {}
    dimensions: dict[str, tuple[float, float, float, float]] = {}
    for ref in movable:
        rect0 = geometry[ref]["0"]["rect"]
        rect90 = geometry[ref]["90"]["rect"]
        width0, height0 = rect0[2] - rect0[0], rect0[3] - rect0[1]
        width90, height90 = rect90[2] - rect90[0], rect90[3] - rect90[1]
        left = variable(REGION[0], REGION[2])
        top = variable(REGION[1], REGION[3])
        rotate = variable(0, 1, integer=True, cost=0.01)
        variables[ref] = (left, top, rotate)
        dimensions[ref] = (
            width0,
            height0,
            width90 - width0,
            height90 - height0,
        )

    rows: list[tuple[dict[int, float], float, float]] = []

    def constraint(coefficients: dict[int, float], lb: float, ub: float) -> None:
        rows.append((coefficients, lb, ub))

    for ref in movable:
        left, top, rotate = variables[ref]
        width0, height0, delta_width, delta_height = dimensions[ref]
        constraint({left: 1.0}, REGION[0], np.inf)
        constraint({top: 1.0}, REGION[1], np.inf)
        constraint(
            {left: 1.0, rotate: delta_width},
            -np.inf,
            REGION[2] - width0,
        )
        constraint(
            {top: 1.0, rotate: delta_height},
            -np.inf,
            REGION[3] - height0,
        )

    def fixed_bounds(ref: str) -> tuple[float, float, float, float]:
        x, y, angle = FIXED_POSITIONS[ref]
        rect = geometry[ref][str(angle)]["rect"]
        return rect[0] + x, rect[1] + y, rect[2] + x, rect[3] + y

    fixed = {ref: fixed_bounds(ref) for ref in FIXED_POSITIONS}
    fixed["TOP_NOTCH"] = (25.0, 0.0, 26.6, 10.0)

    def non_overlap_fixed(ref: str, box: tuple[float, float, float, float]) -> None:
        left, top, rotate = variables[ref]
        width0, height0, delta_width, delta_height = dimensions[ref]
        fleft, ftop, fright, fbottom = box
        selectors = [variable(0, 1, integer=True) for _ in range(4)]
        constraint(
            {left: 1, rotate: delta_width, selectors[0]: BIG_M},
            -np.inf,
            BIG_M + fleft - width0 - CLEARANCE,
        )
        constraint(
            {left: -1, selectors[1]: BIG_M},
            -np.inf,
            BIG_M - fright - CLEARANCE,
        )
        constraint(
            {top: 1, rotate: delta_height, selectors[2]: BIG_M},
            -np.inf,
            BIG_M + ftop - height0 - CLEARANCE,
        )
        constraint(
            {top: -1, selectors[3]: BIG_M},
            -np.inf,
            BIG_M - fbottom - CLEARANCE,
        )
        constraint({selector: 1 for selector in selectors}, 1, np.inf)

    for ref in movable:
        for box in fixed.values():
            non_overlap_fixed(ref, box)

    for index, first in enumerate(movable):
        l1, t1, o1 = variables[first]
        w1, h1, dw1, dh1 = dimensions[first]
        for second in movable[index + 1 :]:
            l2, t2, o2 = variables[second]
            w2, h2, dw2, dh2 = dimensions[second]
            selectors = [variable(0, 1, integer=True) for _ in range(4)]
            constraint(
                {l1: 1, o1: dw1, l2: -1, selectors[0]: BIG_M},
                -np.inf,
                BIG_M - w1 - CLEARANCE,
            )
            constraint(
                {l2: 1, o2: dw2, l1: -1, selectors[1]: BIG_M},
                -np.inf,
                BIG_M - w2 - CLEARANCE,
            )
            constraint(
                {t1: 1, o1: dh1, t2: -1, selectors[2]: BIG_M},
                -np.inf,
                BIG_M - h1 - CLEARANCE,
            )
            constraint(
                {t2: 1, o2: dh2, t1: -1, selectors[3]: BIG_M},
                -np.inf,
                BIG_M - h2 - CLEARANCE,
            )
            constraint({selector: 1 for selector in selectors}, 1, np.inf)

    def pad_expression(pin: str, axis: int) -> tuple[dict[int, float], float]:
        ref, pad_number = pin.split(".", 1)
        if ref in variables:
            position, _, rotate = variables[ref] if axis == 0 else (
                variables[ref][1],
                variables[ref][0],
                variables[ref][2],
            )
            rect0 = geometry[ref]["0"]["rect"]
            rect90 = geometry[ref]["90"]["rect"]
            pad0 = geometry[ref]["0"]["pads"][pad_number]
            pad90 = geometry[ref]["90"]["pads"][pad_number]
            offset0 = pad0[axis] - rect0[axis]
            offset90 = pad90[axis] - rect90[axis]
            return {position: 1.0, rotate: offset90 - offset0}, offset0
        x, y, angle = FIXED_POSITIONS[ref]
        pad = geometry[ref][str(angle)]["pads"][pad_number]
        return {}, (x, y)[axis] + pad[axis]

    def add_absolute_distance(first: str, second: str, weight: float) -> None:
        for axis in (0, 1):
            first_coeffs, first_constant = pad_expression(first, axis)
            second_coeffs, second_constant = pad_expression(second, axis)
            distance = variable(0.0, BIG_M, cost=weight)
            difference: dict[int, float] = {}
            for index, value in first_coeffs.items():
                difference[index] = difference.get(index, 0.0) + value
            for index, value in second_coeffs.items():
                difference[index] = difference.get(index, 0.0) - value
            difference[distance] = -1.0
            constant = first_constant - second_constant
            constraint(difference, -np.inf, -constant)
            constraint(
                {index: -value for index, value in difference.items()
                 if index != distance} | {distance: -1.0},
                -np.inf,
                constant,
            )

    for first, second, weight in LINKS:
        add_absolute_distance(first, second, weight)

    matrix = lil_matrix((len(rows), len(lower)), dtype=float)
    row_lb = np.empty(len(rows))
    row_ub = np.empty(len(rows))
    for row_index, (coefficients, lb, ub) in enumerate(rows):
        for variable_index, value in coefficients.items():
            matrix[row_index, variable_index] = value
        row_lb[row_index] = lb
        row_ub[row_index] = ub

    result = milp(
        c=np.asarray(objective),
        integrality=np.asarray(integrality),
        bounds=Bounds(np.asarray(lower), np.asarray(upper)),
        constraints=LinearConstraint(matrix.tocsr(), row_lb, row_ub),
        options={"time_limit": args.time_limit, "mip_rel_gap": 0.0},
    )
    if result.x is None:
        raise SystemExit(f"MILP failed: status={result.status} message={result.message}")

    placement: dict[str, list[float]] = {
        ref: [x, y, angle] for ref, (x, y, angle) in FIXED_POSITIONS.items()
    }
    for ref in movable:
        left_index, top_index, rotate_index = variables[ref]
        angle = 90 if result.x[rotate_index] >= 0.5 else 0
        rect = geometry[ref][str(angle)]["rect"]
        origin_x = result.x[left_index] - rect[0]
        origin_y = result.x[top_index] - rect[1]
        placement[ref] = [round(origin_x, 4), round(origin_y, 4), angle]

    ordered = {
        ref: placement[ref]
        for ref in geometry
        if ref in placement and ref not in {"H1", "H2", "J1"}
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(ordered, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({
        "status": int(result.status),
        "message": result.message,
        "objective": float(result.fun),
        "movable_count": len(movable),
        "placement": ordered,
    }, indent=2))


if __name__ == "__main__":
    main()
