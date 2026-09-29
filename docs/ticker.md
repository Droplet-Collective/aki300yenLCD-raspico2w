# ネットワーク・ティッカー (`ticker`, v0.3.0〜、現行 v0.3.1)

Pico 2 W + 300 円 LCD を「時計 + 天気 + 流れる文字」の小さな情報端末にする bin。
`wifi_ota` (v0.2.x) の後継で、**Release の OTA イメージはこれ** (`manifest.json` の `bin` が `ticker.bin`)。
OTA / TBYB の仕組みは [wifi-ota.md](wifi-ota.md) と同じ (実装は `src/ota/app.rs` に共通化した)。

## 1. 画面 (400×96)

```text
 y                                                                     色
 2〜29  21:53:44  (5×7 ドット ×4)     2026/09/29 (火)                   時計 白 / 日付 水色
18〜33                                東京 19.1℃ 晴れ時々くもり          地名 橙 / 気温 橙 / 天気 白
34〜49  最高 21.9℃  最低 19.1℃  降水確率 100%                          赤 / 青 / 水色
50      ──────────── 区切り線 (暗灰) ────────────
51〜66  ← こんにちは、おちょこさん。ネットワーク・ティッカー 0.3.1 が …  流れる文字 (黄白、右→左)
67      ──────────── 区切り線 (暗灰) ────────────
68      MySSID 192.168.1.23 | NTP ok s1 | WX ok | MSG ok                 状態行 1 (接続中は水色)
77      ticker v0.3.1 via OTA slot B TBYB:bought OK                      状態行 2 = wifi_ota の行 1
86      OTA: up to date (latest 0.3.1), next check in 45s                状態行 3 = wifi_ota の行 2
```

- 外周 1 px の暗い枠は `wifi_ota` と同じ (四辺が見えれば 400×96 全体が表示されている)。
- 日本語は東雲フォント 14 ドット (全角 14×14、半角 7×14) を**等倍**で、16 px の帯の中央 (上下 1 px 余白) に描く (§5)。
  時計の数字は 5×7 ドットの専用フォントを 4 倍 (20×28)。状態行 3 本は `FONT_6X10` (66 桁) を 9 px ピッチで
  (大文字は行 1〜7、下に伸びる字は行 8〜9、次の行の行 0 は常に空なので重ならない)。
- 流れる文字の帯 (y 51〜66) の区切り線 (y 50 / 67) と文字 (y 52〜65) の間、区切り線と状態行 1 の文字 (y 69〜) の
  間はそれぞれ 1 px 空く。v0.3.0 では下の区切り線 (y 67) が状態行 1 (y 66〜) の大文字の上端に重なっていた。
- 時刻同期前は時計が `--:--:--` (灰) + `時刻同期中…`、天気取得前は `天気取得中…`。
- 状態行 1 は起動から 60 s は**起動診断** (前回の TBYB 起動の記録 `TBYB 0.3.0: dhcp-wait @121.3s ...`、
  無ければ `FLASH_UPDATE P1 A:... reset:wdt` の起動種別。電源投入直後の通常起動では出ない)、
  次いで 90 s まで `ticker.txt` の注意 (無い / 読めない)、その後は
  `<SSID> <IP> | NTP <状態> | WX <状態> | MSG <状態>`。
  - `NTP ok s1` = 同期済み (stratum 1)、`syncing`、`DNS failed` / `timeout` / `bad reply`
  - `WX ok` / `ok(http)` (TLS が通らず平文 HTTP に切り替えた) / `fetching` / `HTTP 429` / `bad JSON` / `too long`
  - `MSG ok` / `ok(cut)` (512 B で切った) / `fetching` / `HTTP 404` / ...
- 状態行 2・3 は `wifi_ota` の行 1・2 と同じ文言・色 (`src/ota/app.rs` が作る)。**OTA の診断はこの 2 行で行う**。
  ダウンロード中は行 3 の上に進捗バーが出る。

## 2. データ源と更新周期

