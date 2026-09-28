# 実装計画: Pico 2W + PIO で LTA042B010F を駆動（Rust）

> この文書は初期の実装計画を記録したものです。現在の Layer 7 は
> [単一バッファ正本と保存サンプル](layer7-fullframe-dma.md)を参照してください。
> 以下の未完了チェック項目や擬似コードは、現行実装の状態を示しません。

## 設計方針

### レイヤー分離によるデバッグ容易性

低レイヤーから段階的に実装・検証を行い、問題発生時に原因特定を容易にする。
各レイヤーには独立した検証用バイナリ（example）を用意し、
**ハードウェアが手元に届いた時点で Layer 0 から順に動作確認**できるようにする。

```
Layer 7: 全フレーム DMA 転送（単一バッファ正本）       ✅
Layer 6: アプリケーション（embedded-graphics DrawTarget）✅
Layer 5: フレームバッファ管理 + ダブルバッファリング  ✅
Layer 4: DMA 転送（フレームバッファ → PIO FIFO）     ✅
Layer 3: PIO 固定色出力 + DMA                       ✅
Layer 2: PIO タイミング制御（HSYNC/VSYNC + ブランキング） ✅
Layer 1: PIO ピクセル出力（RGB + NCLK）              ✅
Layer 0: GPIO 基本動作確認（トグルテスト）             ✅
```

> **Layer 0–7 実装済み** — 現行の単一バッファ正本は 60 Hz 走査と小矩形の往復表示を実機確認済み。

### フレームワーク選択: Embassy

| 選択理由 | 詳細 |
|---------|------|
| async/await | DMA 転送の非同期管理が自然に書ける |
| PIO + DMA 統合 | `tx.dma_push().await` で簡潔に連携 |
| マルチタスク | LCD描画 + Wi-Fi 通信を async で並行実行 |
| defmt 標準対応 | RTT 経由のログ出力でデバッグが容易 |
| RP2350 対応済み | embassy-rp v0.9.0 で RP2350 フルサポート |
| 活発な開発 | コミュニティが活発、サンプル豊富 |

### プロジェクト構成

```
raspberrypi+300yenLCD/
├── .cargo/
│   └── config.toml          # probe-rs runner 設定
├── src/
│   ├── main.rs              # エントリーポイント
│   ├── lcd/
│   │   ├── mod.rs           # LCD ドライバモジュール
│   │   ├── pio_program.rs   # PIO プログラム定義
│   │   ├── timing.rs        # タイミングパラメータ
│   │   └── framebuffer.rs   # フレームバッファ管理 (512×96 パディング方式)
│   ├── gfx/
│   │   ├── mod.rs           # 描画ライブラリ
│   │   ├── primitives.rs    # 基本図形描画
│   │   └── font.rs          # フォント描画
│   └── pins.rs              # ピン定義・型エイリアス
├── examples/
│   ├── layer0_gpio_test.rs      # GPIO トグルテスト
│   ├── layer1_pio_clock.rs      # PIO NCLK 生成テスト
│   ├── layer2_pio_timing.rs     # HSYNC/VSYNC タイミングテスト
│   ├── layer3_pio_pattern.rs    # 固定パターン出力テスト
│   ├── layer4_dma_scanline.rs   # DMA スキャンライン転送テスト
│   ├── layer5_framebuffer.rs    # フレームバッファ + ダブルバッファリング
│   └── layer6_drawing.rs        # 描画ライブラリテスト
├── docs/                        # ドキュメント（既存）
├── Cargo.toml
├── build.rs
├── memory.x                     # リンカスクリプト
└── rust-toolchain.toml
```

## Layer 0: GPIO 基本動作確認

### 目的
- Pico 2W の基本動作確認
- LCD 接続ピン（GP2-GP22）のトグルテスト
- defmt + RTT ログ出力の確認
- オシロスコープ / ロジックアナライザでの信号確認

### 検証内容
```rust
// examples/layer0_gpio_test.rs
// GP2-GP22 を出力モードに設定し、全ピンを HIGH/LOW トグル
// → オシロスコープで各ピンの出力を確認
// → FPC DIP化基板経由でLCD側ピンの導通確認
```

