# CI ビルド（GitHub Actions）と UF2 の入手方法

`.github/workflows/build.yml` が、push ごとに全 `[[bin]]` を
`thumbv8m.main-none-eabihf` 向けに `cargo build --release` し、
picotool で ELF → UF2（`--family rp2350-arm-s`）に変換します。

## UF2 の入手先

- **任意の push / ブランチ**: Actions タブ → 該当の `build` 実行 → Artifacts の
  `firmware-<commit SHA>` をダウンロード（zip 内に `<bin名>.elf` / `<bin名>.uf2` / `commit.txt`）。
  保持期間は 14 日です。
- **リリース**: `v*` 形式のタグ（例 `v0.1.0`）を push すると、同名の GitHub Release が
  自動作成され、全 bin の UF2 が添付されます。こちらは無期限に残ります。
- **手動実行**: Actions → `build` → Run workflow。`bin` 入力に bin 名を入れると
  その 1 つだけをビルドします（既定は `sd_bmp_viewer`）。

## 書き込み方法

- **picotool（ELF / UF2 どちらでも可）**: BOOTSEL を押しながら USB 接続し、
  `picotool load -f -v -x -t elf sd_bmp_viewer.elf`（UF2 なら `-t uf2`）。
  `-f` で実行中の基板を強制的に BOOTSEL へ、`-v` で検証、`-x` で書き込み後に再起動します。
- **ドラッグ＆ドロップ**: BOOTSEL で接続すると現れる `RP2350` ドライブに `.uf2` をコピーするだけで
  書き込み・再起動されます。picotool のインストールは不要です。
