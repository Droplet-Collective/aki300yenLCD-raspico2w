# SD カード BMP ビューアサンプル

`sd_bmp_viewer` は、基板の microSD スロットから `IMAGE.BMP` を一度だけ読み、
400×96 LCD に表示する独立サンプルです。既存の `layer7_single_buffer_dma` と
同じ全フレーム DMA 走査を使い、フレームバッファは 1 枚だけです。

## 準備

1. microSD カードを FAT16 または FAT32 でフォーマットし、最初のパーティションを使います。
2. ルートディレクトリに `IMAGE.BMP` を置きます（8.3 形式のファイル名）。
3. BMP は非圧縮の 24-bit RGB にします。下から上へ格納する通常の BMP と top-down BMP に対応します。PNG、JPEG、32-bit BMP、圧縮 BMP は対象外です。
4. カードを基板の microSD スロットに挿します。

画像が 400×96 より小さい場合は黒背景の中央に配置します。大きい場合は
拡大縮小せず中央の 400×96 部分を表示します。

元画像から LCD 全体を使う BMP を作るには、Python と Pillow を用意して
次を実行します。縦横比を保ち、上下を切り出すため左右に黒帯は入りません。
顔など上側の被写体を残しやすいよう、切り出し位置を少し上へ寄せています。

```powershell
python convert_image_to_bmp.py input.png IMAGE.BMP
```

生成した `IMAGE.BMP` を SD カードのルートにコピーしてください。

基板の SD 配線は `DAT0/MISO=GP0`、`CS=GP26`、`CMD/MOSI=GP27`、
`CLK=GP28` です。LCD は GP2～GP22 を使用します。この組み合わせは
ハードウェア SPI のピン割り当てではないため、サンプルは GPIO で SPI を生成します。
GP0 はこのサンプルでは SD 用で、UART TX には使えません。

## ビルド

```powershell
cargo build --release --bin sd_bmp_viewer
```

生成物は `target/thumbv8m.main-none-eabihf/release/sd_bmp_viewer` です。
書き込みは通常の [セットアップ手順](setup-guide.md)に従い、BOOTSEL モードで
`cargo run --release --bin sd_bmp_viewer` を実行します。

起動時は SD 読み込みを終えてから LCD の走査を開始します。読み込みに失敗した場合は
黒地にエラー名を表示し、同じ内容を defmt に出力します。カードの差し替えや
`IMAGE.BMP` の変更を反映するには再起動してください。

## メモリと制限

- フレームバッファは `FrameBuffer` 1 個（512×112×4 = 229,376 byte）。
- BMP の処理は 54 byte のヘッダと最大 192 byte の画素読み込み領域で行い、画像全体のコピーは作りません。FAT ライブラリには別途 512 byte のブロックキャッシュがあります。
- SD カードの初期化と BMP 読み込みはビット単位の GPIO SPI で行うため、大きな画像は表示開始まで時間がかかります。
- このサンプルは固定画像表示です。実機での SD 読み込み・表示確認は別途必要です。