### 合格基準
- [ ] defmt ログが RTT で表示される
- [ ] GP2-GP22 の全ピンでトグル信号がオシロスコープで確認できる
- [ ] FPC 経由で LCD の対応ピンに信号が到達している

### 必要な機材
- オシロスコープ or ロジックアナライザ
- デバッグプローブ（Pico Debug Probe or 別の Pico）

---

## Layer 1: PIO ピクセルクロック生成

### 目的
- PIO を使って NCLK（GP20）に正確なクロック信号を生成
- 目標周波数: 3.44 MHz（60fps時）
- sideset 機能の動作確認

### PIO プログラム

```
; Layer 1: 純粋なクロック生成テスト
.program nclk_test
.side_set 1
.wrap_target
    nop side 1      ; NCLK = HIGH
    nop side 0      ; NCLK = LOW
.wrap
```

### 検証内容
```rust
// examples/layer1_pio_clock.rs
// PIO0 SM0 で NCLK クロックを GP20 に出力
// クロック分周: 150MHz / (3.44MHz × 2) = 21.8 → 分周比 22
// → 実際の周波数: 150MHz / (22 × 2) = 3.409 MHz
```

### 合格基準
- [ ] GP20 に 3.4MHz のクロック信号がオシロスコープで確認できる
- [ ] デューティ比が約50%
- [ ] ジッターが十分に小さい

---

## Layer 2: PIO タイミング制御

### 目的
- HSYNC（GP21）と VSYNC（GP22）の正確なタイミング生成
- Layer 1 の NCLK と同期したタイミング信号

### タイミングパラメータ

```rust
// src/lcd/timing.rs
pub struct LcdTiming {
    pub h_active: u32,      // 400 clk
    pub h_back_porch: u32,  // 107 clk
    pub h_front_porch: u32, // 5 clk (最小3)
    pub h_total: u32,       // 512 clk

    pub v_active: u32,      // 96 lines
    pub v_back_porch: u32,  // 16 lines
    pub v_front_porch: u32, // 0 lines (最小0)
    pub v_total: u32,       // 112 lines

    pub vsync_sample_offset: u32,  // 98 clk (HSYNC開始から)
}
```

### PIO プログラム設計

2つのステートマシンを使用:

**SM0: データ + NCLK 出力**（Layer 3 以降で使用）
```
.program pixel_out
.side_set 1
.wrap_target
    out pins, 18  side 0    ; RGB出力 + NCLK LOW → LCD がサンプル
    nop           side 1    ; NCLK HIGH（データ安定待ち）
.wrap
```

**SM1: HSYNC/VSYNC タイミング**
```
.program hsync_vsync
; X = H_TOTAL カウンタ
; Y = V_TOTAL カウンタ
; OSR からタイミングパラメータを受け取る
; SET ピンで HSYNC/VSYNC を制御
```

> **注意**: VSYNC は HSYNC 開始から 98 クロック後にサンプリングされるため、
> タイミング SM でこの制約を正確に実装する必要がある。

### 検証内容
```rust
// examples/layer2_pio_timing.rs
// PIO0 SM0: NCLK を GP20 に出力
// PIO0 SM1: HSYNC を GP21, VSYNC を GP22 に出力
// ロジアナで HSYNC/VSYNC のタイミングを確認
```

### 合格基準
- [ ] HSYNC 周期が H_TOTAL × NCLK周期 と一致
- [ ] VSYNC 周期が V_TOTAL × H周期 と一致
- [ ] HSYNC の H→L 遷移から 107 クロック後に表示期間が開始
- [ ] 各タイミング信号が NCLK と同期している
- [ ] LCD のテストパターン機能（Pin 1 = H）で表示が確認できる（電源接続済みの場合）

---

## Layer 3: PIO 固定パターン出力

### 目的
- RGB データピン（GP2-GP19）からの固定色出力
- PIO SM0（データ出力）と SM1（タイミング）の連携
- **LCD 画面に単色が表示されることを確認**

