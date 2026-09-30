#!/usr/bin/env python3
"""ticker などの ELF の最悪スタック深さを逆アセンブルから見積もる (0.4.1〜、docs/ticker.md「スタック」)

    scripts/stack-report.py target/thumbv8m.main-none-eabihf/release/ticker

各関数のプロローグ (push / vpush / sub sp) からフレームの大きさを求め、`bl` / `b.w` の直接呼び出しで
呼び出しグラフを作り、根 (各タスクの poll、割り込み) からの最深経路を出す。間接呼び出し (関数ポインタ、
dyn) は追えないので下限の見積もりだが、async の poll はほぼ全て直接呼び出しになる。

TLS の証明書検証 (`TlsVerify::Certificate` 用の reqwless `Provider`、ed25519 / RSA / P-384 の検証) は
ticker が使う `TlsVerify::None` では呼ばれないので、既定では経路から除く (`--all` で含める)。

空きスタック = `_stack_start` - `__euninit` (cortex-m-rt: .bss / .uninit の上から RAM の最上位まで)。
"""
import argparse
import collections
import os
import re
import shutil
import subprocess
import sys

UNREACHABLE_WITH_TLS_VERIFY_NONE = re.compile(
    r"8reqwless6client8Provider|client_cert_verify|ed25519|der_certificate|rsa|num_bigint|p384|NistP384|ecdsa"
)

ROOTS = [
    ("main task", r"embassy_main_task_inner_function0B5_$"),
    ("jobs_task", r"jobs_task_task.*inner_function0E4poll"),
    ("render_task", r"render_task_task.*inner_function0E4poll"),
    ("slideshow_task", r"slideshow_task_task.*inner_function0E4poll"),
    ("cyw43_task", r"cyw43_task_task.*inner_function0E4poll"),
    ("net_task", r"net_task_task.*inner_function0E4poll"),
    ("usb_task", r"usb_task_task.*inner_function0E4poll"),
]
IRQS = ["DMA_IRQ_0", "DMA_IRQ_1", "PIO0_IRQ_0", "PIO1_IRQ_0", "USBCTRL_IRQ", "TIMER0_IRQ_0", "HardFault"]
# 例外の積み上げ (FPU 使用時の拡張フレーム 26 語 + 余裕)
EXCEPTION_FRAME = 112


def tool(name):
    for cand in (f"llvm-{name}", name):
        path = shutil.which(cand)
        if path:
            return path
    sysroot = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True).stdout.strip()
    for root, _, files in os.walk(os.path.join(sysroot, "lib", "rustlib")):
        if f"llvm-{name}" in files:
            return os.path.join(root, f"llvm-{name}")
    sys.exit(f"llvm-{name} not found (rustup component add llvm-tools)")


def regcount(s):
    n = 0
    for part in s.strip("{} ").split(","):
        part = part.strip()
        if "-" in part:
            a, b = part.split("-")
            n += int(re.sub(r"\D", "", b)) - int(re.sub(r"\D", "", a)) + 1
        elif part:
            n += 1
    return n


def imm(s):
    m = re.search(r"#(0x[0-9a-f]+|\d+)", s)
    return int(m.group(1), 0) if m else 0