| 項目 | 取得先 | 方式 | 周期 | 失敗時 |
|---|---|---|---|---|
| 時刻 | `ntp.nict.jp` → 予備 `pool.ntp.org` (UDP 123) | SNTP v4 (RFC 4330、48 B、自前実装 `src/ticker/sntp*.rs`) | 6 時間 | 30 s → 最大 10 min |
| 天気 | `api.open-meteo.com/v1/forecast` (API キー不要) | HTTPS (TLS 1.3 / AES-128-GCM / RSA-PSS、GitHub と同じ設定)。TLS 失敗時は次回から `http://` (Open-Meteo は平文も受ける) | 30 分 | 2 分 |
| 流れる文字 | `ticker.txt` の `message_url` (既定: このリポジトリの `ticker/message.txt`、raw.githubusercontent.com) | HTTPS (200 直返し、リダイレクト無し) | 5 分 | 1 分 |
| OTA | GitHub Release の `manifest.json` → `ticker.bin` | HTTPS (wifi_ota と同じ) | 60 秒 | 60 s → 最大 10 min |

- 時刻は同期時の UNIX ミリ秒 + embassy の単調時計 (`Instant`) で進める。表示は固定オフセット
  (`ticker.txt` の `tz`、既定 +9 = JST)。夏時間は扱わない。
- 天気の要求は `current=temperature_2m,weather_code&daily=temperature_2m_max,temperature_2m_min,
  precipitation_probability_max&timezone=auto&forecast_days=1` (本文 ≈ 620 B、chunked)。
  `serde-json-core` で `current` / `daily` だけ読む。WMO 天気コードは `src/ticker/weather.rs` で
  日本語にする (0 快晴、1 晴れ、2 晴れ時々くもり、3 くもり、45/48 霧、51〜55 霧雨、61/63/65 小雨/雨/大雨、
  71/73/75 小雪/雪/大雪、80〜82 にわか雨、95 雷雨、96/99 雷雨とひょう …)。
  Open-Meteo の無料枠は 1 日 10,000 要求 / IP なので 30 分周期 (48 回/日) は十分余裕がある。
- 流れる文字は UTF-8 テキストをそのまま 1 行に流す (改行は空白に)。512 バイト (日本語 ≈ 170 文字) で切る。
  内容が変わったときだけ右端から流し直す。**`ticker/message.txt` を main で書き換えれば、5 分以内に
  全機体の表示が変わる。**
- 取得は 1 本の TLS バッファ (16 kB + 3 kB) を共用するため、main ループ (250 ms 周期) が
  OTA → NTP → 天気 → 文字 の優先順で**一度に 1 つずつ**行う。OTA のダウンロード中 (数十秒〜数分) は
  他の取得は待つ。

## 3. TBYB と OTA

- 自己診断 (buy 条件) は `wifi_ota` と同じ「LCD 走査中 + Wi-Fi join + DHCP で IP 取得」だけ。
  **NTP / 天気 / 文字の取得が失敗しても buy には関係しない** (それらは buy が済むまで始めない。取得の待ち時間で
  buy を遅らせないため)。締め切り 120 s、ウォッチドッグ延長 2 s ごと、巻き戻り時の記録も同じ。
- OTA イメージの切り替え: v0.2.x の `wifi_ota` は manifest の `bin` に書かれた名前をそのまま
  `releases/latest/download/<bin>` から取る (`src/ota/app.rs` の `latest_asset_url(&manifest.bin)`)。
  v0.3.0 の Release では `manifest.json` が `ticker.bin` を指すので、**0.2.8 の `wifi_ota` が動いている機体は
  そのまま `ticker` 0.3.0 をダウンロードして切り替わる**。以後の版も `ticker.bin` で配る。
  `wifi_ota.bin` / `wifi_ota.uf2` (TBYB 付き) も Release には引き続き付けるが、manifest からは参照しない。
- `ticker` から `wifi_ota` に戻したいときは USB (`wifi_ota-plain.uf2`) で書くか、manifest の `bin` を
  `wifi_ota.bin` にした Release を出す (版数は上げる必要がある)。

## 4. 設定: SD カードの `ticker.txt` (任意)

`WIFI.TXT` と同じく microSD のルートに置く (8.3 名 `TICKER.TXT`、UTF-8、BOM 可)。無ければ東京の既定値で動き、
起動後しばらく状態行 1 に `ticker.txt not found, using Tokyo (35.6812,139.7671 UTC+9)` と出る。

