# ネットワーク・ティッカー (`ticker`, v0.3.0〜、現行 v0.4.0)

Pico 2 W + 300 円 LCD を「時計 + 天気 + 流れる文字」の小さな情報端末にする bin。
v0.4.0 から SD の写真 (BMP) をスライドショーで背景に敷き、その上に半透明の「ガラス」の板で情報を重ねる。
`wifi_ota` (v0.2.x) の後継で、**Release の OTA イメージはこれ** (`manifest.json` の `bin` が `ticker.bin`)。
OTA / TBYB の仕組みは [wifi-ota.md](wifi-ota.md) と同じ (実装は `src/ota/app.rs` に共通化した)。

## 1. 画面 (400×96、v0.4.0〜)

描画はハードウェアに依存しない `src/ui/` が行う。**同じコードを PC のシミュレータ `tools/ui-sim` で動かして、
書き込む前に PNG / GIF で確かめられる** ([ui-sim.md](ui-sim.md))。既定の構成 `layout=glass`:

```text
 x 8〜        y 4〜35   21:53:44   時計 (DejaVu Sans の 4 bit アンチエイリアス数字 32 px、秒は 15 px)。板なし、右下 1 px の影
              y 40〜53  9月29日(火) 日付 (東雲 14 px、影付き)
              (左上 230×70 は背景に薄い暗がりを焼き込み、明るい写真でも読めるようにする)
 x 234〜396   y 4〜57   天気のガラス板 (黒 62 % + 上辺 1 px の明るい線 + 角丸 4)
                          行 1: 天気アイコン 15×15 (晴れ / 月 / 晴れ時々くもり / くもり / 霧 / 霧雨 / 雨 / 雪 / 雷)
                                + 気温 (橙の AA 数字 19.1°)、右端に地名 (⌖東京)
                          行 2: 天気 (晴れ時々くもり、東雲 14 px)
                          行 3: ▲最高 (赤) ▼最低 (青) + 降水確率の錠剤 (💧40%、水色)
 x 4〜396     y 74〜92  流れる文字の帯 (ガラス、東雲 14 px、右→左) | 右端 118 px に小さな状態:
                          Wi-Fi の扇 + NTP WX MSG (緑 = 良好 / 黄 = 取得中 / 赤 = 失敗 / 灰 = 未取得) + v0.4.0
```

- 日本語は東雲フォント 14 ドットの等倍 (§5)。線はすべて 1 px (2 px の線は LCD のにじみで潰れるため)。
  時計・気温の数字だけは大きさが要るのでアンチエイリアスの数字フォント ([fonts/dejavu/README.md](../fonts/dejavu/README.md)) にした。
- 外周 1 px の枠は写真を全面に出すため `glass` / `dock` では描かない (表示位置 106 は v0.2.8 で確定済み。
  `classic` と `wifi_ota` には残っている)。
- 時刻同期前は `--:--` (灰) + `時刻同期中…`、天気取得前は `?` + `天気取得中…`。
- 18〜6 時は晴れ / 晴れ時々くもりのアイコンが月になる。
- `ticker.txt` の `layout=` で構成を選べる (シミュレータで比べた 3 案。v0.4.0 の既定は `glass`):

| `layout=` | 内容 |
|---|---|
| `glass` (既定) | 上記。写真の中央 (x 160〜230) と帯の上 (y 58〜73) が見え、情報は左上 (時計) / 右上 (天気) / 下 (文字) にまとまる |
| `dock` | 上 52 px は写真と時計 + 日付だけ、下 40 px のガラスの台に天気と流れる文字。写真が一番よく見えるが、天気が 1 行に詰まる |
| `classic` | 0.3.1 と同じ配置 (状態 3 行を常に表示) で、写真を全面 62 % 暗くして敷く。比較用 |

### 1.1 状態表示 (3 行 ⇔ 小さな 1 行)