### 検証内容
```rust
// examples/layer3_pio_pattern.rs
// PIO SM0: 固定値（例: 全白 0x3FFFF）を RGB ピンに出力
// PIO SM1: HSYNC/VSYNC タイミング生成
// SM間は IRQ で同期
// → LCD に単色画面が表示される
```

### PIO プログラム（SM0: ブランキング付きデータ出力）

```
.program pixel_with_blanking
.side_set 1

; 1ライン = h_back_porch + h_active + h_front_porch

; SM1 から IRQ で水平開始を通知される
wait 1 irq 0           side 1    ; SM1 からの HSYNC 開始信号を待つ

; バックポーチ期間（107 clk）: データ出力なし（黒 or ドントケア）
set x, 106             side 1
back_porch:
    nop                side 0    ; NCLK LOW
    jmp x--, back_porch side 1   ; NCLK HIGH

; アクティブ期間（400 clk）: ピクセルデータ出力
set x, 399             side 1    ; ※ set の即値は 0-31 なので
                                  ; 実際はOSRからロードが必要
active:
    out pins, 18       side 0    ; RGB 出力 + NCLK LOW
    jmp x--, active    side 1    ; NCLK HIGH

; フロントポーチ期間: 残りクロック消費
```

> **set 命令の即値制限**: set 命令は即値 0-31 しか指定できない。
> 107 や 400 のカウントには `pull` + `mov x, osr` を使用するか、
> ネストループを使用する。

### 合格基準
- [ ] LCD に単色画面が表示される（全白、全赤、等）
- [ ] 色を変更できる（defmt ログで確認しつつ）
- [ ] 水平方向のずれ（表示位置のオフセット）がない
- [ ] フリッカーが最小限

---

## Layer 4: DMA スキャンライン転送

### 目的
- フレームバッファから DMA で PIO TX FIFO にデータ転送
- CPU 負荷ゼロでの連続ピクセル出力
- autopull との連携確認

### DMA 設計

```rust
// src/lcd/dma.rs

// フレームバッファ: 400 × 96 ピクセル
// 1ピクセル = 18bit → 32bit ワードに1ピクセル格納（14bit 余り）
// または、32bit に複数ピクセルをパック

// 案1: 1ピクセル/ワード（シンプル、メモリ効率×）
//   400 × 96 × 4 bytes = 153,600 bytes (150 KB)

// 案2: DMAパッキング（メモリ効率○、PIOプログラム複雑）
//   32bit に 1.7 ピクセル → 不整合、扱いにくい

// 推奨: 案1（1ピクセル/32bitワード）で開始、
//        最適化は Layer 6 以降で検討
```

### 検証内容
```rust
// examples/layer4_dma_scanline.rs
// SRAM にテストパターン（カラーバー等）を生成
// DMA で PIO TX FIFO に自動転送
// → LCD にカラーバーが表示される
```

### 合格基準
- [ ] DMA 転送中に CPU が自由に使える（defmt ログ出力等）
- [ ] カラーバーや縞模様が正しく表示される
- [ ] 連続フレーム表示でちらつきがない

---

## Layer 5: フレームバッファ管理 ✅

### 目的
- ダブルバッファリングの実装
- フレームバッファへの書き込みと表示の分離
- フレーム同期（VSYNC タイミングでバッファスワップ）

### 実装結果

**コミット**: `01adc8e`

フレームバッファは **512×96 パディング方式** を採用した。
当初の計画（400×96 = 153KB）から変更し、各ラインに H_BACK_PORCH と
H_FRONT_PORCH を含めることで DMA 転送時のブランキング挿入を不要にした:

```rust
// src/lcd/framebuffer.rs

pub const LINE_WIDTH: usize = 512;  // H_TOTAL
pub const ACTIVE_HEIGHT: usize = 96;
pub const FB_SIZE: usize = LINE_WIDTH * ACTIVE_HEIGHT; // 49,152 words

// 各ライン: [107 BLACK | 400 active pixels | 5 BLACK]
pub struct FrameBuffer {
    pub data: [u32; FB_SIZE],  // 192 KB
}
```

