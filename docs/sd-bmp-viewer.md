# SD カード BMP ビューアサンプル

`sd_bmp_viewer` は、基板の microSD スロットにある `IMAGE.BMP` と
`IMAGE2.BMP` を 10 秒ずつ表示する 400×96 LCD のサンプルです。既存の
`layer7_single_buffer_dma` と同じ全フレーム DMA 走査を使い、
フレームバッファは 1 枚だけです。

## 準備

1. microSD カードを FAT16 または FAT32 でフォーマットします。MBR の最初のパーティションと、カード先頭が直接 FAT ブートセクタの形式に対応します。
2. ルートディレクトリに `IMAGE.BMP` と `IMAGE2.BMP` を置きます（8.3 形式のファイル名）。
3. BMP は非圧縮の 24-bit RGB にします。下から上へ格納する通常の BMP と top-down BMP に対応します。PNG、JPEG、32-bit BMP、圧縮 BMP は対象外です。
4. カードを基板の microSD スロットに挿します。

v0.4.0 から LCD のバックバッファが RGB565 になったため、表示は赤・青が 5 bit (緑 6 bit) になります。
拡大縮小・複数枚のスライドショー・32 bit BMP は `ticker` の背景 ([ticker.md §1.2](ticker.md)) が対応しています。

画像が 400×96 より小さい場合は黒背景の中央に配置します。大きい場合は
拡大縮小せず中央の 400×96 部分を表示します。

元画像から LCD 全体を使う BMP を作るには、Python と Pillow を用意して
次を実行します。縦横比を保ち、上下を切り出すため左右に黒帯は入りません。
顔など上側の被写体を残しやすいよう、切り出し位置を少し上へ寄せています。

```powershell
python convert_image_to_bmp.py input.png IMAGE.BMP
```

2枚目の画像は、元の 3840×2160 スクリーンショットから両方の目が収まる範囲を
指定して、リポジトリ内の `IMAGE2.BMP` に変換しています。

```powershell
python convert_image_to_bmp.py VRChat_2026-05-07_01-56-51.698_3840x2160.png IMAGE2.BMP --crop 450 400 3450 1120
```

生成した2つの BMP を SD カードのルートにコピーしてください。

基板の SD 配線は `DAT0/MISO=GP0`、`CS=GP26`、`CMD/MOSI=GP27`、
`CLK=GP28` です。LCD は GP2～GP22 を使用します。この組み合わせは
ハードウェア SPI のピン割り当てではないため、サンプルは GPIO で SPI を生成します。
GP0 はこのサンプルでは SD 用で、UART TX には使えません。

## ビルド

```powershell
cargo build --release --bin sd_bmp_viewer
```

生成物は `target/thumbv8m.main-none-eabihf/release/sd_bmp_viewer` です。
このサンプルは picotool 対応の USB reset interface を公開します。初回は現在の
ファームにそのインターフェースがないため、[セットアップ手順](setup-guide.md)に
従って BOOTSEL モードに入れてください。2 回目以降は USB ケーブルを接続したまま
次のスクリプトでビルド、BOOTSEL への移行、書き込み、検証、再起動を行えます。

```powershell
.\flash-sd-bmp-viewer.ps1 -ExpectedSerial 681CF35F811A6140
```

上の番号は今回の基板の例です。別の基板では `picotool info` で確認した
16 桁の chipid に置き換えます。`-ExpectedSerial` は対象 RP2350 の USB
シリアル番号です。スクリプトは
`picotool load --ser ... -f -u -v -x -t elf ...` を実行します。USB の応答が
止まっている場合は自動移行できず、BOOTSEL 操作が必要です。

起動時は `IMAGE.BMP` の読み込みを終えてから LCD の走査を開始します。
表示中は PIO と DMA が LCD の走査を続け、CPU は次の BMP を有効画素だけの
一時領域へ先読みします。表示開始から10秒後に一時領域をフレームバッファへ
反映し、2枚を繰り返します。読み込みが10秒を超えた場合は完了後に切り替えます。
走査を止めないため、切り替える1フレーム程度は新旧の画像が混ざる可能性があります。
読み込みに失敗した場合は黒地にエラー名を表示し、同じ内容を defmt に出力します。
カードの差し替えは起動中に検出しないため、差し替え後は再起動してください。

LCD の走査タイミングは、[LTA042B010F の ESP32-S3 実動例](https://pol.hateblo.jp/entry/2023/11/27/001524)に合わせています。
水平は HSYNC 1・バックポーチ 107・画像 400・フロントポーチ 1 の 509 クロック、
垂直は VSYNC 1・バックポーチ 15・画像 96・フロントポーチ 1 の 113 行です。
HSYNC と VSYNC は待機時 Low、同期パルス時 High です。
これは画像切り出し量ではなく、LCD に送る信号の配置です。

この基板では左端の色目盛りから可視開始位置をフレーム x=98 と確認しました。
画像の 400 列をその位置へ配置します。画像は切り落とさず、
同期・ポーチを含む追加のフレームバッファも使いません。

## メモリと制限

- フレームバッファは `FrameBuffer` 1 個（509×113×4 = 230,068 byte）。
- 次画像の有効画素用に 400×96×4 = 153,600 byte の一時領域を使います。
  BMP の処理には別途 54 byte のヘッダ、最大 192 byte の読み込み領域、
  FAT ライブラリの 512 byte ブロックキャッシュがあります。
- SD カードの初期化と BMP 読み込みはビット単位の GPIO SPI で行うため、大きな画像は表示開始まで時間がかかります。
- 2枚の BMP を交互に表示します。画像を変えた場合は基板を再起動してください。