0.3.1 の下 3 行 (`SSID IP | NTP | WX | MSG` / `ticker v0.4.0 via OTA slot B TBYB:...` / `OTA: ...`、`FONT_6X10`、9 px ピッチ) は
OTA の診断に欠かせないが画面の 3 分の 1 を使うので、**必要なときだけ** 下の帯の位置 (y 61〜95 のガラス板) に出す。
それ以外は帯の右端の小さな表示だけになる (流れる文字は 3 行の間は隠れる)。3 行を出す条件 (どれか 1 つ):

1. 起動から 60 s
2. 失敗から 30 s (NTP / 天気 / 文字の取得失敗、Wi-Fi の join 失敗、OTA の失敗、背景の写真が読めない)
3. 起動診断 (前回の TBYB の記録) / `ticker.txt` の注意を出している間 (60 s / 90 s)
4. Wi-Fi が IP を得ていない間
5. OTA のダウンロード中 / 検証中 / 再起動待ち (進捗バー付き) と、OTA の失敗・巻き戻り (`OTA:` 行が赤)
6. TBYB の buy 待ち (`TBYB:pending`) / buy 失敗 / 締め切り切れ

`ticker.txt` の `status=full` で常に 3 行、`status=compact` で常に小さな 1 行 (既定 `auto` は上の規則)。
3 行の文言は 0.3.1 と同じ (下記)。

- 行 1: 起動診断 (前回の TBYB 起動の記録 `TBYB 0.3.0: dhcp-wait @121.3s ...`、無ければ `FLASH_UPDATE P1 A:... reset:wdt`
  の起動種別。電源投入直後の通常起動では出ない) → `ticker.txt` の注意 / 背景の写真の失敗 (`BG IMAGE.BMP: BMP: 24/32 bit only`) →
  `<SSID> <IP> | NTP <状態> | WX <状態> | MSG <状態>`
  - `NTP ok s1` = 同期済み (stratum 1)、`syncing`、`DNS failed` / `timeout` / `bad reply`
  - `WX ok` / `ok(http)` (TLS が通らず平文 HTTP に切り替えた) / `fetching` / `HTTP 429` / `bad JSON` / `too long`
  - `MSG ok` / `ok(cut)` (512 B で切った) / `fetching` / `HTTP 404` / ...
- 行 2・3 は `wifi_ota` の行 1・2 と同じ文言・色 (`src/ota/app.rs` が作る)。**OTA の診断はこの 2 行で行う**。
  ダウンロード中は行 3 の上に進捗バーが出る。

### 1.2 背景の写真 (スライドショー)

- SD のルートの `*.BMP` (8.3 名、名前順、最大 16 枚。`_` で始まる名前 = macOS の `._IMAGE.BMP` などは除く) を
  `slide=` 秒 (既定 30) ごとに切り替える。`images=A.BMP,B.BMP` で使う写真と順番を指定できる。
  1 枚だけ、または `slide=0` なら最初の 1 枚を出したまま。BMP が無ければ既定のグラデーション (紺 → 青紫)。
- 形式: **非圧縮の 24 bit / 32 bit BMP** (下から上 / 上から下のどちらも、32 bit は BI_RGB と標準マスクの
  BI_BITFIELDS)。8 bit (パレット) / 16 bit / RLE 圧縮 / PNG / JPEG は不可 (読めなければ状態行 1 に理由を出して次へ)。
- 大きさは任意 (最大 8192×8192)。**縦横比を保って 400×96 を覆う最小の倍率に拡大縮小し、はみ出た部分の中央を切り出す**
  (縮小は面積平均、拡大は最近傍)。ただし SD は遅い (下記) ので、**400×96 の 24 bit BMP (115 kB)** を勧める
  (`convert_image_to_bmp.py` で作れる。`sd_bmp_viewer` 用の `IMAGE.BMP` / `IMAGE2.BMP` はそのまま使える)。
- 背景用の RAM は 1 枚分 (RGB565 76.8 kB) しか無いので、切り替えは「背景だけを 0.5 s で暗くする → 暗いまま
  新しい写真を読みながら行を上書き (下から上の BMP なら下の行から現れる) → 0.7 s で明るく戻す」。
  時計 / 天気 / 流れる文字は通常の明るさのまま動き続ける。8 bit の色は Bayer 4×4 のディザで RGB565 にする。
