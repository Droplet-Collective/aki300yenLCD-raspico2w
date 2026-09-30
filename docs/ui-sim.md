# 画面シミュレータ `tools/ui-sim` (v0.4.0〜)

`ticker` の画面 (写真の背景 + 時計 / 天気 / 流れる文字 / 状態) を、**書き込む前に PC で確かめる**ための道具です。
PNG (1 倍と 3 倍) と、流れる文字とスライドの切り替えを動かした GIF を作ります。

## しくみ: ファームウェアと同じコードで描く

```text
src/ui/        画面の描画 (no_std、embassy / rp に依存しない)  ─┬─ ファームウェア (ticker) が BackBuffer に描く
src/font/      東雲フォント                                     └─ tools/ui-sim が #[path] で取り込んで Vec<u16> に描く
```

- 描画コード (`src/ui/`: 色、ガラス板、文字、アイコン、BMP の読み込み、画面構成) はハードウェアに依存しない
  純粋なコードです。ファームウェアとシミュレータは**同じソースファイル**をコンパイルするので、プレビューと
  実機の画面がずれることはありません (Python などで描き直した写しではありません)。
- 画面は 400×96 の RGB565。シミュレータは LCD が受け取る値 (RGB565 → RGB666、`ui::color::to_666`) を
  8 bit に引き延ばして PNG にします。LCD 自体の発色・にじみ (隣の画素へのにじみで 2 px の線が太く見える等) は
  再現しません。
- 背景の BMP も、ファームウェアと同じ `ui::bmp::Resampler` で (512 バイトずつ) 読みます。
- 本体クレート (thumbv8m) のビルドとは独立したホスト用クレートです。`cargo build --release` (ファームウェア) には
  影響しません。

## 使い方

Rust (stable) があれば動きます (Windows / macOS / Linux)。

```sh
cd tools/ui-sim
cargo run --release -- --scenario scenarios/default.json --out out/
```

出力 (`--name` を省くとシナリオのファイル名):

| ファイル | 内容 |
|---|---|
| `out/default.png` | 400×96 (実機と同じ画素数) |
| `out/default@3x.png` | 3 倍 (最近傍で拡大。画素の形がそのまま見える) |
| `out/default.gif` | 流れる文字 + スライドの切り替え (暗くする → 読み込み → 明るく戻す)。既定 2 倍、25 fps |

ほかの使い方:

```sh
# 自分の写真を背景に (2 枚目を付けると GIF で切り替えも見られる)
cargo run --release -- --scenario scenarios/default.json --bg ../../IMAGE.BMP --bg ../../IMAGE2.BMP --out out/ --name mine
# レイアウトを変える (glass / dock / classic)
cargo run --release -- --scenario scenarios/default.json --layout dock --out out/ --name dock
# レイアウト 3 種 × 背景の比較表 (1 枚の PNG、2 倍)。--bg gradient は写真が無いときの既定の背景
cargo run --release -- sheet --out out/ --bg samples/SUNSET.BMP --bg samples/CLOUDS.BMP --bg gradient
# 見本の背景 BMP を作り直す
cargo run --release -- samples
# 描画部品のテスト (BMP の読み込み、合成、フォント表、全レイアウト × 全状態の描画)
cargo test --release
```

## シナリオ (JSON)

`scenarios/` に例があります。省略した項目は既定値 (`src/scenario.rs`)。

| シナリオ | 内容 |
|---|---|
| `default.json` | 平常時 (状態は小さな 1 行)。GIF は SUNSET → BOKEH の切り替え |
| `boot.json` | 起動直後: 時刻・天気が未取得、写真なし (既定のグラデーション)、状態 3 行 |
| `ota.json` | OTA のダウンロード中 (状態 3 行 + 進捗バー)、雪、氷点下 |
| `error.json` | 天気の取得失敗 (`WX` が赤)、雷雨 |
| `lastreset.json` | 0.4.1〜: 異常終了からの再起動直後 (状態行 1 に赤の `last reset: STACK OVERFLOW ...`、状態行 2 にスタックの最大使用量 `stk`、最初の OTA 確認中) |

```jsonc
{
  "layout": "glass",                         // glass / dock / classic (ticker.txt の layout=)
  "background": "../samples/SUNSET.BMP",     // シナリオのファイルからの相対パス。null で既定のグラデーション
  "next_background": "../samples/BOKEH.BMP", // GIF で切り替える先 (null なら切り替えなし)
  "bg_level": 32,                            // 背景の明るさ 0..32
  "clock": { "year": 2026, "month": 9, "day": 29, "hour": 21, "minute": 53, "second": 44 },  // null = 時刻同期中
  "place": "東京",
  "weather": { "temperature": 19.1, "code": 2, "max": 21.9, "min": 18.6, "rain_pct": 40 },  // null = 天気取得中
  "message": "流れる文字",
  "scroll_x": 0,                             // PNG での流れる文字の位置 (帯の左端からの px)
  "status": {
    "expanded": false,                       // true で状態 3 行 (docs/ticker.md「状態表示」の規則は実機側)
    "line1": "...", "line1_tone": "ok",      // tone: muted / normal / ok / busy / error
    "ident_head": "ticker v0.4.0 via OTA", "ident_rest": " slot B TBYB:bought OK", "ident_tone": "ok",
    "ota": "OTA: ...", "ota_tone": "ok", "progress": [612, 1196],
    "wifi": "ok", "ntp": "ok", "wx": "ok", "msg": "ok",   // 小さな 1 行の色
    "version": "v0.4.0"
  },
  "animation": {
    "duration_ms": 4400, "frame_ms": 40,     // GIF の長さと 1 コマ
    "scroll_px": 1, "lcd_hz": 60,            // 流れる文字の速さ (実機と同じく LCD のフレームで進める)
    "transition_at_ms": 800,                 // 切り替えの開始
    "load_ms": 900,                          // 次の写真の読み込み時間 (SD の速さの見積り)
    "scale": 2
  }
}
```

天気の日本語はファームウェアと同じ表 (`src/ticker/weather.rs`) から、アイコンは天気コードと時刻 (18〜6 時は月) から決まります。

## 見本の背景 (`samples/`)

写真ではなく手続き生成した画像なので、著作権の心配はありません (`cargo run --release -- samples` で作り直せます)。
BMP の読み込みの各経路を通るよう、大きさと形式を変えてあります。そのまま SD にコピーしても使えます。

| ファイル | 大きさ | 形式 | 読み込みの経路 |
|---|---|---|---|
| `SUNSET.BMP` | 400×96 | 24 bit、下から上 | そのまま |
| `CLOUDS.BMP` | 301×100 | 24 bit、上から下、行に詰め物 | 拡大 (最近傍) + 上下を切る |
| `BOKEH.BMP` | 640×200 | 32 bit、下から上 | 縮小 (面積平均) + 左右を切る |

## 時計・気温の数字フォント

時計 / 気温の数字は DejaVu Sans (Bitstream Vera 由来の自由なフォント、[fonts/dejavu/LICENSE](../fonts/dejavu/LICENSE)) を
4 bit のアンチエイリアスでラスタライズした表 (`src/ui/aafont_data.rs`、約 5.6 kB) です。作り直すときは
Python + Pillow で `python3 tools/ui-sim/gen_aafont.py [DejaVuSans.ttf]` (大きさ・字種はスクリプトの `FONTS`)。

## CI

`build.yml` の `ui-preview` ジョブが `cargo test` とシナリオ全部 + 比較表を描き、Actions の成果物
`ui-preview-<sha>` に置きます (参考用。失敗してもファームウェアのビルドや Release は止めません)。
