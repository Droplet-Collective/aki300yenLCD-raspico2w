"""Deterministic bounded placement search; geometry and pad pins come from KiCad.

The score is a placement heuristic, not an electrical simulation. Review the
native plots and DRC before applying a result. All distances are in mm.
"""
import argparse
import json
import math
import random
from pathlib import Path

import pcbnew
from place_power_stage import BOARD, PLACEMENT, xy

LOCKED = {'J2', 'J3', 'TP1', 'TP2', 'TP3', 'TP4'}
MOVABLE = [reference for reference in PLACEMENT if reference not in LOCKED]
FIXED = ('J1', 'H1', 'H2', *sorted(LOCKED))
REGION = (0.4, 0.4, 26.6, 51.6)
TOP_NOTCH = (25.0, 0.0, 26.6, 10.0)
CLEARANCE = 0.15
SWAP_GROUPS = [
    ('R7', 'R9', 'R10', 'R11', 'R16'),
    ('D4', 'D6', 'D7'),
    ('C8', 'C9', 'C11'),
    ('D5', 'D8'),
]
LINKS = [
    ('U3.1', 'D4.2', 28), ('D4.1', 'C8.1', 24), ('C8.2', 'U3.2', 24),
    ('L1.2', 'U3.1', 36), ('R7.2', 'U3.7', 18), ('R7.2', 'L1.1', 32),
    ('C6.1', 'U3.6', 12), ('C6.1', 'R7.1', 14), ('C6.2', 'U3.2', 12),
    ('C7.1', 'U3.3', 8), ('C7.2', 'U3.4', 5),
    ('R9.2', 'U3.5', 10), ('R10.1', 'U3.5', 10), ('R9.2', 'R10.1', 6),
    ('R10.2', 'U3.4', 1), ('R9.1', 'C8.1', 1),
    ('R8.2', 'U3.8', 3), ('R8.1', 'C6.1', 1),
    ('J2.1', 'C6.1', 5), ('J2.2', 'C6.2', 4),
    ('J3.1', 'C8.1', 1), ('J3.2', 'C8.2', 1),
    ('C9.1', 'U3.1', 14), ('C9.2', 'D6.2', 10),
    ('C9.2', 'D7.2', 10), ('D7.1', 'C11.2', 10),
    ('D6.1', 'C11.1', 2), ('R16.1', 'D8.2', 2),
    ('R11.2', 'D5.2', 2),
]
LIMITS = [
    ('U3.1', 'D4.2', 5), ('L1.2', 'U3.1', 7),
    ('R7.2', 'L1.1', 7), ('C8.2', 'U3.2', 7),
    ('C6.1', 'U3.6', 10), ('C7.1', 'U3.3', 7),
    ('R7.2', 'U3.7', 7), ('R9.2', 'U3.5', 7),
    ('R10.1', 'U3.5', 7), ('U3.1', 'C9.1', 10),
    ('C9.2', 'D6.2', 8), ('C9.2', 'D7.2', 8),
    ('D7.1', 'C11.2', 8),
]
ANGLES = (0, 90, 180, 270)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('--output', type=Path, required=True)
    ap.add_argument('--steps', type=int, default=400000)
    ap.add_argument('--seed', type=int, default=8)
    ap.add_argument('--start', type=Path)
    ap.add_argument('--temperature', type=float, default=2000000)
    ap.add_argument('--geometry-output', type=Path)
    args = ap.parse_args()
    rng = random.Random(args.seed)
    b = pcbnew.LoadBoard(str(BOARD))
    fs = {f.GetReference(): f for f in b.GetFootprints()}
    layers = pcbnew.LSET()
    layers.AddLayer(pcbnew.F_CrtYd)
    geom = {}
    for ref, f in fs.items():
        if ref not in PLACEMENT and ref not in FIXED:
            continue
        f.SetPosition(pcbnew.VECTOR2I(0, 0))
        for angle in ANGLES:
            f.SetOrientationDegrees(angle)
            bb = f.GetLayerBoundingBox(layers)
            if bb.GetWidth() <= 0:
                bb = f.GetBoundingBox(False, False)
            rect = tuple(pcbnew.ToMM(v) for v in
                         [bb.GetX(), bb.GetY(), bb.GetRight(), bb.GetBottom()])
            pads = {p.GetNumber(): xy(p.GetPosition()) for p in f.Pads()}
            geom[ref, angle] = (rect, pads)
    if args.geometry_output:
        serialized = {
            ref: {
                str(angle): {
                    'rect': geom[ref, angle][0],
                    'pads': geom[ref, angle][1],
                }
                for angle in ANGLES
            }
            for ref in {*PLACEMENT, *FIXED}
            if (ref, 0) in geom
        }
        args.geometry_output.parent.mkdir(parents=True, exist_ok=True)
        args.geometry_output.write_text(json.dumps(serialized, indent=2) + '\n')
    state = {k: tuple(v) for k, v in PLACEMENT.items()}
    state.update({'J1': (37.75, 42.65, 180), 'H1': (4, 4, 0), 'H2': (4, 48, 0)})
    if args.start:
        state.update({
            k: tuple(v)
            for k, v in json.loads(args.start.read_text()).items()
            if k in MOVABLE
        })
    refs = list(state)

    def cost(state):
        rects, pads = {}, {}
        score = 0.0
        for ref, (x, y, a) in state.items():
            r, ps = geom[ref, a]
            rects[ref] = (r[0] + x, r[1] + y, r[2] + x, r[3] + y)
            pads.update({ref + '.' + n: (p[0] + x, p[1] + y) for n, p in ps.items()})
            if ref in MOVABLE:
                l, t, rr, bb = rects[ref]
                excess = (max(REGION[0]-l, 0) + max(rr-REGION[2], 0)
                          + max(REGION[1]-t, 0) + max(bb-REGION[3], 0))
                score += 1000000000 * excess
                notch_dx = max(0, min(rr, TOP_NOTCH[2]) - max(l, TOP_NOTCH[0]))
                notch_dy = max(0, min(bb, TOP_NOTCH[3]) - max(t, TOP_NOTCH[1]))
                if notch_dx > 0 and notch_dy > 0:
                    score += 1000000000 + 100000000 * notch_dx * notch_dy
        for i, ref in enumerate(refs):
            for other in refs[i+1:]:
                if ref not in MOVABLE and other not in MOVABLE:
                    continue
                a, b = rects[ref], rects[other]
                # Courtyards carry the assembly margin; keep a small search gap.
                dx = min(a[2], b[2]) - max(a[0], b[0]) + CLEARANCE
                dy = min(a[3], b[3]) - max(a[1], b[1]) + CLEARANCE
                if dx > 0 and dy > 0:
                    score += 1000000000 + 100000000 * (min(dx, dy) + dx * dy)
        for a, b, w in LINKS:
            score += math.dist(pads[a], pads[b]) * w * 25
        for a, b, limit in LIMITS:
            score += 5000 * max(0, math.dist(pads[a], pads[b]) - limit) ** 2
        # External access constraints: the 13V header sits on any perimeter,
        # while the four probe pads form one compact row on the bottom edge.
        j3 = rects['J3']
        j3_inset = min(
            j3[0] - REGION[0], REGION[2] - j3[2],
            j3[1] - REGION[1], REGION[3] - j3[3],
        )
        score += 200000 * max(0, j3_inset - 0.2) ** 2
        tp_rects = [rects[f'TP{index}'] for index in range(1, 5)]
        tp_centers = [
            ((rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2)
            for rect in tp_rects
        ]
        score += 200000 * sum(
            max(0, REGION[3] - rect[3] - 0.2) ** 2 for rect in tp_rects
        )
        score += 100000 * (
            max(center[1] for center in tp_centers)
            - min(center[1] for center in tp_centers)
        ) ** 2
        score += 100000 * max(
            0,
            max(center[0] for center in tp_centers)
            - min(center[0] for center in tp_centers)
            - 12.25,
        ) ** 2
        # Reserve a direct high-current corridor; sensitive components must not
        # occupy the proposed switch-node segment or its 1 mm surrounding band.
        for a, b in [('L1.2', 'U3.1'), ('D4.2', 'U3.1')]:
            start, end = pads[a], pads[b]
            for sensitive in ['R9', 'R10', 'C7']:
                left, top, right, bottom = rects[sensitive]
                for j in range(1, 20):
                    px = start[0] + (end[0]-start[0])*j/20
                    py = start[1] + (end[1]-start[1])*j/20
                    if left-1 < px < right+1 and top-1 < py < bottom+1:
                        score += 20000
                        break
        # Minimize the area bounded by the boost rectifier/output pulsed-current loop.
        loop = [pads[p] for p in ['U3.1', 'D4.2', 'D4.1', 'C8.1', 'C8.2', 'U3.2']]
        area = abs(sum(loop[i][0]*loop[(i+1)%6][1] - loop[(i+1)%6][0]*loop[i][1]
                       for i in range(6))) / 2
        score += area * 20
        return score

    current = cost(state)
    best_score, best = current, state.copy()
    for step in range(args.steps):
        temperature = args.temperature * (0.0001 ** (step / args.steps))
        if rng.random() < 0.12:
            first, second = rng.sample(rng.choice(SWAP_GROUPS), 2)
            old_first, old_second = state[first], state[second]
            state[first], state[second] = old_second, old_first
            proposal = cost(state)
            if proposal <= current or rng.random() < math.exp(min(0, (current-proposal)/temperature)):
                current = proposal
                if current < best_score:
                    best_score, best = current, state.copy()
            else:
                state[first], state[second] = old_first, old_second
            continue
        ref = rng.choice(MOVABLE)
        old = state[ref]
        x, y, a = old
        if rng.random() < 0.18:
            a2 = rng.choice(ANGLES)
            r1, r2 = geom[ref, a][0], geom[ref, a2][0]
            x += (r1[0]+r1[2]-r2[0]-r2[2])/2
            y += (r1[1]+r1[3]-r2[1]-r2[3])/2
            a = a2
        else:
            delta = rng.choice([0.25, 0.5, 1, 2, 4, 8, 12, 20, 30])
            x += rng.choice([-delta, 0, delta])
            y += rng.choice([-delta, 0, delta])
        state[ref] = (round(x*4)/4, round(y*4)/4, a)
        proposal = cost(state)
        if proposal <= current or rng.random() < math.exp(min(0, (current-proposal)/temperature)):
            current = proposal
            if current < best_score:
                best_score, best = current, state.copy()
        else:
            state[ref] = old
    output = {ref: best[ref] for ref in PLACEMENT}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2) + '\n')
    print(json.dumps({'seed': args.seed, 'steps': args.steps, 'score': best_score,
                      'placement': {ref: best[ref] for ref in MOVABLE}}, indent=2))


if __name__ == '__main__':
    main()
