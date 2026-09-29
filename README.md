# aki300yenLCD-raspico2w — Pico 2 W の Wi-Fi OTA リファレンス実装 (Rust / Embassy)

Raspberry Pi Pico 2 W (RP2350) で秋月電子の 300 円 LCD **LTA042B010F (400×96 RGB666 TFT)** を
PIO + DMA で駆動し、その表示を持ったファームウェアを **Wi-Fi 経由 (GitHub Release) で自己更新する**
リファレンス実装です。`no_std` の Rust と [Embassy](https://embassy.dev/) で書かれています。

## 概要

- **LCD 駆動**: PIO0 の 2 つのステートマシンが RGB666 + NCLK と HSYNC/VSYNC を出力し、DMA CH0〜CH3 の
  自走リングが 60 Hz でフレームバッファを流し続けます。CPU は垂直ブランキング中にバックバッファを
  フロントへコピーするだけで、フラッシュ消去・書き込み中 (割り込み禁止、数百 ms) でも走査は乱れません。
- **Wi-Fi OTA**: `wifi_ota` bin が SD カードの `WIFI.TXT` で Wi-Fi に接続し、60 秒ごとに GitHub Release の
  `manifest.json` を HTTPS で取得します。自分より新しい版があれば `wifi_ota.bin` を **他方の A/B 区画** に
  ストリーミング書き込みし、SHA-256 と読み戻しで検証してから RP2350 bootrom の FLASH_UPDATE 再起動で
  新版を起動します。新版は TBYB (Try Before You Buy) で起動し、Wi-Fi + DHCP が通ったら `explicit_buy` で
  確定、通らなければ bootrom のウォッチドッグで旧版に戻ります。独自ブートローダは書かず、bootrom の
  標準機能 (A/B 版数比較・FLASH_UPDATE・TBYB) だけで構成しています。
- **実機確認**: 2026-09-29 に実機 (1 台) で v0.2.6 への自動更新 (ダウンロード → 検証 →
  FLASH_UPDATE 起動 → TBYB 自己診断 → 確定) が通ることを確認しました。
- **ネットワーク・ティッカー (v0.3.0〜)**: OTA の土台の上に、NTP 時計・Open-Meteo の天気・GitHub 上の
  `message.txt` を流す表示を載せた `ticker` bin。v0.3.0 からは Release の OTA イメージがこれになり、
  `wifi_ota` 0.2.x が動いている機体もそのまま `ticker` に切り替わります ([ticker.md](docs/ticker.md))。
  v0.4.0 からは SD の写真 (BMP) をスライドショーで背景にし、その上に半透明の板で情報を重ねます。
  画面は PC のシミュレータ `tools/ui-sim` で書き込む前に確かめられます ([ui-sim.md](docs/ui-sim.md))。

## ハードウェア

| 部品 | 内容 |
|---|---|
| マイコン | Raspberry Pi Pico 2 W (RP2350A、フラッシュ 4 MB、SRAM 512 kB、CYW43439 Wi-Fi) |
| LCD | LTA042B010F 400×96 RGB666 TFT (秋月電子「300 円 LCD」)。DE 無し、HSYNC/VSYNC 同期 |
| microSD | GPIO ビットバング SPI (基板配線がハード SPI のピン組ではないため) |
| 電源 | LCD の ±13.8 V (VGON/VSS) と CCFL インバータが別途必要 ([hardware-connection.md](docs/hardware-connection.md)) |

ピン割り当て:

| 機能 | GPIO | リソース |
|---|---|---|
| LCD B5..B0 / G5..G0 / R5..R0 | GP2〜GP7 / GP8〜GP13 / GP14〜GP19 | PIO0 SM0 (OUT 18 本) |
| LCD NCLK | GP20 | PIO0 SM0 (side-set) |
| LCD HSYNC / VSYNC | GP21 / GP22 | PIO0 SM1 (SET 2 本) |
| LCD 画素 / 同期データ転送 | — | DMA CH0〜CH3 |
| microSD DAT0(MISO) / CS / CMD(MOSI) / CLK | GP0 / GP26 / GP27 / GP28 | GPIO ビットバング |
| CYW43439 PWR / DIO / CS / CLK | GP23 / GP24 / GP25 / GP29 (基板内配線) | PIO1 SM0、DMA CH4 |
| USB | — | picotool 用 reset interface (`picotool load -f`) |

LCD のタイミング (H 合計 509 clk = 表示 400 + ブランキング 109、V 合計 113 行 = 表示 96 + 17、
NCLK ≈ 3.45 MHz) は [datasheet-LTA042B010F.md](docs/datasheet-LTA042B010F.md) と `src/lcd/timing.rs` に、
配線と電源回路は [hardware-connection.md](docs/hardware-connection.md)、事前検討は
[feasibility-report.md](docs/feasibility-report.md) にあります。

## リポジトリ構成

```
src/
  lib.rs            ライブラリクレート (各 bin から共用)
  lcd/              LCD 走査層: display.rs (PIO0 + DMA リング、BackBuffer / present)、
                    timing.rs (LTA042B010F のタイミング定数)、framebuffer.rs、pio_program.rs
  wifi.rs           CYW43439 の立ち上げ (PIO1 + DMA CH4)、embassy-net (DHCP)、WIFI.TXT の読み込み
  ota/              OTA 本体: manifest.rs (manifest.json と semver 比較)、http.rs (HTTPS、302 追跡)、
                    slot.rs (書き込み先区画の決定、セクタ消去 / ページ書き込み、SHA-256、読み戻し)
  ab_boot.rs        bootrom の A/B・TBYB API (get_sys_info / get_partition_table_info / explicit_buy / reboot)
  boot_trace.rs     TBYB 起動の進行を WATCHDOG.SCRATCH5〜7 に記録し、巻き戻り後の旧版が読む
  image_def.rs      Cargo.toml の version から VERSION 項目付き IMAGE_DEF を生成 (TBYB フラグも)
  usb_reset.rs      picotool 用 USB reset interface
  sdcard.rs         microSD (GPIO SPI) と FAT ボリューム
  ota/app.rs        OTA + TBYB + 接続管理の実行部 (ticker / wifi_ota 共用)
  ticker/           ticker の部品 (暦、ticker.txt、Open-Meteo、SNTP、写真のスライドショー)
  ui/               画面の描画 (no_std の純粋なコード、tools/ui-sim と共用): 画面構成、ガラス板、AA 数字、
                    天気アイコン、BMP の読み込み (拡大縮小 + 切り出し)
  font/shinonome.rs 東雲フォント (14 ドット日本語) の検索と描画
  bin/              下記の実行ファイル
partition/          A/B パーティションテーブル (pico2w-ab.json → pico2w-ab.uf2)
scripts/            make-ota-image.sh (ELF → .bin/.uf2/.sha256)、make-manifest.sh、make-partition-table.sh
fonts/shinonome/    東雲フォント (14 ドット) のビットマップテーブルとライセンス (Public Domain)
fonts/dejavu/       時計 / 気温の AA 数字の元 (DejaVu Sans) のライセンス
ticker/message.txt  ticker が流す文字 (main を書き換えれば 5 分以内に反映)
tools/              bdf2bin.py (BDF → フォントテーブル)、ticker-tests (ホストでのユニットテスト)、
                    ui-sim (画面シミュレータ: PNG / GIF、見本の背景 BMP。docs/ui-sim.md)
.github/workflows/  build.yml (全 bin をビルド、v* タグで Release)、release.yml (workflow_dispatch で Release)
docs/               設計・手順・実機で得た知見 (下記リンク)
```

`[[bin]]` 一覧 (`Cargo.toml`):

| bin | 内容 |
|---|---|
| `ticker` | **Release の OTA イメージ (v0.3.0〜)**。NTP 時計 + Open-Meteo 天気 + `ticker/message.txt` の流れる文字 (東雲フォント 14 ドット) + OTA。v0.4.0〜 SD の BMP のスライドショーを背景に、ガラス風の板で重ね描き。設定は SD の `TICKER.TXT` ([ticker.md](docs/ticker.md))。Release 用は `--features tbyb` |
| `wifi_ota` | OTA の最小構成 (v0.2.x の OTA イメージ)。`wifi_status` の表示 + GitHub Release からの自己更新。OTA / TBYB の本体は `src/ota/app.rs` で `ticker` と共用 |
| `wifi_status` | SD の `WIFI.TXT` で Wi-Fi に接続し、周辺 AP の RSSI を LCD に表示 ([wifi-status.md](docs/wifi-status.md)) |
| `ota_selftest` | Wi-Fi 無しで A/B・TBYB を確認する診断 bin。起動区画・版数・TBYB 状態を表示して `explicit_buy` ([ota-setup.md](docs/ota-setup.md)) |
| `ota_selftest_min` | 実機二分探索用の最小表示 bin |
| `sd_bmp_viewer` | SD の `IMAGE.BMP` / `IMAGE2.BMP` を 10 秒ずつ表示 ([sd-bmp-viewer.md](docs/sd-bmp-viewer.md)) |
| `layer0_gpio_test` 〜 `layer7_*` | LCD 駆動を段階的に作った検証用 bin (GPIO → PIO クロック → 同期 → 単色 → DMA → 全フレーム DMA)。 [layer7-fullframe-dma.md](docs/layer7-fullframe-dma.md) |
| `pico2w-300yen-lcd` | `src/main.rs` (初期の GPIO 動作確認) |

## OTA の仕組み

設計の全体は [ota-design.md](docs/ota-design.md)、`wifi_ota` の挙動と LCD の読み方は
[wifi-ota.md](docs/wifi-ota.md) にあります。要点だけ挙げます。

### A/B パーティション

`partition/pico2w-ab.json` を `picotool partition create` で UF2 にして、フラッシュ先頭に 1 回だけ書きます。

| 領域 | ストレージオフセット | サイズ | 用途 |
|---|---|---|---|
| slot 0 | 0x000000–0x000FFF | 4 kB | パーティションテーブル |
| slot 1 | 0x001000–0x001FFF | 4 kB | 予約 (テーブルの A/B 用、未使用) |
| P0 `app-a` | 0x002000–0x1E1FFF | 1920 kB | アプリ A (family `rp2350-arm-s`) |
| P1 `app-b` | 0x1E2000–0x3C1FFF | 1920 kB | アプリ B (`link: ["a", 0]` で P0 の B) |
| P2 `data` | 0x3C2000–0x3FCFFF | 236 kB | 将来の設定保存用 (起動では無視) |
| 未区画 | 0x3FD000–0x3FFFFF | 12 kB | 空き (最終ページは RP2350-E10 対策の絶対ブロック用) |

イメージは常に 0x10000000 でリンクし、どちらの区画に置かれても bootrom の QMI アドレス変換で
0x10000000 に見えるので、区画別のビルドは不要です (`memory.x` の `FLASH LENGTH = 1920K`)。

### 版数と起動選択

- `src/image_def.rs` が `Cargo.toml` の `version` から VERSION 項目付きの IMAGE_DEF を各 bin に埋め込みます
  (`0.2.6` → IMAGE_DEF `0.206`)。bootrom は A/B のうち版数の高い方を起動します。
- `--features tbyb` を付けると IMAGE_DEF に TBYB フラグが立ちます。この版は FLASH_UPDATE 起動のときだけ選ばれ、
  bootrom が 16.7 s のウォッチドッグを仕掛けた状態で起動します。`explicit_buy` を呼ぶまで確定しません。

### 更新フロー (`ticker` / `wifi_ota` 共通、`src/ota/app.rs`)

1. DHCP 完了の 5 秒後、以後 60 秒ごとに `https://github.com/<repo>/releases/latest/download/manifest.json` を取得
   (github.com の 302 → `*.githubusercontent.com` のリダイレクトは自前で追う。404 = Release 無しは正常)。
2. `{"version","bin","size","sha256"}` を読み、自分の `CARGO_PKG_VERSION` より厳密に新しいときだけ続行。
3. 書き込み先は自分が起動している区画の他方。まず先頭セクタ (IMAGE_DEF) を消して無効化。
4. manifest の `bin` (v0.3.0〜 `ticker.bin`、v0.2.x は `wifi_ota.bin`) を 4 kB ずつ受信しながらセクタ消去 →
   256 B ページ書き込み。同時に SHA-256 を計算。名前は manifest に従うので、bin の種類の切り替えも OTA でできる。
5. サイズと SHA-256 が manifest と一致したら先頭セクタを書き、**0x1C000000 (アドレス変換もキャッシュも通さない
   XIP 窓)** から全域を読み戻してもう一度 SHA-256 を照合。
6. `reboot(FLASH_UPDATE, 対象区画)` → 新版が TBYB で起動。
7. 新版はウォッチドッグを 2 s ごとに延長しながら LCD 走査 + Wi-Fi join + DHCP を待ち、通れば `explicit_buy`。
   起動から 120 s 以内に通らなければ延長をやめ、ウォッチドッグで旧版に戻る。
8. 巻き戻ったとき: 新版は進行段階と稼働時間を `WATCHDOG.SCRATCH5〜7` に書き続けているので、旧版が起動時に読んで
   `WATCHDOG.REASON` や BOOT_INFO の診断ワードと共に LCD に出す。旧版は対象区画に manifest と同じイメージが
   あることから巻き戻りを検出し、10 分後に FLASH_UPDATE 起動を再試行する。
9. DNS / TLS / HTTP / フラッシュ / ハッシュの失敗は LCD に表示し、60 s → 最大 600 s のバックオフで再試行。
   検証を通らないイメージで再起動することはない。

## ビルド

```sh
rustup target add thumbv8m.main-none-eabihf     # rust-toolchain.toml が stable + rust-src + llvm-tools を指定
cargo build --release                           # 全 bin
cargo build --release --bin ticker --features tbyb     # OTA で配る (Release 用) イメージ (v0.3.0〜)
(cd tools/ticker-tests && cargo test)                   # ticker の純粋なロジックをホストでテスト
(cd tools/ui-sim && cargo test --release && cargo run --release -- --scenario scenarios/default.json --out out/)
                                                        # 画面を PC で描いて out/*.png / *.gif に (docs/ui-sim.md)
```

ELF から配布物を作るには picotool 2.x が必要です:

```sh
PICOTOOL=/path/to/picotool scripts/make-ota-image.sh target/thumbv8m.main-none-eabihf/release/ticker out
#   → out/ticker.bin (OTA 用生イメージ) / out/ticker.uf2 / out/ticker.sha256
scripts/make-manifest.sh out/ticker.bin 0.3.0 out/manifest.json
scripts/make-partition-table.sh                 # partition/pico2w-ab.uf2
```

`make-ota-image.sh` が作る UF2 には RP2350-E10 対策の「絶対ブロック」(`picotool uf2 convert --abs-block 0x103FFF00`)
が付きます。パーティションテーブルのある RP2350 A2 で BOOTSEL ドライブへドラッグ&ドロップするにはこれが
必要です ([ota-setup.md §2.1](docs/ota-setup.md))。書き込み環境 (Windows) は [setup-guide.md](docs/setup-guide.md)、
CI アーティファクトの入手は [ci-build.md](docs/ci-build.md)。

## 初回セットアップ

1. **パーティションテーブル (1 回だけ)**: BOOTSEL で接続し `picotool load -v partition/pico2w-ab.uf2`
   (または `pico2w-ab.uf2` を D&D)。`picotool partition info` で `app-a` / `app-b` / `data` が見えれば OK
   ([ota-setup.md §1](docs/ota-setup.md))。
2. **Wi-Fi 設定**: microSD (FAT16/32) のルートに `WIFI.TXT` を置く (1 行目 SSID、2 行目 WPA2 パスフレーズ。
   [wifi-status.md](docs/wifi-status.md))。
3. **最初のアプリ**: Release から `ticker-plain.uf2` (TBYB 無し。Wi-Fi 未設定でも起動する) を D&D するか
   `picotool load -f -v -x ticker-plain.uf2`。Wi-Fi が確実に通る機体なら `ticker.uf2` (TBYB 付き) でもよいが、
   Wi-Fi + DHCP が通らないと 16.7 s で旧版へ戻る (旧版が無ければ BOOTSEL に落ちる)。
   (`wifi_ota-plain.uf2` も同様に使える。任意で `TICKER.TXT` を SD に置く: [ticker.md §4](docs/ticker.md))
4. 以後は電源と Wi-Fi だけで更新されます。LCD の下 2 行に `ticker vX.Y.Z [via OTA] slot A/B TBYB:...` と
   `OTA: ...` の進捗が出ます ([wifi-ota.md §4](docs/wifi-ota.md)、[ticker.md §1](docs/ticker.md))。

## リリースと自動更新

1. `Cargo.toml` の `version` を上げる (IMAGE_DEF の版数は自動で追従)。
2. main にマージする。
3. Release を作る。**タグ `vX.Y.Z` の X.Y.Z と `Cargo.toml` の `version` が一致しないと CI が失敗し、Release は
   作られません。**
   - Actions → `release` → Run workflow で `version` を入力 (`gh workflow run release.yml -f version=X.Y.Z`)。
     `release.yml` が版数一致とタグ未存在を確認し、`build.yml` のビルドジョブを呼んでから注釈付きタグを打ち、
     Release にアセットを添付します。
   - または `git tag vX.Y.Z && git push origin vX.Y.Z`。`build.yml` の `push: tags` が同じことをします。
4. Release には全 bin の `.uf2` / `.bin` / `.sha256`、`pico2w-ab.uf2`、そして `--features tbyb` でビルドした
   `ticker.bin` / `ticker.uf2` (と `wifi_ota.bin` / `wifi_ota.uf2`) と、`ticker.bin` を指す `manifest.json` が付きます。
   実機は常に `releases/latest/download/<name>` を見るので、アセット名に版数は入れません。
5. 稼働中の機体は 60 秒以内に manifest を見に行き、新しければ更新 → 再起動 → 自己診断 → 確定します。

## 実機で学んだこと

- **RP2350-E10**: パーティションテーブルがある A2 機体では、UF2 の先頭に絶対ブロックが無いと BOOTSEL ドライブへの
  D&D が「何も起きない」形で失敗する。`picotool load` (PICOBOOT) は影響を受けない。
- **DMA が読むメモリはフラッシュ操作中は SRAM だけ**: フラッシュ消去・書き込み中は XIP 窓への DMA アクセスが
  バスフォールトになり、そのチャネルは停止したままになる。HSYNC/VSYNC 用のテーブルが `.rodata` にあったときは
  `explicit_buy` 1 回で画面が砂嵐になった。`#[link_section = ".data…"]` で RAM に置いて解決。
- **embassy の `Pio` ハンドルは drop しない**: `Common` / `StateMachine` の最後の drop で PIO が使っていた GPIO の
  FUNCSEL が NULL に戻り、LCD も CYW43 も止まる。`mem::forget` で保持する。
- **GitHub の 302 応答ヘッダは 5〜6 kB**: Content-Security-Policy と Set-Cookie で膨らむので、HTTP 受信バッファは
  8 kB にした。アセット配信ホストは RSA 4096 の証明書で、embedded-tls の `rsa` feature (alloc) 無しでは TLS 1.3 の
  ハンドシェイクが成立しない。
- **TBYB のウォッチドッグは 16.7 s (24 bit × 1 µs)**: Wi-Fi join + DHCP はこれに収まらないことがあるので、buy 待ちの
  間は 2 s ごとに `WATCHDOG.LOAD` を再ロードして延長し、120 s を自己診断の締め切りにした。
- **温かい再起動で CYW43439 が状態を引き継ぐ**: FLASH_UPDATE 再起動後、join は通るのに DHCP が一度も通らなかった。
  起動時に WL_REG_ON を 500 ms 落としてコールドスタートさせ、再起動前にも `leave()` + 電源断をする。
- **DHCP タイムアウトでは再 join する**: 20 s で IP が取れなければ AP から離脱してやり直す。
- **表示開始位置は名目値より 2 ワード右**: HSYNC/VSYNC 用の SM1 は有効化直後に 2 命令進んだ状態で止まるので、
  多チャネル同時トリガ後は SM0 (画素) より 2 NCLK 先行し、LCD の最初の表示画素はフレーム行の x=106 になる
  (名目 108、旧値 98 では左端 1 文字が欠けた)。v0.2.7 の目盛り表示で実機確認し、v0.2.8 から正式値。

## 既知の制限・今後

- **TLS はサーバ証明書を検証していない** (`TlsVerify::None`)。reqwless 0.14 / embedded-tls 0.18 の検証器がワイルドカード
  証明書を扱えないため。経路上の攻撃者が任意のファームウェアを配れるので、**信頼できる LAN でだけ使うこと**。
  第 3 段階として manifest への Ed25519 署名 (公開鍵をファームウェアに埋め込み) を予定
  ([wifi-ota.md §6](docs/wifi-ota.md))。
- OTA で配れるのは manifest が指す 1 つの bin (v0.3.0〜 `ticker`)。他の bin は USB (picotool / D&D) で書く。
- 実機確認は 1 台のみ。複数台・長期運用・フラッシュ書き込み中の cyw43 の挙動などは未確認
  ([wifi-ota.md §8](docs/wifi-ota.md))。
- パーティションテーブル自体の更新、Wi-Fi ファームウェア (cyw43、約 231 kB) の分離配布は扱っていない。
