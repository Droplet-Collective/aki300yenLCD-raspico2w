# 設定ページ (`ticker` v0.5.0〜、現行 v0.5.1)

同じ LAN (同じ Wi-Fi) のスマートフォンや PC のブラウザから、ticker の設定を変えるページ。端末 (Pico 2 W) の中の
小さな HTTP サーバ (`src/web/`) が配る。変えられるのは `ticker.txt` の設定 (地域 / 表示 / 流れる文字 / 写真の並び)
と SD の写真 (追加 / 削除)。保存は SD の `ticker.txt` に書くので、手で書いた内容と画面の設定は 1 つのまま。

## 1. 開き方

1. 端末を起動する。最初の OTA 確認 (起動から 10〜20 s) が通ると、LCD の流れる文字の後ろに毎周
   `◆ 設定 http://192.168.x.y/ コード 123456 ◆` が流れる (0.5.1〜。0.5.0 は下の帯に 1 分だけ出していた)。
   状態 3 行を出している間 (起動 60 s など) は行 1 にも 1 分 `settings: http://... code ...`。
2. 同じ Wi-Fi につないだブラウザでその URL を開く (`http://` で。`https://` ではない)。
3. 設定を変えて「保存」を押す。初めて保存するときに LCD の 6 桁のコードを聞かれる。コードは起動するたびに変わる。
   `show_settings=0` (流れる文字に入れない) にしているときは、ページの「LCD にコードを表示」で 1 分だけ流れる文字に入れられる
   (設定の部分へ進めて 12 s 目立たせる)。

IP アドレスは状態 3 行の行 1 (`SSID 192.168.x.y | NTP ...`) にも出る (`status=full` なら常に)。ルーターの DHCP の
一覧でも探せる (ホスト名は付けていないので MAC アドレスで)。`ticker.local` (mDNS) は使えない (§7)。

LCD の見本 (ui-sim、`tools/ui-sim/scenarios/settings*.json`):

- ふだんの画面: 流れる文字の中の設定の部分 (`settings.json`)、つなぎ目を流れる GIF (`settings-scroll.json`)、
  「LCD にコードを表示」の直後 (`settings-highlight.json`)
- 状態 3 行の表示中 (起動 60 s、TBYB の buy 待ちなど): 行 1 が `settings: http://… code …` (`settings-status.json`)
- `layout=dock`: 下の台の 2 行目 (`settings-dock.json`)
- 流れる文字の組み立ての規則 (区切り、IP が無いとき、`show_settings=0`、長い文字の切り方) は [ticker.md §1.3](ticker.md)

## 2. できること

| 画面の欄 | `ticker.txt` のキー | 反映 |
|---|---|---|
| 地域: 都市の検索 (ブラウザが Open-Meteo の地名検索 API を直接呼ぶ。端末は通信しない)、地名、緯度、経度、UTC からの時差 | `place` `lat` `lon` `tz` | すぐ。緯度 / 経度が変わったら天気をすぐ取り直す |
| 表示: 画面構成 (Glass / Dock / Classic)、写真の切り替え間隔、状態 3 行の出し方、流れる文字の速さ | `layout` `slide` `status` `scroll` | すぐ。画面構成を変えたら写真を読み直す (写真の暗がりを構成ごとに焼き込むため) |
| 流れる文字: 「URL から取得」(既定、5 分ごと) か「この端末で決める」 | `message_url` / `message` | すぐ。`message=` があれば取得しない |
| 流れる文字に設定 URL とコードを入れる (0.5.1〜、既定 入れる) | `show_settings` (`1` / `0`) | すぐ (次のフレームで組み直す) |
| 写真: SD の BMP の一覧 (サムネイル)、並べ替え、使う / 使わない、削除、追加 | `images` | すぐ (スライドショーが一覧を作り直す) |
| 端末の状態 (読むだけ): 版、稼働時間、Wi-Fi (SSID / IP)、OTA の行、最後の確認、NTP / 天気 / 文字、スタックの最大使用量、前回のリセット理由、TBYB | — | 5 s ごとに更新 |
| ボタン: 今すぐ更新を確認 / 再起動 / LCD にコードを表示 | — | — |

