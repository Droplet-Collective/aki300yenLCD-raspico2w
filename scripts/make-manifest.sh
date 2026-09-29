#!/usr/bin/env bash
# OTA 用 manifest.json を作る。
#
#   scripts/make-manifest.sh <bin> <version> [出力ファイル]   (既定: <bin と同じディレクトリ>/manifest.json)
#
#   <bin>      make-ota-image.sh が作った OTA 用フラッシュイメージ (例 out/ticker.bin、--features tbyb でビルド)
#   <version>  semver "X.Y.Z" (先頭の v は取り除く)。Cargo.toml の version / git タグと一致させる
#
# 出力例:
#   {"version":"0.3.0","bin":"ticker.bin","size":1058560,"sha256":"<64 hex>"}
#
# ファームウェア (src/ota/manifest.rs) はこの 4 項目だけを読む。"bin" は Release アセット名
# (ディレクトリ無し) で、同じ Release に添付されていなければならない。実機は "bin" の名前をそのまま
# 取りに行くので、v0.2.8 (wifi_ota) までの機体も v0.3.0 からは ticker.bin を受け取って切り替わる。
set -euo pipefail

bin="${1:?usage: $0 <bin> <version> [out.json]}"
version="${2:?usage: $0 <bin> <version> [out.json]}"
version="${version#v}"
out="${3:-$(dirname "$bin")/manifest.json}"

if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "error: version must be X.Y.Z (got '$version')" >&2
    exit 1
fi
[ -f "$bin" ] || { echo "error: $bin not found" >&2; exit 1; }

name="$(basename "$bin")"
size="$(stat -c %s "$bin")"
sha256="$(sha256sum "$bin" | cut -d' ' -f1)"
if [ "$size" -eq 0 ]; then
    echo "error: $bin is empty" >&2
    exit 1
fi

printf '{"version":"%s","bin":"%s","size":%s,"sha256":"%s"}\n' "$version" "$name" "$size" "$sha256" > "$out"
cat "$out"