def parse(elf):
    out = subprocess.run([tool("objdump"), "-d", "--no-show-raw-insn", elf], capture_output=True, text=True, check=True).stdout
    func_re = re.compile(r"^([0-9a-f]+) <(.*)>:$")
    ins_re = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$")
    funcs = collections.OrderedDict()
    cur = None
    for line in out.splitlines():
        m = func_re.match(line)
        if m:
            cur = m.group(2)
            funcs[cur] = {"addr": int(m.group(1), 16), "ins": []}
            continue
        m = ins_re.match(line)
        if m and cur:
            funcs[cur]["ins"].append((m.group(2), m.group(3)))
    addr2f = {v["addr"]: k for k, v in funcs.items()}
    frame, calls = {}, {}
    for name, f in funcs.items():
        size, movs, callees = 0, {}, set()
        for i, (op, args) in enumerate(f["ins"]):
            if i < 40:
                parts = [p.strip() for p in args.split(",")]
                if op in ("push", "push.w"):
                    size += 4 * regcount(args)
                elif op == "stmdb" and args.startswith("sp!"):
                    size += 4 * regcount(args.split(",", 1)[1])
                elif op == "vpush":
                    size += 8 * regcount(args)
                elif op.startswith("str") and args.startswith("lr, [sp, #-"):
                    size += imm(args)
                elif op in ("sub", "sub.w", "subw") and args.startswith("sp, "):
                    if "#" in args:
                        size += imm(args)
                    elif parts[-1] in movs:
                        size += movs[parts[-1]]
                elif op in ("movw", "mov.w", "movs", "mov") and "#" in args:
                    movs[parts[0]] = imm(args)
                elif op == "movt":
                    movs[parts[0]] = movs.get(parts[0], 0) | (imm(args) << 16)
            if op in ("bl", "blx", "b.w", "b") and "<" in args:
                m = re.search(r"([0-9a-f]+) <(.*)>", args)
                if m and int(m.group(1), 16) in addr2f and addr2f[int(m.group(1), 16)] != name:
                    callees.add(addr2f[int(m.group(1), 16)])
        frame[name], calls[name] = size, callees
    return funcs, frame, calls


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("elf")
    ap.add_argument("--all", action="store_true", help="TlsVerify::None で呼ばれない証明書検証も含める")
    ap.add_argument("--top", type=int, default=0, help="大きなフレームの関数を N 個出す")
    ap.add_argument("--path", type=int, default=8, help="各根の最深経路を何段まで出す")
    args = ap.parse_args()
    funcs, frame, calls = parse(args.elf)
    sys.setrecursionlimit(100000)
    memo, onstack = {}, set()

    def worst(n):
        if n in memo:
            return memo[n]
        if n in onstack:
            return (0, [])
        onstack.add(n)
        best = (0, [])
        for t in calls.get(n, ()):
            if not args.all and UNREACHABLE_WITH_TLS_VERIFY_NONE.search(t):
                continue
            w = worst(t)
            if w[0] > best[0]:
                best = w
        onstack.discard(n)
        memo[n] = (frame[n] + best[0], [n] + best[1])
        return memo[n]

    nm = subprocess.run([tool("nm"), args.elf], capture_output=True, text=True, check=True).stdout
    syms = {l.split()[-1]: int(l.split()[0], 16) for l in nm.splitlines() if len(l.split()) == 3}
    free = syms.get("_stack_start", 0) - syms.get("__euninit", 0)

    if args.top:
        for n, s in sorted(frame.items(), key=lambda x: -x[1])[: args.top]:
            print(f"{s:7d}  {n[:160]}")
        print()

    irq_worst = 0
    for irq in IRQS:
        if irq in funcs:
            irq_worst = max(irq_worst, worst(irq)[0])
    executor = 0
    for n in funcs:
        if re.search(r"embassy_executor.*SyncExecutor.*poll|embassy_executor4arch6thread.*Executor3run", n):
            executor = max(executor, frame[n])
    overhead = executor + 256 + irq_worst + EXCEPTION_FRAME
    print(f"free stack (_stack_start - __euninit): {free} B ({free / 1024:.1f} KiB)")
    print(f"executor + cortex-m-rt + worst IRQ ({irq_worst} B) + exception frame: ~{overhead} B")
    deepest = 0
    for label, pat in ROOTS:
        for n in funcs:
            if re.search(pat, n):
                total, path = worst(n)
                deepest = max(deepest, total)
                print(f"\n{label}: {total} B (+ overhead = {total + overhead} B, margin {free - total - overhead} B)")
                for p in path[: args.path]:
                    print(f"  {frame[p]:6d}  {p[:150]}")
                break
    print(f"\nworst task path + overhead: {deepest + overhead} B, free {free} B, margin {free - deepest - overhead} B")
    return 0 if deepest + overhead < free else 1


if __name__ == "__main__":
    sys.exit(main())