- 再起動は **不要** (全部その場で反映する)。「再起動」ボタンは TBYB の buy 待ちの間は断る (buy していない版を再起動すると旧版に戻るため)。
- **流れる文字は `message=` (新しいキー) にした**: 「この端末で決める」を選ぶと `ticker.txt` に `message=文字` の行を書き、
  `message_url` からの取得を止める。URL に戻すと `message=` の行を消し、すぐ取得し直す。`message=` は **行末までそのまま**
  (他のキーと違って `#` をコメントにしない)。512 バイトまで、改行は空白になる。
  既定の `ticker/message.txt` (リポジトリ) を編集する運用もそのまま使える。
- 写真の追加: ブラウザで写真を選ぶ → 400×96 の枠でドラッグ (位置) と拡大 (スライダー / ホイール / 2 本指) で切り抜く
  (時計 / 天気 / 帯の位置を重ねて確かめられる) → ブラウザが 24 bit の BMP (下から上、BGR、115,254 バイト) にして送る →
  端末が 512 B ずつ SD に書く。名前は元のファイル名から 8.3 形式 (英数字 8 文字 + `.BMP`) を作り、同じ名前があれば
  `NAME~2.BMP`、それも無理なら `IMG00001.BMP`。SD の BMP は 16 枚まで (スライドショーの上限)。
- 写真の並びは `images=` に書く。全部を名前順に使うなら `images=` の行を消す (= ルートの *.BMP 全部、0.4.0 からの既定)。
- Wi-Fi のパスワードはどの API からも出ない (サーバは持っていない)。`debug_crash` / `sdfast` はページからは変えない (手で書く)。

## 3. API

すべて `http://<端末の IP>/`。応答は JSON (UTF-8)。エラーは `{"ok":false,"status":N,"error":"日本語の理由"}`。

| メソッド / パス | コード | 内容 |
|---|---|---|
| `GET /` | 不要 | 設定ページ (gzip、`Content-Encoding: gzip`、CSP 付き) |
| `GET /api/status` | 不要 | 端末の状態 (`version` `uptime_s` `ssid` `ip` `rssi` (常に null) `wifi` `ntp` `weather` `message_state` `ota` `ota_tone` `ident` `tbyb` `stack_used` `stack_total` `last_reset` `layout` `weather_now` `last_ota_check_s` `ota_checks` `pending`) |
| `GET /api/settings` | 不要 | 今の設定 (`place` `lat` `lon` `tz_offset_secs` `layout` `slide` `status` `scroll` `show_settings` (0.5.1〜) `message_url` `local_message` `message` `images` `sd`) |
| `GET /api/images` | 不要 | SD の BMP `{"files":[{"name","size"}],"order":[images= の名前],"max":16,"upload_size":115254}` |
| `GET /img/NAME.BMP` | 不要 | SD の BMP をそのまま (`image/bmp`)。サムネイルはブラウザが縮める |
| `POST /api/settings` | 要 | `application/x-www-form-urlencoded`。変えるキーだけ (`place` `lat` `lon` `tz` `layout` `slide` `status` `scroll` `message` `message_url` `images` `show_settings`)。`images=` / `message=` を空で送ると行を消す。値の規則は `ticker.txt` と同じ (`config::valid_value`)。4 kB まで |
| `POST /api/images/delete` | 要 | `name=NAME.BMP`。`images=` にあれば外す |
| `POST /api/upload?name=元の名前` | 要 | 本文は 400×96 の 24 bit BMP ちょうど 115,254 バイト (`application/octet-stream`)。応答 `{"ok":true,"name":"SD の名前"}` |
| `POST /api/reboot` | 要 | 再起動 (buy 待ちなら 409) |
| `POST /api/ota-check` | 要 | すぐに OTA を確認する (新しい版があれば入れて再起動) |
| `POST /api/auth` | 要 | コードを確かめるだけ |
| `POST /api/show-code` | 不要 | 流れる文字を設定の部分 (URL とコード) へ進めて 12 s 目立たせ、1 分は `show_settings=0` でも入れる (状態 3 行の行 1 にも 1 分)。10 s に 1 回まで |
| `OPTIONS *` | — | 405 (CORS の許可は返さない) |