ダブルバッファリングは `Channel<&'static mut FrameBuffer, 1>` による
所有権移動方式を採用した。当初計画の `DoubleBuffer` 構造体 +
インデックス方式から変更し、Rust の所有権システムでデータ競合を型レベルで防止する:

```text
main (描画タスク)                display_task (DMA転送)
┌──────────────────────┐        ┌──────────────────────┐
│ back に描画            │        │ front を DMA 転送      │
│ SWAP_CH.send(back)    │───────▶│ try_receive()         │
│ back = RETURN_CH      │◀───────│ RETURN_CH.send(front) │
│       .receive()      │        │ front = new_front     │
└──────────────────────┘        └──────────────────────┘
```

- `display_task`: VSYNC 境界（V_BACK_PORCH 終了後）で `try_receive()` 非ブロッキングチェック
- V_BACK_PORCH 期間 (line 0-14): `BLACK_LINE` を転送
- アクティブ期間 (line 15-110): `fb.row_slice(y)` を DMA 転送
- `DisplayPeripherals` 構造体で embassy の TaskFn 16 引数制限を回避

**ヘルパー関数**:
- `rgb666(r, g, b)`: 6bit チャンネル → ビット反転済みピクセルワード
- `reverse6(v)`: PIO OUT right-shift に合わせた 6bit ビット反転
- `set_pixel(x, y, color)` / `get_pixel(x, y)`: パディング考慮済みアクセス
- `row_slice(y)`: DMA 転送用のライン全体スライス (512 words)
- `clear(color)`: アクティブ領域のみクリア

### メモリ使用量

```
フレームバッファ: 512 × 96 × 4 = 196,608 bytes (192 KB)
ダブルバッファ:   192 KB × 2 = 384 KB
RP2350 SRAM:     520 KB
使用率:          384 / 520 = 74%
残り:            136 KB → アプリケーション + スタック用
```

### 合格基準
- [x] ダブルバッファリングでティアリングなし
- [x] VSYNC タイミングでのバッファスワップ
- [x] グラデーションアニメーションが正常に表示される

---

## Layer 6: 描画ライブラリ ✅

### 目的
- embedded-graphics クレートの `DrawTarget` トレイトを `FrameBuffer` に実装
- 基本図形描画（点、線、矩形、円）
- フォント描画（英数字）

### 実装結果

**コミット**: `978e93b`

`FrameBuffer` に `DrawTarget<Color = Rgb666>` と `OriginDimensions` を実装した。
独自の `gfx/` モジュールは不要となり、embedded-graphics のエコシステムを
そのまま利用する構成となった。

```rust
// src/lcd/framebuffer.rs

impl DrawTarget for FrameBuffer {
    type Color = Rgb666;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where I: IntoIterator<Item = Pixel<Self::Color>> {
        // 座標をクリッピングし、set_pixel でパディング考慮済みアクセス
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        // Rectangle intersection + スライス fill で高速矩形塗りつぶし
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        // アクティブ領域のみクリア（パディング領域は BLACK のまま）
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size { Size::new(400, 96) }
}
```

**名前衝突の解決**: `FrameBuffer` には固有の `clear(u32)` メソッドがあり、
`DrawTarget::clear(Rgb666)` と衝突する。呼び出し側では完全修飾構文
`<FrameBuffer as DrawTarget>::clear(&mut fb, color)` で解決した。

**fill_solid 最適化**: `draw_iter` のピクセル単位描画に対し、`fill_solid` では
アクティブ領域との Rectangle intersection を計算し、行ごとにスライス `fill` で
一括書き込みを行う。大きな矩形の描画で大幅なパフォーマンス向上が得られる。

### デモ内容 (`src/bin/layer6_drawtest.rs`)

- "Hello, 300yen LCD!" テキスト（FONT_6X10）
- 赤矩形 + 緑円 + 青矩形 + 白線
- SMPTE 8 色カラーバー（白・黄・シアン・緑・マゼンタ・赤・青・黒）
- Channel ベースのダブルバッファリング（Layer 5 と同一方式）

