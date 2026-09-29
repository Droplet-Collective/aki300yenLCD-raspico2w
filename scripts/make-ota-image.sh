#!/usr/bin/env bash
# ELF から OTA 配布用 .bin と、picotool / ドラッグ&ドロップ用 UF2 を作る。
#
#   scripts/make-ota-image.sh <elf> [出力ディレクトリ]   (既定: out/)
#
# 生成物 (<name> は ELF のファイル名):
#   <name>.bin   フラッシュ生イメージ (0x10000000 起点、リンク時のまま)。
#                A/B どちらのスロットに書いても bootrom の QMI アドレス変換
#                (データシート 5.1.19) により 0x10000000 に見えるので、
#                スロットごとにビルドし直す必要はない。stage 2 の OTA が
#                空きスロットへそのまま書き込む。
#   <name>.uf2   family rp2350-arm-s。アドレスは実行時アドレスのまま。
#                パーティションテーブルがある機体では bootrom / picotool が
#                「今起動していない方」の A/B パーティションへ配置する
#                (5.1.18)。明示するなら `picotool load -p <n>`。
#                先頭に RP2350-E10 対策の「絶対ブロック」(family absolute、
#                block 0/2、拡張 UF2_EXTENSION_RP2_IGNORE_BLOCK) を 1 個付ける
#                (`picotool uf2 convert --abs-block`)。テーブルがある RP2350 A2 に
#                BOOTSEL ドライブへ D&D した UF2 は、これが無いとフラッシュ初期化
#                前にテーブルを読みに行って失敗し、ドライブが閉じず何も書かれない
#                (データシート Errata RP2350-E10)。ブロックの宛先は
#                UF2_ABS_BLOCK (既定 0x103FFF00 = Pico 2 W の 4 MB フラッシュ最終
#                ページ。picotool 既定の 0x10FFFF00 は 16 MB 用)。パーティション
#                外である必要があり、pico2w-ab.json は 0x3FD000 以降を空けている。
#                A3 以降の bootrom はこのブロックを無視する。picotool load
#                (PICOBOOT 経由) はこの問題に無関係で、ブロックは無害。
#   <name>.sha256  .bin の SHA-256 (manifest / 書き込み後検証用)
#
# picotool は PATH か PICOTOOL で、objcopy は rustup の llvm-tools
# (llvm-objcopy) を自動検出する。
set -euo pipefail

elf="${1:?usage: $0 <elf> [outdir]}"
outdir="${2:-out}"
picotool="${PICOTOOL:-picotool}"
abs_block="${UF2_ABS_BLOCK:-0x103FFF00}"
name="$(basename "$elf")"
name="${name%.elf}"

find_objcopy() {
    if [ -n "${OBJCOPY:-}" ]; then echo "$OBJCOPY"; return; fi
    local sysroot
    sysroot="$(rustc --print sysroot 2>/dev/null || true)"
    if [ -n "$sysroot" ]; then
        local found
        found="$(find "$sysroot/lib/rustlib" -name llvm-objcopy -type f 2>/dev/null | head -n 1)"
        if [ -n "$found" ]; then echo "$found"; return; fi
    fi
    for c in rust-objcopy llvm-objcopy arm-none-eabi-objcopy; do
        if command -v "$c" >/dev/null 2>&1; then echo "$c"; return; fi
    done
    echo "error: objcopy not found (rustup component add llvm-tools)" >&2
    exit 1
}

objcopy="$(find_objcopy)"
mkdir -p "$outdir"

"$objcopy" -O binary "$elf" "$outdir/$name.bin"
"$picotool" uf2 convert --quiet -t elf "$elf" "$outdir/$name.uf2" --family rp2350-arm-s \
    --abs-block "$abs_block"
(cd "$outdir" && sha256sum "$name.bin" > "$name.sha256")

printf '%-28s %10s bytes\n' "$outdir/$name.bin" "$(stat -c %s "$outdir/$name.bin")"
printf '%-28s %10s bytes\n' "$outdir/$name.uf2" "$(stat -c %s "$outdir/$name.uf2")"
cat "$outdir/$name.sha256"
"$picotool" info -t elf "$elf" | sed -n '/Program Information/,/^$/p' | grep -E 'name|version|image type' || true