curl の例: `curl -H 'X-Ticker-Code: 123456' -d 'layout=dock&slide=60' http://192.168.200.130/api/settings`

## 4. 安全のしくみ (と、守れないもの)

LAN の中だけで使う前提で、TLS は無い (平文の HTTP)。そのうえで、**よそのウェブサイトがブラウザを使って端末を
操作する (CSRF / DNS rebinding) こと** を防ぐ:

1. **アクセスコード**: 状態を変える要求 (POST) は `X-Ticker-Code` ヘッダに 6 桁のコードが要る。コードは起動ごとに乱数
   (RP2350 の ROSC) から作り、LCD にだけ出す (0.5.1〜 既定では流れる文字の中に常に。`show_settings=0` なら
   ページが頼んだときの 1 分と、状態 3 行の行 1 に起動後 1 分)。
2. **独自ヘッダ = CORS の事前確認**: `X-Ticker-Code` は「単純でない」ヘッダなので、よそのサイトのページが送るには
   ブラウザが先に `OPTIONS` で許可を聞く。サーバは許可を返さない (405) ので、ブラウザは本当の要求を送らない。
3. **`Host` の確認** (GET も): `Host` はこの端末の IP アドレス (`a.b.c.d` / `a.b.c.d:80`) だけを受ける。DNS rebinding
   (攻撃者の名前を 192.168.x.y に向ける) で来た要求は `Host` が違うので 403。
4. **`Origin` / `Sec-Fetch-Site` の確認** (POST): あれば `http://<Host>` / `same-origin` (または `none`) と一致すること。
5. **総当たりの抑止**: コードを 5 回続けて間違えると 30 s 受け付けない (締め出し中は正しいコードも断る)。次からは
   60 s、120 s … と倍、上限 15 分。1 日に試せるのは 1,000 回未満 (6 桁 = 90 万通り、`tools/ticker-tests` で確認)。
6. 応答には `Cache-Control: no-store`、`X-Frame-Options: DENY` (クリックジャッキング)、`X-Content-Type-Options: nosniff`、
   ページには `Content-Security-Policy` (スクリプトはページの中だけ、通信は端末と Open-Meteo の地名検索だけ)。
7. 要求は 1 本ずつ、ヘッダ 2 kB、本文 4 kB (写真はちょうど 115,254 B)、読み書き 5 s、要求全体 15 s (写真 60 s) で打ち切る。

守れないもの:

- **同じ LAN の中の盗聴 / なりすまし**: 平文なので、同じ Wi-Fi で通信を見られる人はコードも設定も読める。コードを
  知った人 / LCD を見られる人は設定を変えられる。LAN の中の人を信用できない場所では使わない (または SD の抜き差しで設定する)。
- **LCD を見られる人 (0.5.1〜 の既定)**: `show_settings=1` (既定) ではコードが流れる文字にずっと出ているので、**LCD を
  見られる人なら誰でも、同じ Wi-Fi から設定を変えられる** (写真の削除 / 追加、再起動も)。0.5.0 でも起動後 1 分の案内の間は
  同じだった (出ている時間が長くなっただけで、守りの仕組みは変わらない)。来客や人通りから LCD が見える場所では、設定ページの
  「流れる文字に設定 URL とコードを入れる」を外す (`show_settings=0`)。ページにも同じ注意を書いた。
- **読み取り**: GET (状態 / 設定 / 写真) はコード無しで読める (同じ LAN の中の人だけ。よそのサイトからは `Host` の確認で読めない)。
  Wi-Fi のパスワードはどこにも出ない。