```ini
# 行頭 # はコメント。key=value、キーの大文字小文字は区別しない。値の後ろの # 以降も無視
lat=35.6812          # 緯度 (-90〜90)
lon=139.7671         # 経度 (-180〜180)
tz=+9                # UTC からの時差 (時間)。9 / -5.5 / +05:30 / 9:00 の形も可。±14 まで
place=東京           # 天気の行の先頭に出す地名 (最大 32 バイト、東雲フォントにある文字)
message_url=https://raw.githubusercontent.com/Droplet-Collective/aki300yenLCD-raspico2w/main/ticker/message.txt
scroll=1             # 流れる文字の速さ (px / フレーム、1〜8。60 Hz なので 1 = 60 px/s)
```

| キー | 既定値 | 備考 |
|---|---|---|
| `lat` / `lon` | 35.6812 / 139.7671 (東京駅) | Open-Meteo に小数 4 桁で渡す |
| `tz` | +9 | 表示だけに使う。天気の「今日」は Open-Meteo が座標から決める (`timezone=auto`) |
| `place` | 東京 | 空なら既定のまま |
| `message_url` | 上記 | `http://` / `https://` で始まること。最大 256 バイト |
| `scroll` | 1 | |

不正な値のキーは無視して既定値のまま。有効なキーが 1 つも無ければ `ticker.txt: no valid keys, ...` と出る。

## 5. 日本語フォントとライセンス

- /efont/ プロジェクトの**東雲フォント** (Shinonome) 0.9.11、ゴシック体 14 ドット (`shnmk14` JIS X 0208 全角 14×14 +
  `shnm7x14r` JIS X 0201 半角 7×14) を `tools/bdf2bin.py` で 3 つのテーブル (`fonts/shinonome/codes.bin` 14,094 B +
  `glyphs.bin` 197,316 B + `widths.bin` 7,047 B、計 218,457 B、7,047 グリフ = JIS 第一・第二水準 + かな + 記号 +
  半角 ASCII / 半角カナ) に変換し、フラッシュに埋め込む (`src/font/shinonome.rs`、Unicode の二分探索、等倍描画)。
  取得元は Debian / Ubuntu の `xfonts-shinonome` パッケージ (PCF → `pcf2bdf`)。
- ライセンスは **Public Domain** (作者が権利を行使しないと宣言。改造・変換・組込み・再配布自由、無保証)。
  原文は [fonts/shinonome/LICENSE](../fonts/shinonome/LICENSE)、由来は [fonts/shinonome/README.md](../fonts/shinonome/README.md)。
- **v0.3.1 で美咲フォント (8×8) の 2 倍表示から置き換えた理由**: 実機の写真で、2 倍にした線 (2 px) が
  隣の線との隙間 (2 px) を LCD の画素のにじみで埋めてしまい、「太すぎて潰れて」見えた。ASCII の `FONT_6X10` や
  時計の数字 (5×7 ×4 は線が 4 px だが隙間も 4 px) は問題なかったので、線 1 px・隙間 1 px 以上の等倍 14 ドット
  フォントに変えた。文字の大きさ (全角 14 px) は美咲 ×2 の字面 (7×7 ×2 = 14 px) と同じで、レイアウトは変えていない。
  16 ドットの東雲 (`shnmk16`) も検討したが、字面が 16 行を使い切るため 16 px の帯に余白が取れず、区切り線や
  隣の行と接してしまうので 14 ドットにした。
- 収録外の文字 (絵文字など) は `□` で描く。半角 (ASCII、半角カナ) は 7 px 幅、全角は 14 px 幅。
  `～` (U+FF5E) と `〜` (U+301C)、`－` (U+FF0D) と `−` (U+2212)、`¥` と `\` は同じグリフ。

## 6. 構成

```text
src/bin/ticker.rs        main (SD → LCD → USB → CYW43 → 250 ms ループ: 接続 / TBYB / OTA / NTP / 天気 / 文字)
                         render_task (垂直同期ごとに全画面を描き直し、文字を scroll px 動かす)
                         共有モデル MODEL (ThreadModeRawMutex + RefCell。main が文字列を入れ、render が読む)