### 合格基準
- [x] 点、線、矩形、円が正しく描画される
- [x] テキスト表示（英数字）が可能
- [x] 描画パフォーマンスが実用的（60fps 維持）

---

## Layer 7: 全フレーム DMA 転送

### 目的
- Layer 6 の横ジッター（SM0 FIFO アンダーラン）を根本解消
- SM0 に加え SM1 も DMA 化し、1フレームあたりの CPU 介入をゼロにする

### 問題の原因

Layer 6 では SM0 のピクセルデータを 1行ずつ DMA 転送し、SM1 には CPU が
`wait_push` でタイミングデータを供給していた。行間の CPU 介入
（DMA 再設定 + SM1 push）が散発的に SM0 TX FIFO バッファ（8エントリ ≈ 2.3μs）を
超過し、PIO が `out pins` で停止 → NCLK 一時停止 → 横ジッターとなっていた。

### 解決策: デュアル DMA 全フレーム転送

```text
DMA_CH0 → SM0: 拡張フレームバッファ 57,344 ワード (512×112) 一括転送
DMA_CH1 → SM1: タイミングバッファ 225 ワード 一括転送

embassy_futures::join::join(dma_ch0, dma_ch1) で並行実行
```

### フレームバッファ拡張

V-blank 期間 (16行) をフレームバッファ先頭に含める:

```text
行 0-15:   [BLACK × 512]  (V-blank)
行 16-111: [107 BLACK | 400 active | 5 BLACK]  (アクティブ)

合計: 512 × 112 = 57,344 ワード = 224 KB / バッファ
```

アプリケーション側は `set_pixel(x, y)` の内部オフセットが変わるのみで、
(0,0) 基準の座標体系は同一。

### SM1 タイミングバッファ

Layer 6 の CPU `wait_push` を置き換える静的バッファ:

```text
[SM1_HSYNC_COUNT, SM1_VSYNC_REST_COUNT, SM1_NORMAL_LINES_Y,
 SM1_HSYNC_COUNT, SM1_REST_COUNT,  ← 通常ライン #1
 SM1_HSYNC_COUNT, SM1_REST_COUNT,  ← 通常ライン #2
 ...
 SM1_HSYNC_COUNT, SM1_REST_COUNT]  ← 通常ライン #111

合計: 3 + 2 × 111 = 225 ワード (900 bytes)
```

### メモリ使用量

```text
拡張 FB ×2: 224 KB × 2 = 448 KB
SM1 buf:    ≈ 1 KB
合計:       449 KB / 520 KB SRAM (86%)
残り:       71 KB
```

### DMA チャネル

| チャネル | 用途 | ワード数 |
|---------|------|---------|
| DMA_CH0 | SM0 ピクセルデータ | 57,344 |
| DMA_CH1 | SM1 タイミングデータ | 225 |

### 検証内容
```rust
// src/bin/layer7_fullframe_dma.rs
// DMA_CH0: 拡張フレームバッファ → SM0 TX FIFO (57,344 ワード一括)
// DMA_CH1: タイミングバッファ → SM1 TX FIFO (225 ワード一括)
// join で両 DMA を並行実行
// → 横ジッターが解消された滑らかな表示
```

### 合格基準
- [ ] 横ジッターが完全に解消されている
- [ ] 60fps を維持できている
- [ ] ダブルバッファリング + VSYNC swap が正常に動作する
- [ ] NCLK が全フレームを通じて連続している

---

## 依存クレート

```toml
[dependencies]
embassy-executor = { version = "0.7", features = ["arch-cortex-m", "executor-thread"] }
embassy-rp = { version = "0.9", features = ["rp235xa"] }
embassy-time = { version = "0.4" }
embassy-sync = "0.6"
defmt = "1.0"
defmt-rtt = "1.0"
cortex-m = { version = "0.7", features = ["critical-section-single-core"] }
cortex-m-rt = "0.7"
panic-probe = { version = "0.3", features = ["print-defmt"] }
static_cell = "2"
pio = "0.3"
pio-proc = "0.3"
embedded-graphics = "0.8"    # Layer 6 で追加
fixed = "1"                  # 固定小数点（PIO分周器用）
```