- **締め出しを使った妨害**: 間違ったコードを送り続ければ、正しい利用者も締め出される (数分〜15 分)。再起動で解ける。
- 端末の OTA / 天気 / 文字の取得の TLS がサーバ証明書を検証しない点 ([wifi-ota.md §6](wifi-ota.md)) は変わらない。

## 5. ticker.txt の書き方 (壊さないために)

embedded-sdmmc 0.10 には名前の変更 (rename) が無いので、次の順で書く (`web::server::save_ticker_txt`)。

1. 今の `TICKER.TXT` を読む (無ければ見出しのコメント 1 行から始める)。1,536 バイト (`config::CONFIG_MAX`、0.5.0 で 512 から拡大) まで。
2. `config::rewrite` で変えるキーの行だけを書き換える。**知らないキー、コメント、空行、行の順番、キーの書き方 (`LAT=` など)、
   値の後ろのコメント、改行 (CRLF / LF)、先頭の BOM はそのまま**。同じキーの行が複数あればすべて (読むときは最後の行が勝つので)。
   ファイルに無いキーは末尾に足し、空で送ったキー (`images` / `message`) は行を消す。
3. `TICKER.NEW` に書いて読み戻し、一致を確かめる (ここで失敗しても `TICKER.TXT` はそのまま)。
4. 元の内容を `TICKER.BAK` に書いて読み戻す。
5. `TICKER.TXT` を書き直して読み戻す。一致したら `TICKER.NEW` を消す。失敗したら `TICKER.NEW` を残してエラーを返す
   (起動時、`TICKER.TXT` が無い / 空なら `TICKER.NEW` を読む)。
6. 書いた内容を読み直して (`TickerConfig::parse`) その場で反映する。

SD の 1 回の操作 (ボリュームを開く、1 ブロックの読み書き、閉じる) には 2 s の期限 (`sdcard::with_deadline`)、
処理全体にも期限を付け、過ぎたら SD の転送を失敗させて必ず戻る (SD は GPIO SPI の同期処理で、その間は描画も止まるため)。
SD はスライドショーと `slideshow::SD_LOCK` で分け合い、サーバが SD を使った後 5 s はスライドショーが次の写真を読み始めない
(サムネイルを続けて読む間)。取れなければ 6 s で 503。**SD カードの抜き差しは電源を切ってから** (動いている間の抜き差しは想定していない)。

## 6. OTA 到達保証との関係 ([ticker.md §8](ticker.md)、[SKILL.md](../.claude/skills/ota-firmware/SKILL.md))

- サーバは **取得タスク (`jobs_task`) の中で 1 要求ずつ** 動く。取得タスクは毎周まず OTA 確認を見るので、OTA 確認と
  ダウンロードの間はサーバは動かない (要求はソケットで待たされる。ページは「応答なし (更新中かもしれません)」と出す)。
  順番は OTA 確認 > 設定ページの要求 > NTP > 天気 > 文字。専用のタスクにしなかったのは RAM のため: HTTPS のバッファ
  (`NetBuffers`、≈ 30 kB) を作業領域として借りられ (OTA / 取得とは同時に動かない)、タスク領域も増えない。
- 待ち受けは **最初の OTA 確認が通って (`OTA_PROVED`) から** 始める。**回復モードでは作らない**。
- TBYB の buy 条件の一巡 (`boot_policy::Round`) に **`web` (待ち受けを始めた)** を足した。LCD の `wait:web`。
- 生存確認 **`Who::Web`** (上限 20 s、`health::Limits::TICKER`): 要求を処理している間だけ監視し (読み書き / SD の 1 回ごとに
  `supervisor::beat(Who::Web)`)、待ち受けに戻ったら `supervisor::park(Who::Web)` で監視を止める。1 つの要求で止まったら
  `last reset: wdt: web stalled 21s` を記録してリセットする (取得タスク全体の 90 s より早い)。
