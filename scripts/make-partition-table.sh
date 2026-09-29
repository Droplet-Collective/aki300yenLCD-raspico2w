#!/usr/bin/env bash
# partition/pico2w-ab.json から A/B パーティションテーブルの UF2 を作る。
#
#   scripts/make-partition-table.sh [出力.uf2]   (既定: partition/pico2w-ab.uf2)
#
# 出来た UF2 は family "absolute" でフラッシュ先頭 (slot 0, 0x10000000) に
# 書かれる。初回は BOOTSEL で `picotool load -v partition/pico2w-ab.uf2`。
# picotool は PATH か環境変数 PICOTOOL で指定する。
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
json="$here/partition/pico2w-ab.json"
out="${1:-$here/partition/pico2w-ab.uf2}"
picotool="${PICOTOOL:-picotool}"

"$picotool" version >/dev/null
"$picotool" partition create "$json" "$out"
echo "wrote $out ($(stat -c %s "$out") bytes)"