src/ota/app.rs           OTA + TBYB + 接続管理 (wifi_ota から移動。wifi_ota と共用)
src/ticker/civil.rs      UNIX 秒 → 年月日 / 曜日 / 時分秒 (Hinnant の civil_from_days)
src/ticker/config.rs     ticker.txt
src/ticker/weather.rs    Open-Meteo の URL / JSON / 天気コード
src/ticker/sntp.rs       SNTP パケット (純粋)、sntp_net.rs: embassy-net での問い合わせ
src/ticker/digits.rs     時計の 5×7 数字
src/font/shinonome.rs    東雲フォント (14 ドット) の検索と描画
tools/bdf2bin.py         BDF → テーブル (JIS X 0208 / JIS X 0201 / Unicode 符号の BDF に対応)
tools/ticker-tests/      上の純粋なモジュールをホストでテストする (cd tools/ticker-tests && cargo test)
ticker/message.txt       流れる文字の既定の取得元
```

描画は毎フレーム (≈16.6 ms) 全画面をバックバッファへ描き直して `present()` (垂直ブランキングでフロントへ
コピー) するので、スクロールにティアリングは出ない。1 フレームの描画は 1 ms 程度 (38,400 ワードのクリア +
数十グリフ)。フラッシュ書き込み中 (OTA のダウンロード中、1 セクタごとに 45〜400 ms 割り込み禁止) は描画が
止まり文字が一瞬引っかかるが、走査 (SRAM の DMA リング) は乱れない。

## 7. RAM / フラッシュ (v0.3.1、`--features tbyb`、`llvm-size`)

| bin | `.text` + `.rodata` | `.bss` | スタック (512 kB − bss) |
|---|---|---|---|
| `ticker` 0.3.1 | 677,772 + 515,768 = 1,193,540 B ≈ **1,166 kB** (1 スロット 1920 kB の 61 %) | 483,056 B | ≈ 40.3 kB |
| `ticker` 0.3.0 | 1,055,700 B ≈ 1,031 kB (54 %) | 484,080 B | ≈ 39.3 kB |
| `wifi_ota` | 911,996 B ≈ 891 kB | 483,296 B | ≈ 40.0 kB |

0.3.1 の増分はフォント (東雲 14 ドット 218,457 B − 美咲 78,881 B = +139,576 B)。
`ticker` 0.3.0 の増分: フラッシュ +144 kB (美咲フォント 79 kB、`f32` の書式化、天気 / SNTP / JSON)、RAM +784 B
(SNTP の UDP バッファ 4 × 128 B + メタデータ、天気 / 文字の本文 1,536 B、共有モデル ≈ 1 kB、レンダ・タスク。
TLS / HTTP / セクタバッファは wifi_ota と同じものを共用)。LCD のフロント 230 kB + バック 154 kB が大半。

## 8. 既知の制限

- TLS はサーバ証明書を検証しない (`TlsVerify::None`、wifi_ota と同じ。[wifi-ota.md §6](wifi-ota.md))。
  天気 / 文字も同様なので、経路上で内容を書き換えられる (表示が変わるだけで、ファームウェアの検証は別)。
- 時計は SNTP 同期 (6 時間ごと) の間 RP2350 の内蔵クロックで進む (数 ppm〜数十 ppm、6 時間で最大 1 秒程度)。
  うるう秒・夏時間は扱わない。
- Open-Meteo の応答が 1,536 B を超える (項目を増やした) 場合は `too long`。無料枠 (1 日 10,000 / IP) を
  超えると `HTTP 429`。
- 流れる文字は 512 バイトまで、1 行のみ。色や複数行の指定は無い。
- 天気の地名 (`place`) と文字は東雲フォントにある文字だけ (JIS X 0208 第一・第二水準 + JIS X 0201。絵文字は `□`)。
- ウィジェットの配置は固定 (400×96 前提)。
- 実機確認: v0.3.0 は 0.2.8 からの OTA で実機に載り、NTP / 天気 / 文字の取得と表示を確認した (2026-09-29)。
  v0.3.1 のフォントは実機の写真で確認する。

## 9. 履歴

| 版 | 内容 |
|---|---|
| 0.3.0 | 初版。`wifi_ota` の OTA / TBYB を `ota::app` に共通化し、NTP 時計 + Open-Meteo 天気 + `message.txt` の流れる文字 + 美咲フォントを追加。Release の OTA イメージを `ticker.bin` に切り替え |
| 0.3.1 | 日本語フォントを美咲 8×8 の 2 倍表示から東雲 14 ドットの等倍に変更 (実機で線が太く潰れて見えたため。§5)。流れる文字の下の区切り線が状態行 1 の文字に重なっていたのを直し、状態行を 9 px ピッチ (y 68 / 77 / 86) に、時計・日付・天気を 1 px 上に |