- 写真の追加 (最長 60 s) の間も OTA 確認は遅れるが、止まりはしない (取得タスクの上限 90 s 以内、60 s ごとの確認が 1 回ずれるだけ)。
- `tools/ticker-tests` の `boot_sim` に段階 `Web` (OTA 確認の直後) を足し、`Crash(Web)` / `Hang(Web)` を注入して「buy しない・
  旧版へ戻る・どの試行でも OTA 確認が先に通る・直した版で直る」、buy の後に落ちる場合は回復モード (サーバ無し) で直ることを確かめる
  (`web_server_hang_or_crash_is_never_bought_and_ota_still_runs`、`broken_first_round_is_never_bought` の全段階)。

## 7. 制限

- **RSSI は出せない**: cyw43 0.6 は接続中の電波の強さを読む API を公開していない (`/api/status` の `rssi` は常に null)。
- **mDNS (`ticker.local`) は無い**: embassy-net の `multicast` (IGMP) と cyw43 のマルチキャストの登録、UDP ソケットを
  もう 1 つと、そのバッファが要る。スタックの余裕 (10 kB 以上を保つ) を削るので見送った。IP で開く。
- 接続は 1 本ずつ (keep-alive、5 s 何もなければ閉じる)。2 台で同時に開くと、片方が少し待たされる / つながらないことがある。
- 取得タスクが天気 (最長 20 s) / OTA 確認 (最長 30 s、ダウンロードは数分) をしている間は応答しない。
- サムネイルは写真を 1 枚ずつ丸ごと送る (1 枚 115 kB、LAN で 1〜2 s)。16 枚なら 20〜30 s で全部並ぶ。
- 追加できる写真は 400×96 に切り抜いたものだけ (大きな BMP は SD へ手で置けばスライドショーは読む)。
- 夏時間は扱わない (時差は固定。季節で変わる地域は保存し直す)。

## 8. RAM / フラッシュ (0.4.2 → 0.5.0、`--features tbyb`)

| | 0.4.2 | 0.5.0 |
|---|---|---|
| `.text` + `.rodata` | 778,288 + 531,536 = 1,309,824 B | 874,108 + 555,748 = 1,429,856 B (+120 kB: サーバ、ページ 17.5 kB (gzip、元 57 kB)、SD の書き込み) |
| `ticker.bin` | 1,315,200 B | 1,434,740 B (1 スロット 1920 kB の 73 %) |
| `.data` + `.bss` | 4,144 + 489,168 B | 4,540 + 490,532 B (+1.8 kB: 待ち受けソケットのバッファ 1 kB + 1 kB) |
| 空きスタック | 38,144 B | 36,380 B |
| 最深経路 (+ 割り込み) | 24,416 B | 23,588 B (取得タスク → OTA 確認 → TLS。設定ページの経路は ≈ 7.5 kB) |
| 余裕 | +13.7 kB | **+12.8 kB** |

## 9. ホストで確かめる (`tools/settings-mock`)

```sh
python3 tools/settings-mock/mock_server.py            # http://127.0.0.1:8080/ (コード 123456)
NODE_PATH=$(npm root -g) node tools/settings-mock/screenshots.mjs out/ 写真.jpg
```

`mock_server.py` は **ファームウェアと同じ `web/settings/index.html`** を gzip で返し、API を同じ形・同じ検査
(コード、締め出し、Origin、BMP のヘッダ、8.3 の名前、値の規則) で真似る (写真は `IMAGE*.BMP` と `tools/ui-sim/samples`)。
`screenshots.mjs` (Playwright) はデスクトップ / スマートフォン / 切り抜きの画面を PNG にする (地名検索は決まった結果を返す)。

ページは `build.rs` が gzip にして埋め込む (`OUT_DIR/settings.html.gz`)。HTTP の解釈、フォームの復号、`ticker.txt` の書き換え、
値の検査、アクセスコードと締め出し、Host / Origin の確認、BMP のヘッダ、8.3 の名前、JSON は `tools/ticker-tests` の `web_tests`。
