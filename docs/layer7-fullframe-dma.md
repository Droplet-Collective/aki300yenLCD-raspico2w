# Layer 7: 60 Hz 全フレーム DMA 走査

## 現行の実装

| ビルド対象 | 役割 |
|---|---|
| `layer7_fullframe_dma` | **単一バッファの正本**。固定図形と左右に動く黄色い四角を表示 |
| `layer7_single_buffer_dma` | 上記の実機確認済みコードを別ファイルに保存したサンプル |
| `layer7_double_buffer_dma` | 以前に実機確認したダブルバッファ版のサンプル。計測 HUD を含む |

正本は [`src/bin/layer7_fullframe_dma.rs`](../src/bin/layer7_fullframe_dma.rs)。
保存サンプルはそれぞれ独立した Cargo バイナリであり、正本を後から編集しても自動では更新されない。

## 走査の仕組み

- LCD の表示領域は 400 × 96 ピクセル。1 枚のバッファは 512 × 112 個の `u32` で、先頭 16 行が垂直ブランキング、各行が `107 黒 + 400 表示 + 5 黒`。
- 1 枚の容量は 57,344 ワード、229,376 バイト（224 KiB）。RGB666 のピン並びに合わせたワードを保持する。
- [`TARGET_FPS`](../src/lcd/timing.rs) は 60 Hz、目標 NCLK は 3,440,640 Hz。
- PIO0 SM0 が RGB と NCLK、SM1 が HSYNC と VSYNC を出力する。初回は `MULTI_CHAN_TRIGGER` でピクセル用 CH0 と同期用 CH1 を起動する。
- 以後は `CH0 → CH2 → CH0`、`CH1 → CH3 → CH1` の DMA 連鎖が各フレームの転送元アドレスを再設定する。行ごとの CPU 供給はない。

```text
単一バッファ (512 × 112 × 4 バイト)
       └─ CH0 ─→ PIO0 SM0 (RGB + NCLK)
            ↑ CH2 が毎フレーム先頭アドレスを再設定

SM1 用の固定タイミングデータ (225 ワード)
       └─ CH1 ─→ PIO0 SM1 (HSYNC + VSYNC)
            ↑ CH3 が毎フレーム先頭アドレスを再設定
```

## 単一バッファでの動き

起動時に文字・図形・カラーバーを一度描く。黄色い 12 × 12 ピクセルの四角だけを、右側の `x=230..380, y=55` で 更新できたフレームにつき 2 ピクセルずつ往復させる。

CH0 の完了フラグを1 ms 間隔を目安に確認する。CH0 が完了すると DMA 連鎖は既に次フレームを開始しているため、CH0 の `TRANS_COUNT`（現在の転送残数）から読み取り位置を推定する。矩形の行まで **16 ライン以上** 残っている場合だけ旧位置を消して新位置を描き、遅れたフレームは更新を見送る。対象ピクセルは volatile 書き込みとメモリバリアで更新する。[RP2350 の DMA 仕様](https://datasheets.raspberrypi.com/rp2350/rp2350-datasheet.pdf)には、`TRANS_COUNT` の読み取り値が現在の転送残数と記載されている。

この方式で走査中に更新するのは小矩形だけ。任意の全画面描画をティアリングなしで行える汎用 API ではない。PIO の `FDEBUG.TXSTALL` は各フレームで読み、起動直後の標本を除き、停止を検出した場合は RTT に警告を出す。単一バッファ版に画面上の `CPUms` / `FRMms` HUD はない。

## メモリとダブルバッファ版

| 対象 | フレームバッファ | release ELF の静的主 RAM | 512 KiB 主 RAM の残り |
|---|---:|---:|---:|
| 単一バッファ正本 | 224 KiB × 1 | 約 226 KiB | 約 286 KiB |
| ダブルバッファサンプル | 224 KiB × 2 | 約 450 KiB | 約 62 KiB |

残りの値は静的領域の末尾からスタック開始位置までのアドレス差で、全量を安全に追加割り当てできるという意味ではない。主 RAM と別にある 4 KiB × 2 の SRAM 領域は、この表に含めていない。

ダブルバッファサンプルは描画用と走査用を所有権付きチャネルで交換する。走査は 60 Hz で連続するが、現在の交換手順では画面内容の更新は約 30 回/秒。従来の `CPUms`、`FRMms`、`SM0/SM1 stalls` の HUD はこのサンプルに残してある。

## ビルドと実機確認

```powershell
cargo build --release --bin layer7_fullframe_dma
cargo build --release --bin layer7_single_buffer_dma
cargo build --release --bin layer7_double_buffer_dma
```

書き込み時は基板を BOOTSEL モードにし、`picotool info -d` で対象の RP2350 とシリアルを確認してから、対象 ELF を `picotool load -u -v -x ... -t elf --ser <serial>` で指定する。手順の詳細は [セットアップガイド](setup-guide.md)を参照。

正本のアニメーション版は release ビルド、実機への書き込みと Flash 照合を完了し、ユーザーが画面表示と四角の往復動作を目視確認した。保存サンプルは同じソースのコピーで、別バイナリとしてのビルドを確認する。