- 読み込みの速さ: SD は GPIO のビットバング SPI。起動時 (wifi.txt / ticker.txt) は従来どおり ≈200 kHz、
  写真を読むときだけ ≈1.5〜2 MHz に上げる (`sdfast=1`、既定)。400×96 の BMP で ≈1〜2 s の見込み。
  読み誤り (CRC) が出たら自動で低速に戻して読み直す (低速だと ≈4〜5 s、その間は描画が 30 fps に落ちる)。
  1 回に読むのは 512 バイト (SD の 1 ブロック) までで、描画 1 フレームの合間に 5 ms まで読むので、読み込み中も
  時計と流れる文字は止まらない。
- OTA の確認 / ダウンロード / 検証の間は切り替えを始めず、読み込み中なら止まって待つ (フラッシュ書き込みと重ねない)。

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
slide=30             # 写真の切り替え間隔 (秒、5〜3600。0 = 切り替えない)          v0.4.0〜
images=IMAGE.BMP,IMAGE2.BMP   # 背景に使う BMP と順番 (省略時はルートの *.BMP 全部、名前順)
layout=glass         # 画面構成 glass / dock / classic (§1)
status=auto          # 状態 3 行 auto (必要なときだけ、§1.1) / full (常に) / compact (常に小さな 1 行)
sdfast=1             # 写真を読むときの SD の速さ 1 = 速い (読み誤りで自動的に低速へ) / 0 = 常に低速
```

| キー | 既定値 | 備考 |
|---|---|---|
| `lat` / `lon` | 35.6812 / 139.7671 (東京駅) | Open-Meteo に小数 4 桁で渡す |
| `tz` | +9 | 表示だけに使う。天気の「今日」は Open-Meteo が座標から決める (`timezone=auto`) |
| `place` | 東京 | 空なら既定のまま |
| `message_url` | 上記 | `http://` / `https://` で始まること。最大 256 バイト |
| `scroll` | 1 | |
| `slide` | 30 | 0 なら最初の 1 枚のまま (v0.4.0〜) |
| `images` | (ルートの *.BMP) | カンマ区切り、8.3 名、最大 16。正しい名前が 1 つも無ければ無視 |
| `layout` | `glass` | `glass` / `dock` / `classic` |
| `status` | `auto` | `auto` / `full` / `compact` |
| `sdfast` | 1 | `0` / `1` |

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
                         render_task (垂直同期ごとに全画面を描き直し、文字を scroll px 動かす。状態 3 行の規則 §1.1)
                         共有モデル MODEL (ThreadModeRawMutex + RefCell。main が文字列を入れ、render が読む)
src/ticker/slideshow.rs  背景の写真 (SD の BMP を 1 ブロックずつ読む、フェード、OTA 中は停止。§1.2)
src/ui/                  画面の描画 (no_std の純粋なコード。tools/ui-sim と共用): screen.rs (3 つの構成)、
                         canvas.rs (ガラス板 / 文字 / アイコン)、bmp.rs (BMP → 400×96、拡大縮小 + 切り出し + ディザ)、
                         color.rs (RGB565 / 666 / 888、合成)、aafont*.rs (AA 数字)、icons.rs、background.rs、slide.rs
