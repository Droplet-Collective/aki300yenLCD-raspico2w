# Wi-Fi ステータス表示サンプル (`wifi_status`)

`wifi_status` は、基板の microSD にある `wifi.txt` の SSID / パスワードで
Pico 2 W の CYW43439 を Wi-Fi に接続し、接続状態と周辺アクセスポイントの
電波強度 (RSSI) を 400×96 LCD に表示するサンプルです。LCD の走査は
`sd_bmp_viewer` と同じ全フレーム DMA (PIO0 SM0/SM1 + DMA CH0〜CH3) を使います。

## 画面

```
┌──────────────────────────────────────────────────────────────┐
│ MyHomeAP 192.168.0.23 -52dBm                        scan #12 │ ← 状態行
├──────────────────────────────────────────────────────────────┤
│ MyHomeAP             ████████████████████░░░░░   -52dBm ch6  │ ← 接続中 AP はシアン
│ Neighbor-2G          ████████████░░░░░░░░░░░░░   -68dBm ch1  │
│ ...                                                          │  (最大 8 件、RSSI 順)
└──────────────────────────────────────────────────────────────┘
```

- 1 行目 (状態行)
  - 起動直後: `Wi-Fi: starting... (SSID)`
  - 接続中: `connecting to SSID...` → `SSID: waiting for DHCP...`
  - 接続済み: `SSID  IPアドレス  自APのRSSI  scan #n`
  - 接続失敗: `join failed (status N), retry in Ns` (5 秒から倍々で最大 60 秒までリトライ)
  - `wifi.txt` が無い/不正: `wifi.txt not found (scan only)` などのメッセージを表示し、
    接続せずにスキャンだけ続けます。
- 2 行目以降: 約 10 秒ごとのパッシブスキャン結果を RSSI の強い順に最大 8 件。
  SSID (21 文字まで、ASCII 以外のバイトは `?`) と棒グラフ (-100〜-30 dBm)、
  RSSI 値、チャネル番号を表示します。棒の色は -60 dBm 以上が緑、-75 dBm 以上が黄、
  それ以下が赤です。ステルス (SSID 空) の AP は表示しません。

## wifi.txt の書き方

microSD (FAT16/FAT32) のルートに `wifi.txt` (8.3 形式では `WIFI.TXT`) を置きます。
UTF-8 で 2 行、改行は LF でも CRLF でも構いません。

```
MySSID
MyPassword123
```

- 1 行目: SSID (最大 32 バイト)
- 2 行目: WPA2 パスフレーズ (8〜63 文字)。2 行目を省略するとオープンネットワークとして接続します。
- 行末の CR/LF だけを取り除きます。空行は読み飛ばします。先頭の BOM は無視します。
- SD カードの認証情報は平文です。カードの取り扱いに注意してください。

SD の配線は `sd_bmp_viewer` と同じ `DAT0/MISO=GP0`、`CS=GP26`、`CMD/MOSI=GP27`、`CLK=GP28` です。

## ハードウェア割り当て

| 機能 | GPIO / リソース |
|------|-----------------|
| LCD RGB / NCLK / HSYNC / VSYNC | GP2〜GP22、PIO0 SM0/SM1、DMA CH0〜CH3 |
| microSD (GPIO SPI) | GP0, GP26, GP27, GP28 |
| CYW43439 (Wi-Fi) | PWR=GP23, CS=GP25, DIO=GP24, CLK=GP29、PIO1 SM0、DMA CH4 |
| USB (picotool reset interface) | USB |

CYW43439 の GPIO は Pico 2 W 基板内で配線済みで、LCD / SD のピンとは重なりません。
PIO も LCD (PIO0) と Wi-Fi (PIO1) で分けています。

## ビルドと書き込み

```powershell
cargo build --release --bin wifi_status
picotool load -f -v -x -t elf target\thumbv8m.main-none-eabihf\release\wifi_status
```

CYW43439 のファームウェア (`firmware/cyw43/43439A0.bin`, `43439A0_clm.bin`) は
`include_bytes!` でバイナリに含めます (約 230 KB)。ライセンスは
`firmware/cyw43/LICENSE-permissive-binary-license-1.0.txt` を参照してください。

`sd_bmp_viewer` と同じ picotool 用 USB reset interface (`src/usb_reset.rs`) を
公開しているため、2 回目以降は BOOTSEL ボタンなしで `picotool load -f` で
書き換えられます。UF2 で書き込む場合は GitHub Actions の成果物
`wifi_status.uf2` を BOOTSEL ドライブにコピーします。

## 実装メモ

- クレート: `cyw43 0.6` + `cyw43-pio 0.9` + `embassy-net 0.8` (embassy-rp 0.9 と組み合わせられる世代)。
- `wifi.txt` の読み込みは起動時に 1 回だけ行い、その後 SD は使いません。
- `control.join()` は WPA2 (`JoinAuth::Wpa2`) を指定します。WPA3 専用 AP には接続できません。
- スキャンは接続中も行います。パッシブスキャン中は一時的に通信が滞ることがあります。
- フレームバッファは 1 枚 (約 230 KB) で、走査中に直接描き替えます。書き換え中の
  1 フレームだけ新旧の表示が混ざることがあります。

## 未検証事項

このサンプルはビルドのみ確認しており、実機では未確認です。特に次の点は実機で確認してください。

- PIO1 / DMA CH4 を Wi-Fi に使いながら PIO0 / DMA CH0〜CH3 の LCD 走査が乱れないこと
- CYW43439 のファームウェアロード中 (起動から約 1〜2 秒) も LCD 走査が継続すること
- RAM 使用量 (BSS 約 257 KB + スタック) が 512 KB に収まっていること
- GPIO SPI による `wifi.txt` 読み込みが安定していること