## ビルド・フラッシュ手順

```bash
# ビルド
cargo build --example layer0_gpio_test --release

# フラッシュ + RTTログ表示（probe-rs経由）
cargo run --example layer0_gpio_test --release

# UF2 フラッシュ（BOOTSELモード）
cargo build --example layer0_gpio_test --release
elf2uf2-rs target/thumbv8m.main-none-eabihf/release/examples/layer0_gpio_test
# → Pico 2W を BOOTSEL モードで接続し、UF2 をコピー
```

## デバッグ戦略

### 各レイヤーのデバッグツール

| レイヤー | 主要デバッグツール | 確認項目 |
|---------|-----------------|---------|
| Layer 0 | オシロ / ロジアナ + defmt | GPIO 出力レベル |
| Layer 1 | オシロスコープ | クロック周波数・デューティ比 |
| Layer 2 | ロジックアナライザ | HSYNC/VSYNC タイミング |
| Layer 3 | 目視（LCD表示） + ロジアナ | 単色表示・同期 |
| Layer 4 | 目視 + defmt | テストパターン表示 |
| Layer 5 | 目視 + FPS カウンタ | アニメーション・ティアリング |
| Layer 6 | 目視 | 描画正確性 |
| Layer 7 | 目視 + ロジアナ | ジッター解消・NCLK連続性 |

### 問題切り分け手順

```
LCD が何も表示しない場合:
  1. Layer 0 に戻り GPIO 出力を確認
  2. 電源電圧を確認（DVDD, VGON, VSS）
  3. CCFL バックライトが点灯しているか確認
  4. TEST ピンを H にしてテストパターンが出るか確認
  5. Layer 1 で NCLK がLCD側に届いているか確認

表示が乱れる場合:
  1. Layer 2 で HSYNC/VSYNC タイミングをロジアナで確認
  2. NCLK 周波数が適切か確認
  3. H_BACK_PORCH (107 clk) が正確か確認
  4. VSYNC サンプリング（HSYNC+98clk）が正しいか確認

色がおかしい場合:
  1. RGBピンの接続順序を確認（B→G→R の順）
  2. MSB/LSB の向きを確認（秋月資料のピン番号逆転注意）
  3. PIO OUT のビット配置を確認
```

### defmt ログレベル

```rust
// .cargo/config.toml
[env]
DEFMT_LOG = "info"  # 通常
# DEFMT_LOG = "trace"  # デバッグ時
```

## スケジュール目安

| フェーズ | 期間 | 内容 |
|---------|------|------|
| 環境構築 | 1日 | プロジェクト生成、ビルド確認 |
| Layer 0 | 0.5日 | GPIO テスト |
| Layer 1 | 0.5日 | PIO クロック生成 |
| Layer 2 | 1-2日 | タイミング制御（最も難易度高い） |
| Layer 3 | 1日 | 固定パターン表示（**ここで初めて画面に映る**） |
| Layer 4 | 1日 | DMA 転送 |
| Layer 5 | 1日 | フレームバッファ + ダブルバッファ |
| Layer 6 | 1日 | 描画ライブラリ |
| **合計** | **7-8日** | |

> Layer 2（タイミング制御）が最難関。VSYNC のサンプリング条件（HSYNC+98clk）の
> 正確な実装が鍵。ロジックアナライザ必須。

## 注意事項

1. **NCLK を停止してはいけない** — PIO プログラム停止時もクロックが止まるとパネル損傷の可能性。
   PIO 初期化後は電源を切るまでクロックを止めない設計にする。

2. **電源投入順序** — LCD の電源（DVDD → VGON/VSS → バックライト）を順に投入。
   逆順だとパネルにストレスがかかる可能性あり。

3. **PIO 命令メモリは32命令** — 複雑なタイミング制御が1つのPIOプログラムに収まらない場合、
   DMA からのデータでタイミングを動的に制御する方式を検討。

4. **set 即値は 0-31** — 400 や 107 のカウントには `pull` + `mov x, osr` を使用。
   DMA でカウント値を供給する方式が現実的。