tools/ui-sim/            画面シミュレータ (PNG / GIF、docs/ui-sim.md)
src/ota/app.rs           OTA + TBYB + 接続管理 (wifi_ota から移動。wifi_ota と共用)
src/ticker/civil.rs      UNIX 秒 → 年月日 / 曜日 / 時分秒 (Hinnant の civil_from_days)
src/ticker/config.rs     ticker.txt
src/ticker/weather.rs    Open-Meteo の URL / JSON / 天気コード
src/ticker/sntp.rs       SNTP パケット (純粋)、sntp_net.rs: embassy-net での問い合わせ
src/ticker/digits.rs     5×7 の数字 (0.3.x の時計。v0.4.0 の画面では使わない、テストのみ)
src/font/shinonome.rs    東雲フォント (14 ドット) の検索と描画
tools/bdf2bin.py         BDF → テーブル (JIS X 0208 / JIS X 0201 / Unicode 符号の BDF に対応)
tools/ticker-tests/      上の純粋なモジュールをホストでテストする (cd tools/ticker-tests && cargo test)
ticker/message.txt       流れる文字の既定の取得元
```

描画は毎フレーム (≈16.6 ms) 全画面をバックバッファへ描き直して `present()` (垂直ブランキングでフロントへ
コピー) するので、スクロールにティアリングは出ない。v0.4.0 の 1 フレーム: 背景のコピー (76.8 kB の memcpy ≈0.2 ms)
+ ガラス板 2 枚の半透明合成 (≈16,000 画素 × ≈12 サイクル ≈1.3 ms) + AA 数字 / 文字 / アイコン (≈0.5 ms) の
見積り ≈2〜3 ms (150 MHz)、垂直ブランキングのコピー (RGB565 → 走査ワードの表引き、≈7 サイクル / 画素、≈2.4 ms) を
足して ≈5 ms / 16.6 ms。暗がりは写真を読んだときに 1 回だけ背景へ焼き込み、毎フレームは計算しない。
実測値は defmt のログ `render: max N us per frame` (30 s ごと) に出る。流れる文字は LCD のフレーム番号で進めるので、
描画が 1 フレーム遅れても速さは変わらない (2 px 飛ぶだけ)。フラッシュ書き込み中 (OTA のダウンロード中、1 セクタごとに 45〜400 ms 割り込み禁止) は描画が
止まり文字が一瞬引っかかるが、走査 (SRAM の DMA リング) は乱れない。

## 7. RAM / フラッシュ (v0.4.0、`--features tbyb`、`llvm-size`)

| bin | `.text` + `.rodata` | `.data` + `.bss` + `.uninit` | スタック (512 kB − 左) |
|---|---|---|---|
| `ticker` 0.4.0 | 720,720 + 528,348 = 1,249,068 B ≈ **1,220 kB** (1 スロット 1920 kB の 65 %) | 4,136 + 491,024 + 1,024 = 496,184 B | ≈ **27.4 kB** |
| `ticker` 0.3.1 | 677,772 + 515,768 = 1,193,540 B ≈ 1,166 kB (61 %) | 2,064 + 483,056 + 1,024 = 486,144 B | ≈ 37.3 kB |
| `wifi_ota` 0.4.0 | 629,404 + 280,680 = 910,084 B | 2,628 + 405,472 + 1,024 = 409,124 B | ≈ 112 kB |

**v0.4.0 の RAM の組み替え**: 写真の背景 (400×96) を持つには 1 枚分のバッファが要るが、0.3.1 の空きは ≈37 kB しか無く、
RGB666 ワード (`u32`) のままでは 153.6 kB、RGB565 でも 76.8 kB で入らない。そこで

- LCD のバックバッファ (`lcd::display::BackBuffer`) を **RGB666 の `u32` (153,600 B) から RGB565 の `u16` (76,800 B)** にし、
- 空いた 76.8 kB に背景 `ticker::slideshow::BG` (RGB565、76,800 B) を置いた。
- 垂直ブランキングのコピー (`present()`) は 2 つの 256 語の表 (上位バイト / 下位バイトの寄与、SRAM に 2 kB) の OR で
  RGB565 → 走査ワード (RGB666、ビット順反転込み) に広げる。1 行 ≈ 20〜25 µs で、走査の 1 行 147 µs より十分速いので、
  ブランキング中に始めれば 0.3 までの単純コピーと同じく走査に追い越されない。R/B は 5 bit になる (上位ビットの複製で 6 bit に
  広げる) が、写真は 8 bit からディザを掛けて RGB565 にするので縞は目立たない。フロントバッファ (DMA が走査する
  509×113 ワード、230 kB) と走査の仕組み (VISIBLE_X_OFFSET = 106 を含む) は変えていない。
- 他の bin (`wifi_ota` / `wifi_status` / `ota_selftest` / `sd_bmp_viewer`) も同じ `BackBuffer` を使うので RAM が 76.8 kB 空いた。
  見た目の違いは R/B の最下位ビットだけ。

増分 (ticker 0.3.1 → 0.4.0): RAM +10 kB (スライドショーのタスク 6.7 kB = 行の累積 3.2 kB + 512 B の読み込みバッファ + 一覧、
表 2 kB、他)。スタックの目安 ≈27 kB (TLS / HTTP のバッファは static で、0.3.1 と同じ)。フラッシュ +55 kB (描画コード、AA 数字 5.6 kB、BMP)。

## 8. 既知の制限

- TLS はサーバ証明書を検証しない (`TlsVerify::None`、wifi_ota と同じ。[wifi-ota.md §6](wifi-ota.md))。
  天気 / 文字も同様なので、経路上で内容を書き換えられる (表示が変わるだけで、ファームウェアの検証は別)。
- 時計は SNTP 同期 (6 時間ごと) の間 RP2350 の内蔵クロックで進む (数 ppm〜数十 ppm、6 時間で最大 1 秒程度)。
  うるう秒・夏時間は扱わない。
- Open-Meteo の応答が 1,536 B を超える (項目を増やした) 場合は `too long`。無料枠 (1 日 10,000 / IP) を
  超えると `HTTP 429`。
- 流れる文字は 512 バイトまで、1 行のみ。色や複数行の指定は無い。
- 天気の地名 (`place`) と文字は東雲フォントにある文字だけ (JIS X 0208 第一・第二水準 + JIS X 0201。絵文字は `□`)。
- ウィジェットの配置は 3 種類から選ぶだけ (400×96 前提)。
- 背景の写真は SD のルートだけ (サブディレクトリは見ない)、8.3 名、最大 16 枚、非圧縮 24 / 32 bit BMP のみ。
  大きな BMP は読み込みに時間がかかる (400×96 ならおよそ 1〜2 s、1920×1080 の BMP は数十秒)。
- 切り替えの途中で読めなかった写真は元に戻せないのでグラデーションになる (次の間隔で次の写真へ)。
- 実機確認: v0.3.0 は 0.2.8 からの OTA で実機に載り、NTP / 天気 / 文字の取得と表示を確認した (2026-09-29)。
  v0.3.1 のフォントは実機の写真で確認済み (「きれいに見えた」)。**v0.4.0 (写真の背景、RGB565 のバックバッファ、
  SD の速い読み出し、描画時間) は実機で未確認** (シミュレータでのみ確認)。

## 9. 履歴

| 版 | 内容 |
|---|---|
| 0.3.0 | 初版。`wifi_ota` の OTA / TBYB を `ota::app` に共通化し、NTP 時計 + Open-Meteo 天気 + `message.txt` の流れる文字 + 美咲フォントを追加。Release の OTA イメージを `ticker.bin` に切り替え |
| 0.3.1 | 日本語フォントを美咲 8×8 の 2 倍表示から東雲 14 ドットの等倍に変更 (実機で線が太く潰れて見えたため。§5)。流れる文字の下の区切り線が状態行 1 の文字に重なっていたのを直し、状態行を 9 px ピッチ (y 68 / 77 / 86) に、時計・日付・天気を 1 px 上に |
| 0.4.0 | **写真の背景 + モダンな画面**。SD の BMP のスライドショー (§1.2、`slide` / `images` / `sdfast`)、ガラスの板・AA 数字の時計・天気アイコン・降水確率の錠剤 (§1、`layout`)、状態 3 行を必要なときだけ出す (§1.1、`status`)。描画を `src/ui/` に分けて PC のシミュレータ `tools/ui-sim` と共用 ([ui-sim.md](ui-sim.md))。バックバッファを RGB565 にして背景用の RAM を作った (§7) |
