# ファームウェア書き込み環境構築ガイド (Windows)

Raspberry Pi Pico 2W (RP2350) 向けファームウェアのビルドと書き込み環境を Windows 上に構築する手順です。

## 前提条件

- Windows 10 または Windows 11
- Rust ツールチェーン (stable)
- ターゲット: `thumbv8m.main-none-eabihf`

### Rust ツールチェーンの確認

本プロジェクトの `rust-toolchain.toml` により、必要なターゲットとコンポーネントは自動でインストールされます。手動で確認する場合:

```powershell
rustup show
rustup target list --installed
```

`thumbv8m.main-none-eabihf` が一覧に含まれていることを確認してください。含まれていない場合:

```powershell
rustup target add thumbv8m.main-none-eabihf
```

## picotool のインストール

picotool は Raspberry Pi 公式のファームウェア書き込みツールです。

### ダウンロード

1. [picotool リリースページ](https://github.com/raspberrypi/picotool/releases) から Windows 用バイナリをダウンロードする
   - Windows バイナリは `pico-sdk-tools` に含まれる場合と、`picotool` 単体で配布される場合がある
2. 任意のディレクトリに展開する（例: `C:\Users\<ユーザー名>\tools\picotool\`）

### PATH の設定

以下のいずれかの方法で `picotool.exe` を呼び出せるようにします。

**方法 A: システムの PATH に追加する**

「設定」→「システムの詳細設定」→「環境変数」から、`picotool.exe` のあるディレクトリを `PATH` に追加します。

**方法 B: `.cargo/config.toml` で絶対パスを指定する（後述）**

PATH を汚さずにプロジェクト単位で設定できます。

## .cargo/config.toml の設定

プロジェクトルートの `.cargo/config.toml` にランナーとビルドターゲットを設定します。

```toml
[target.thumbv8m.main-none-eabihf]
# picotool (PATH に通っている場合):
runner = "picotool load -u -v -x -t elf"

# picotool (絶対パス指定の場合):
# runner = "C:\\Users\\<ユーザー名>\\path\\to\\picotool.exe load -u -v -x -t elf"

# 代替: probe-rs (SWD デバッグプローブが必要)
# runner = "probe-rs run --chip RP235x --protocol swd"

[build]
target = "thumbv8m.main-none-eabihf"

[env]
DEFMT_LOG = "debug"
```

`runner` は `cargo run` 実行時に自動で呼び出されるコマンドです。picotool の各オプションの意味:

| オプション | 説明 |
|-----------|------|
| `load` | ファームウェアを書き込む |
| `-u` | BOOTSEL モードのデバイスを自動検出 |
| `-v` | 書き込み内容を検証 (verify) |
| `-x` | 書き込み後にリブート (execute) |
| `-t elf` | 入力ファイル形式を ELF に指定 |

## 書き込み手順

1. Pico 2W の **BOOTSEL ボタンを押しながら** USB ケーブルを PC に接続する
2. エクスプローラーに「RP2350」ドライブが表示されることを確認する
3. ターミナルで以下を実行する:

```powershell
cargo run --bin <ターゲット名> --release
```

4. picotool がファームウェアを自動で書き込み、Pico 2W がリブートする

> **補足:** 2 回目以降の書き込みでも、毎回 BOOTSEL モードで接続する必要があります（picotool が USB ブートローダと通信するため）。

## 利用可能なビルドターゲット

| ターゲット | 内容 |
|-----------|------|
| `layer0_gpio_test` | GPIO トグルテスト |
| `layer1_pio_clock` | PIO NCLK クロック生成 |
| `layer2_hsync_vsync` | HSYNC/VSYNC タイミング生成 |
| `layer3_solid_color` | LCD 固定色表示 |
| `layer4_dma_colorbar` | DMA カラーバー表示テスト |
| `layer5_framebuffer` | フレームバッファ + ダブルバッファリング |
| `layer6_drawtest` | embedded-graphics 描画テスト（テキスト・図形・カラーバー） |
| `layer7_fullframe_dma` | 単一バッファの正本。60 Hz DMA 走査と黄色い四角の往復動作 |
| `layer7_single_buffer_dma` | 正本の動作確認済みコピー（独立サンプル） |
| `layer7_double_buffer_dma` | 動作確認済みダブルバッファ版。計測 HUD 付き |
| `sd_bmp_viewer` | SD カードの `IMAGE.BMP` を単一バッファで表示（[準備手順](sd-bmp-viewer.md)） |

実行例:

```powershell
cargo run --bin layer0_gpio_test --release
```

現在の正本を動かす場合は次を実行してください。

```powershell
cargo run --bin layer7_fullframe_dma --release
```

LCD 右側で黄色い四角が左右に往復します。正本は単一バッファのため、
画面上に `CPUms` / `FRMms` の HUD はありません。保存サンプルの構成と
DMA 連鎖の説明は [Layer 7 のドキュメント](layer7-fullframe-dma.md)を参照してください。

従来の計測 HUD を確認する場合は `layer7_double_buffer_dma` を選びます。

## トラブルシューティング

### "program not found"

`picotool.exe` のパスが正しくありません。以下を確認してください:

- PATH に `picotool.exe` のディレクトリが含まれているか
- `.cargo/config.toml` の `runner` に指定した絶対パスが正しいか

```powershell
# PATH を確認
where.exe picotool
```

### "invalid memory range"

`build.rs` のリンカ引数指定が `cargo:rustc-link-arg-bins` であることを確認してください。`-examples` ではなく `-bins` を使う必要があります。

```rust
// build.rs — 正しい指定
println!("cargo:rustc-link-arg-bins=--nmagic");
println!("cargo:rustc-link-arg-bins=-Tlink.x");
println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
```

また、`[[example]]` ではなく `[[bin]]` ターゲットを使用してください。

### "cannot find linker script"

関連するリンカスクリプトは以下の 2 種類です:

- `link.x`: cortex-m-rt が提供。`build.rs` で `-Tlink.x` を明示的に指定している
- `link-rp.x`: embassy-rp が提供。`[[bin]]` ターゲットにのみ自動で適用される

RP2350 では `link-rp.x` は不要です（embassy-rp v0.9 では RP2040 用のみ生成）。`Cargo.toml` でバイナリを `[[bin]]` として定義し、ソースを `src/bin/` に配置してください。

### BOOTSEL モードに入れない

BOOTSEL ボタンを**電源投入前から**押し続けてください。手順:

1. USB ケーブルを抜く
2. BOOTSEL ボタンを押す
3. ボタンを押したまま USB ケーブルを接続する
4. エクスプローラーに「RP2350」が表示されたらボタンを離す

## 代替ツール

### elf2uf2-rs

USB 経由の書き込みツールです。SWD プローブは不要です。

```powershell
cargo install elf2uf2-rs
```

`.cargo/config.toml` での設定:

```toml
runner = "elf2uf2-rs -s"
```

> **注意:** elf2uf2-rs v2.2.0 は RP2350 で「entry point not in mapped part」エラーが発生します。RP2350 対応には `--family rp2350-arm-s` オプションが必要ですが、バージョンによっては未対応です。

### probe-rs

SWD デバッグプローブ経由の書き込みツールです。デバッグ機能と defmt ログのリアルタイム表示が可能です。

```powershell
cargo install probe-rs-tools
```

`.cargo/config.toml` での設定:

```toml
runner = "probe-rs run --chip RP235x --protocol swd"
```

別途 SWD 対応のデバッグプローブ（Raspberry Pi Debug Probe など）が必要です。

## 重要な教訓

このプロジェクトの構築過程で判明した事項です。

- **pico-setup-windows は使用不可** — 2024 年 9 月にアーカイブ済み。代わりに picotool を直接ダウンロードしてください
- **embassy-rp のリンカスクリプト** — `-Tlink-rp.x` は `[[bin]]` ターゲットにのみ提供される。`examples/` ディレクトリのコードには適用されない
- **RP2350 では `-Tlink-rp.x` は不要** — embassy-rp v0.9 では RP2040 用のリンカスクリプトのみ生成される。RP2350 では `link.x` と `defmt.x` のみ必要
- **サンプルコードの配置** — embassy-rs の慣例に従い、`examples/` ではなく `src/bin/` にバイナリを配置する
- **elf2uf2-rs の RP2350 互換性** — v2.2.0 では RP2350 で「entry point not in mapped part」エラーが発生する。picotool を推奨
