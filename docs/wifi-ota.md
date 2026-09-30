# Wi-Fi OTA (第 2 段階): `wifi_ota` で GitHub Release から自己更新する

設計の全体は [ota-design.md](ota-design.md)、パーティションテーブルの導入と A/B・TBYB の
実機確認は [ota-setup.md](ota-setup.md)。ここでは `wifi_ota` bin の使い方と挙動をまとめる。

> **v0.3.0 から**: Release の OTA イメージ (`manifest.json` の `bin`) は `wifi_ota.bin` ではなく
> [`ticker`](ticker.md) の `ticker.bin`。OTA / TBYB / 接続管理の本体は `src/ota/app.rs` に移して両 bin で
> 共用しており、この文書の手順・LCD の文言・失敗時の挙動はそのまま `ticker` の下 2 行に当てはまる。
> 実機は manifest の `bin` の名前をそのまま取りに行くので、0.2.8 の `wifi_ota` からも `ticker` へ更新される。

## 1. 仕組み

`wifi_ota` は `wifi_status` (SD カードの `WIFI.TXT` で Wi-Fi に接続し、周辺 AP の RSSI を表示) に
OTA 機能を足したもの。

```
起動 → LCD 走査開始 → SD の WIFI.TXT → Wi-Fi join → DHCP
   ↓ (TBYB 起動なら、ここまで通ったら explicit_buy で確定)
   ↓ 5 秒後、以後 60 秒ごと (OTA_CHECK_INTERVAL)
[1] GET https://github.com/Droplet-Collective/aki300yenLCD-raspico2w/releases/latest/download/manifest.json
      302 → github.com/.../releases/download/vX.Y.Z/manifest.json → 302 → *.githubusercontent.com/... を自前で追う
      404 = Release がまだ無い (正常、次回また確認)
[2] {"version":"0.2.1","bin":"wifi_ota.bin","size":N,"sha256":"..."} を semver で比較。
      自分 (CARGO_PKG_VERSION) より厳密に新しいときだけ続行
[3] 書き込み先 = 自分が起動している A/B 区画の他方 (P0 app-a ⇄ P1 app-b)
      対象区画の先頭セクタ (IMAGE_DEF) を消して無効化
[4] GET .../releases/latest/download/wifi_ota.bin を 4 kB ずつ受信しながら
      セクタ消去 → 256 B ページ書き込み。同時に SHA-256 を計算。先頭 4 kB だけは RAM に取り置く
[5] 受信サイズ == size かつ SHA-256 == sha256 なら先頭セクタを書き、
      0x1C000000 (アドレス変換を通さない XIP 窓) から全域を読み戻してもう一度 SHA-256 を比較
[6] reboot(FLASH_UPDATE, p0 = 対象区画) → 新版が TBYB (ウォッチドッグ 16.7 s) で起動
[7] 新版: ウォッチドッグを 2 s ごとに延長しながら LCD 走査 + Wi-Fi join + DHCP 完了を待つ
      → explicit_buy → 確定。起動から 120 s (TBYB_SELFTEST_DEADLINE_SECS) までに通らなければ
      延長をやめ、ウォッチドッグで旧版へ戻る (§5.1)。進行は WATCHDOG.SCRATCH5〜7 に記録し、
      巻き戻り後の旧版が LCD に出す (§5.2)
```

- 更新元のリポジトリ名は `src/ota/mod.rs` の `REPO` に固定 (SD カードからは読まない)。
- 対象区画に「manifest と同じ SHA-256 のイメージ」が既にあるときはダウンロードしない。
  これは前回その版で TBYB 起動したが自己診断が通らず巻き戻された状態なので、LCD に
  `already in slot B but was rolled back` と表示し、10 分後にもう一度 FLASH_UPDATE 起動を試す
  (以後 60 秒ごとの確認で新しい Release が出ていれば普通に更新する)。
- ダウンロード中も LCD の走査は乱れない。フラッシュ消去 (45〜400 ms) / 書き込みは割り込み禁止で
  行うが、LCD の DMA リングは SRAM だけを読むため ([ota-design.md §4.1](ota-design.md#41-表示とフラッシュ操作の共存))。
  cyw43 側は割り込みが遅れるだけで、1 セクタずつ挟むのでタイムアウト内に収まる見込み (実機未確認)。

## 2. 初回インストール

前提: パーティションテーブル `pico2w-ab.uf2` を入れてある ([ota-setup.md §1](ota-setup.md#1-パーティションテーブルの導入-1-回だけ))。
SD カードのルートに `WIFI.TXT` (1 行目 SSID、2 行目パスワード) を置く。

Release (または CI アーティファクト `firmware-<sha>`) から次のどちらかを BOOTSEL ドライブへ
ドラッグ&ドロップ、または `picotool load -f -v -x`:

| ファイル | TBYB | 用途 |
|---|---|---|
| `wifi_ota.uf2` | **有り** | OTA と同じイメージ。FLASH_UPDATE で起動し、Wi-Fi + DHCP が通れば buy して確定。**Wi-Fi が使えない機体に入れると 16.7 秒で旧版へ戻る** (旧版が無ければ BOOTSEL に落ちる) ので、初回は Wi-Fi 設定を済ませてから |
| `wifi_ota-plain.uf2` | 無し | 従来どおりの起動 (ウォッチドッグ無し)。Wi-Fi 未設定でも起動する。初回はこちらが安全 |

版数は bin の種類を区別しない ([ota-setup.md §4.4 の注意](ota-setup.md#44-表示修正後の再確認-v012--v013))。
他方の区画に `ota_selftest` 等の高い版数が残っていると、電源再投入でそちらが起動する。
その場合は `picotool erase -p <n>` で消すか、`wifi_ota` の版数を上げる。

## 3. 更新を配る (開発者側)

1. `Cargo.toml` の `version` を上げる (例 `0.2.0` → `0.2.1`)。IMAGE_DEF の版数 (`0.201`) は自動で決まる。
2. コミットして main に push (PR 経由でも直接でも)。
3. Release を作る。方法は 2 つあり、どちらも **タグ `vX.Y.Z` の X.Y.Z が `Cargo.toml` の `version` と
   一致していなければ失敗し、Release は作られない**。
   - (a) 手でタグを push する: `git tag v0.2.1 && git push origin v0.2.1`。
     `.github/workflows/build.yml` の `push: tags` がビルドし、`release` ジョブが Release を作る。
   - (b) `release` ワークフローを実行する (タグを手で打てない環境や自動化向け):
     Actions タブ → `release` → Run workflow で `version` に `0.2.1` を入れる、または
     `gh workflow run release.yml -f version=0.2.1` (`-f ref=<ブランチ>` で main 以外も可、既定 main)。
     `.github/workflows/release.yml` が `Cargo.toml` の版数一致とタグ未存在を確認してから
     build.yml のビルドジョブを呼び出し、ビルドしたコミットに注釈付きタグ `v0.2.1` を打って Release を作る。
     GITHUB_TOKEN で push したタグは他のワークフローを起動しない (GitHub の規則) ため、
     アセットの添付も release.yml 自身が行う。

   どちらの経路でも `wifi_ota` を `--features tbyb` でビルドして `wifi_ota.bin` / `wifi_ota.uf2` /
   `wifi_ota.sha256` / `manifest.json` (`scripts/make-manifest.sh`) と他 bin の UF2 を Release に添付する。
4. 実機は 60 秒以内に manifest を見に行き、新しければダウンロード → 検証 → 再起動 → 自己診断 → 確定。
   400 kB 台のイメージで、LAN 内なら 1〜2 分で完了する見込み (実測はまだ)。

手元で確認するには:

```sh
cargo build --release --bin wifi_ota --features tbyb
PICOTOOL=/path/to/picotool scripts/make-ota-image.sh target/thumbv8m.main-none-eabihf/release/wifi_ota out
scripts/make-manifest.sh out/wifi_ota.bin 0.2.1 out/manifest.json
```

## 4. LCD の読み方

```
MyWiFi 192.168.1.23 -52dBm  scan #12                                       ← 行 0: wifi_status と同じ
wifi_ota v0.2.4 via OTA slot B TBYB:bought OK                              ← 行 1: 自分の版数 / 区画 / TBYB
OTA: 0.2.0 -> 0.2.1 downloading 45%  196608/435200 B                       ← 行 2: OTA の状態
[====================                          ]                           ← 進捗バー (ダウンロード中のみ)
 SSID / RSSI 棒グラフ (上位 5 件。下の診断行が出るときは 3〜4 件)
TBYB 0.2.3: dhcp-wait @121.3s join3 fail2 dhcpto1                          ← 前回の TBYB 起動の記録 (あるとき、§5.2)
NORMAL P0 A:4C4D launched B:000D imgdef reset:wdt                          ← 起動種別 / 診断 / リセット理由 (ウォッチドッグ起動のとき、§5.2)
```

各行は FONT_6X10 で 66 桁 (400 px) に収まるように書いている (v0.2.3 までの行 1 は 100 桁を超え、
末尾の `WDT` が画面外だった。行 2 の `retry boot in NNNs` も末尾が切れていた)。

v0.2.7 から画面の外周 1 px に暗い灰色の枠を常に描く。写真で四辺が見えれば 400×96 の全体が LCD に
表示されている (v0.2.6 までは左端の 1 文字が欠けていた。`src/lcd/display.rs` の `VISIBLE_X_OFFSET` を
98 → 106 に補正)。v0.2.7 だけは起動から 5 秒間、行 0 の代わりに x 座標の目盛り (10 px ごとの刻み、
50 px ごとのラベル `0` 〜 `350` と右端の `390`、左右端の中央に短い印) を出していた。実機の写真で
四辺の枠線と左端の `0` / 右端の `390` の両方が読めることを確認 (2026-09-29) したので、v0.2.8 で目盛りは
削除し、106 を正式値とした。枠線はそのまま残している。

行 1 の `via OTA` は、今動いているイメージが `reboot(FLASH_UPDATE)` で起動されたとき
(= OTA で書き込んだ側の区画から起動したとき) だけ版数の隣に出る (BOOT_INFO の boot_type)。
USB で入れた版では出ないので、OTA 更新が実際に反映されたかを版数と合わせて一目で確認できる。

行 1 の TBYB 表示 (ota_selftest と同じ色分け):

| 表示 | 意味 |
|---|---|
| `TBYB:no` (灰) | TBYB でない通常起動 |
| `TBYB:pending 37/120s WDT 15.1s` (黄) | TBYB 起動。Wi-Fi + DHCP が通れば buy。`37/120s` は起動からの経過秒 / 自己診断の締め切り (§5.1)。`WDT` は bootrom のウォッチドッグ残り秒で、延長中は 16.7 → 14.7 s を繰り返す |
| `TBYB:timeout->rollback` (赤) | 締め切り (120 s) までに Wi-Fi + DHCP が通らなかった。延長をやめたので最長 16.7 s 後に旧版へ戻る |
| `TBYB:bought OK` (緑) | explicit_buy 成功 (bootrom がウォッチドッグを止めるので `WDT` 表示も消える)。以後この版が通常起動で選ばれる |
| `TBYB:buy FAILED rc=-N` (赤) | explicit_buy 失敗。bootrom は explicit_buy の冒頭でウォッチドッグを止めるため自動では戻らない。電源を入れ直せば旧版 (非 TBYB) が選ばれる |

版数の隣の `slot A` / `slot B` は起動中の区画。TBYB フラグ無しのビルド (`wifi_ota-plain`) では `plain` が付く。

行 2 の OTA 表示:

| 表示 | 意味 |
|---|---|
| `OTA: waiting for network` | DHCP 前 |
| `OTA: disabled (...)` | `WIFI.TXT` が無い / パーティションテーブルが無い等 |
| `OTA: checking manifest.json (#n)...` | 取得中 |
| `OTA: no release yet (404), next check in 55s` | Release が無い (正常) |
| `OTA: up to date (latest 0.2.0), next check in 55s` (緑) | 最新 |
| `OTA: 0.2.0 -> 0.2.1 downloading 45%  ...` (黄) | 書き込み中。進捗バー付き |
| `OTA: 0.2.1 downloaded, verifying (sha256 + readback)...` (黄) | 先頭セクタ書き込みと読み戻し検証 |
| `OTA: 0.2.1 verified -> reboot into slot B in 2s (TBYB)` (緑) | 直後に FLASH_UPDATE 再起動 |
| `OTA: 0.2.1 in slot B was rolled back; retry boot in 590s` (赤) | 前回の TBYB 起動で buy されなかった (§5)。残り秒は最初にそう判定した時刻から数える (60 s ごとの確認で延びない) |
| `OTA: TLS failed, retry in 120s` (赤) | 失敗。理由 (`DNS failed` / `network error` / `TLS failed` / `bad HTTP response` / `HTTP 5xx` / `bad manifest.json` / `bad image size` / `size mismatch` / `sha256 mismatch` / `flash readback mismatch` / `flash error` / `timeout`) とバックオフ後の再試行時刻 |

## 5. 失敗時の挙動

| 事象 | 結果 |
|---|---|
| DNS / TCP / TLS / HTTP エラー、タイムアウト (manifest 30 s、ダウンロード 300 s、ソケット無通信 20 s) | LCD に表示。60 s → 120 s → … → 最大 10 min のバックオフで再試行。成功したらバックオフは 60 s に戻る |
| ダウンロード途中で切断・電源断 | 対象区画は先頭セクタを消した状態 (無効)。起動側は無傷。次回また最初から |
| 受信サイズ / SHA-256 / 読み戻しの不一致 | 対象区画の先頭セクタを消して終了。再起動しない。バックオフ後に再試行 |
| 新版が起動しない / ハング (ウォッチドッグを延長するタスクまで届かない) | 16.7 s のウォッチドッグで旧版へ。旧版は manifest と対象区画の内容が一致することから「巻き戻された」と判断し、最初にそう判定してから 10 分後に FLASH_UPDATE 起動を再試行 (その後も 10 分ごと)。**v0.2.3 までは 60 s ごとの確認のたびに 10 分後へ延びるバグがあり、再試行は一度も起きなかった** (v0.2.4 で修正) |
| 新版は起きるが Wi-Fi + DHCP が 120 s 以内に通らない | 延長をやめ、最長 16.7 s 後にウォッチドッグで旧版へ (§5.1)。以後は上と同じ。v0.2.6 からは DHCP が 20 s で通らないたびに AP から離脱して再 join する (§5.3) |
| join は通るが DHCP が一度も通らない (温かい再起動で CYW43439 が接続中の状態を引き継いだ) | v0.2.5 の実機で発生。v0.2.6 から起動時に WL_REG_ON を 500 ms 落としてコールドスタートさせ、再起動前にも電源を切る (§5.3) |
| explicit_buy 失敗 | LCD に表示。bootrom が explicit_buy の冒頭でウォッチドッグを止めるので、電源を入れ直すまで新版 (未確定) のまま動く。次の通常起動では旧版が選ばれる |
| 電源断が「書き込み完了〜再起動」の間に起きた | 対象区画は有効な TBYB イメージ。通常起動では選ばれないが、次回起動時の確認で「巻き戻された」扱いになり 10 分後に FLASH_UPDATE 起動する |

### 5.1 自己診断とウォッチドッグ

bootrom は TBYB イメージを起動するとき、内部で `reboot(NORMAL, delay = 0xFFFFFF ms)` に相当する
設定を行う: `WATCHDOG.CTRL = 0` → `PSM_WDSEL` を全段リセットに → `SCRATCH2..7` を通常起動用に
クリア → `WATCHDOG.LOAD = 0xFFFFFF` (24 bit × 1 µs ≈ 16.7 s、ハードウェアの上限) → `CTRL.ENABLE = 1`
(pico-bootrom-rp2350 `varm_launch_image.c` / `varm_apis.c`)。データシート §5.1.17 は
「カウンタを再ロードすれば延長できる (ただし延長し続けて抜けられなくなる危険がある)」と明記している。

v0.2.2 までの `wifi_ota` はこのウォッチドッグに一切触れず、16.7 s 以内に Wi-Fi join + DHCP が
通ることを前提にしていたが、実機では DHCP 待ちの途中で 16.7 s が尽きて旧版へ巻き戻った
(join に数秒、DHCP は最大 20 s)。v0.2.3 からは次のように扱う:

- `BOOT_INFO` が buy 待ちを示していたら、起動直後に一度 `WATCHDOG.LOAD` に `0xFFFFFF` を書き、
  `tbyb_watchdog_task` を起動する。タスクは 2 s ごと (`TBYB_WATCHDOG_FEED_INTERVAL`) に同じ値を
  書き続ける。main ループは join / DHCP / scan で数秒〜20 s 待つので、別タスクで回す。
- `LOAD` は書き込み専用でカウンタを再ロードするだけ。`CTRL` (ENABLE / PAUSE_*) や、bootrom の
  再起動パラメータが入る `SCRATCH2..7` には触れない。embassy の `Watchdog::start` は `CTRL` と
  `SCRATCH` を書き換えるので使わない。
- 起動から 120 s (`TBYB_SELFTEST_DEADLINE_SECS`、0.4.2〜 180 s) までに自己診断 (0.4.1 まで: LCD 走査中 + 起動 2 s 以上 +
  Wi-Fi join + DHCP で IP 取得。0.4.2〜: LCD 走査中 + Wi-Fi + DHCP + OTA の manifest 確認が TLS + HTTP を最後まで通った
  + 25 s。`ticker` はさらに機能の一巡、[ticker.md §8.2](ticker.md)) が通らなければ延長をやめ、buy もしない。以後は最長 16.7 s で
  ウォッチドッグが発火し、bootrom が旧版で通常起動する。これが「延長し続けて抜けられなくなる」
  ことへの歯止めで、LCD には `TBYB: self-test timed out (120 s), rolling back` と出る。
- 自己診断が通ったら `explicit_buy`。bootrom の `explicit_buy` は最初に `CTRL.ENABLE` を落とす
  (`s_varm_api_explicit_buy` の 1 行目) ので、ファームウェア側でウォッチドッグを止める必要はなく、
  buy 後は `WDT` 表示が消える。成功・失敗にかかわらず延長タスクも終わる。
- buy 条件そのものは v0.2.2 から変えていない。締め切りの 120 s は join の再試行 (5 → 10 → 20 s
  間隔) と DHCP (20 s タイムアウト) を数回やり直せる長さとして決めた。

### 5.2 起動診断 (v0.2.4): 巻き戻りの原因を旧版の画面で読む

巻き戻ると新版の画面は消えるので、buy 待ちの新版は進行を `WATCHDOG.SCRATCH5〜7` に書き続ける
(`src/boot_trace.rs`)。SCRATCH5 = 版 (識別用 magic 付き)、SCRATCH6 = 稼働時間 (0.1 s 単位) + 段階、
SCRATCH7 = 付加情報 (join 回数 / join 失敗 / DHCP タイムアウト / 直近 join status、PANIC なら行番号、
HARDFAULT なら PC)。段階は `main → wdt-feed → sd-read → lcd → cyw43-pwr-cycle (0.2.6〜) → cyw43-init →
cyw43-ready → joining → (join-failed) → joined → dhcp-wait → (dhcp-timeout → dhcp-rejoin (0.2.6〜) → joining …)
→ network-up → buy-called → bought / buy-failed`、
締め切り超過は `selftest-timeout`。延長タスクが 2 s ごとに稼働時間を更新する。`wifi_ota` は panic-probe の
代わりに自前の panic / HardFault ハンドラを持ち、これらも記録してから止まる (止まり方は panic-probe と同じ)。

SCRATCH5〜7 を使える根拠 (pico-bootrom-rp2350): bootrom が SCRATCH5〜7 を書くのは FLASH_UPDATE など
NORMAL 以外の `reboot()` のときだけ (`varm_apis.c s_varm_hx_reboot`)。TBYB 起動時に仕掛けるウォッチドッグは
NORMAL 型で SCRATCH2〜4 しか書かず、起動時の `try_vector` (`varm_boot_path.c`) も magic が合ったときに
SCRATCH4 を 0 にするだけ。SCRATCH と `WATCHDOG.REASON` はチップレベルリセット / RUN ピンで消えるが
(データシート §12.9.5)、bootrom のウォッチドッグは PSM リセット (`PSM_WDSEL`) なので残る。

巻き戻り後に起動した旧版は起動時にこれを読み、LCD の下段 2 行に出す (AP 一覧はその分減る):

| 行 | 例 | 意味 |
|---|---|---|
| 記録 (あるときだけ) | `TBYB 0.2.3: dhcp-wait @121.3s join3 fail2 dhcpto1` (黄) | 0.2.3 の TBYB 起動は 121.3 s 時点で DHCP 待ち、join 3 回 (失敗 2)、DHCP タイムアウト 1 回 まで進んで巻き戻った |
| | `TBYB 0.2.3: selftest-timeout @120.0s ...` (赤) | 締め切りで自ら延長をやめた (Wi-Fi + DHCP が 120 s で通らなかった) |
| | `TBYB 0.2.3: PANIC @12.3s line=1234` / `HARDFAULT @12.3s pc=0x1001abcd` (赤) | panic / HardFault で止まりウォッチドッグが発火した (`line` は panic 箇所の行番号、`pc` はフォルト時の PC) |
| | 段階が途中で `@16.xs` 前後 | 延長タスクが動く前 / 延長が効かずに 16.7 s で発火した (`main`〜`lcd` で止まる等) |
| 起動 (ウォッチドッグ経由の起動のとき) | `NORMAL P0 A:4C4D launched B:000D imgdef reset:wdt` (灰) | 起動種別 / 起動パーティション、BOOT_INFO 診断ワードの A 側・B 側 (16 進と要約: `launched` / `cond-fail` (検証は通ったが TBYB 条件などで起動せず) / `chosen` / `consid` (検証対象になった) / `imgdef` / `badloop` / `searched` / `-`)、`WATCHDOG.REASON` (`wdt` = タイマ満了。bootrom の `reboot()` も `wdt` になる、`force`、`hw` = 電源投入等) |

読み方の例:

- 記録行が無く、起動行が `FLASH_UPDATE P0 ... B:xxxx consid/cond-fail` → bootrom が新版を起動せず旧版を直接
  起動した (新版は一度も走っていない)。このとき旧版の行 1 には `via OTA` が出る。
- 記録行 `selftest-timeout @120.0s` → 新版は動いたが Wi-Fi + DHCP が通らなかった。`join`/`fail`/`dhcpto`/`st` で内訳。
- 記録行が `PANIC` / `HARDFAULT` → 新版のバグ。行番号 / PC で場所を特定できる。
- 記録行の段階が途中で稼働時間が 16.7 s 未満 → 延長が始まる前に発火した (起動が 16.7 s 以上かかった)。

### 5.3 温かい再起動と CYW43439 の状態 (v0.2.6): join は通るのに DHCP が通らない

v0.2.5 の TBYB 起動 (0.2.4 からの FLASH_UPDATE) を §5.2 の記録で読むと
`TBYB 0.2.5: selftest-timeout @120.1s join1 fail0 dhcpto1` / `NORMAL P1 A:000D imgdef B:506D launched reset:wdt`
だった。つまり新版は起動し、ウォッチドッグの延長も 120 s まで効き、Wi-Fi の join は 1 回で成功したのに、
DHCP は 20 s のタイムアウトが 1 回記録されたあと 120 s まで IP を取れず、締め切りで巻き戻った。
同じイメージを電源投入や BOOTSEL から起動すると数秒で IP が取れるので、コードではなく起動経路の違いが原因と見る。

- `reboot(FLASH_UPDATE)` は RP2350 だけをリセットする。CYW43439 は直前まで通電・AP に接続・DHCP 済みの
  まま次の版に引き継がれる。cyw43 0.6 の `Bus::init` は WL_REG_ON (GP23) を **20 ms** 落として上げ、250 ms
  待ってから WLAN / SOCSRAM コアをリセットしてファームウェアを転送するが、接続中だったチップに対しては
  この電源断が短く、association はできるのにデータ経路 (DHCP のブロードキャスト) が通らない状態になり得る。
- v0.2.5 までの接続管理は DHCP タイムアウト後も `Joined` のまま DHCP クライアントに任せ、リンクが落ちない
  限り再 join しなかった (`join1 dhcpto1` のまま 120 s)。

v0.2.6 の対策 (`src/wifi.rs` / `src/bin/wifi_ota.rs`):

1. 起動時、cyw43 にピンを渡す前に WL_REG_ON を `CYW43_POWER_OFF_MS` = **500 ms** Low に保つ (`wifi::start`)。
   起動経路にかかわらず毎回コールドスタートになる。LCD 行 0 には `Wi-Fi: power cycle (500 ms) + init...`、
   §5.2 の記録には段階 `cyw43-pwr-cycle` が出る。`wifi_status` も同じ経路を通る。
2. FLASH_UPDATE 再起動 (検証後・巻き戻り後の再試行の両方) の直前に `control.leave()` → WL_REG_ON Low →
   100 ms → `reboot()` (`wifi::power_off_for_reboot`)。GP23 の `Output` は cyw43 の `Bus` が持っているので
   `PIN_23::steal()` で作り直して Low に駆動し、drop で駆動が外れないよう `forget` する (Bus は init 後に
   このピンへ触らない)。LCD 行 0 には `rebooting into slot B (P1)... wifi off`。
3. DHCP が 20 s で通らなければ `leave()` して 0.5 s 後に再 join する (段階 `dhcp-timeout → dhcp-rejoin → joining`)。
   記録の `join` は再 join も数えるので、`join3 fail0 dhcpto2` なら DHCP 再試行 2 回。TBYB の締め切り (120 s)
   判定と延長タスクはこのループの外で回り続けるので、再試行しても 120 s で必ず巻き戻る。

## 6. セキュリティ (重要)

**TLS は `TlsVerify::None`、つまりサーバ証明書を検証していない。** 経路上の攻撃者 (DNS 詐称、
偽 AP、ルータ) が manifest と bin を差し替えれば、任意のファームウェアを実機に入れられる。
manifest の SHA-256 は破損検出と「書き込んだ物が受信した物と一致する」ことの確認であり、
manifest 自体が同じ経路で来る以上、改竄対策にはならない。**信頼できる LAN でだけ使うこと。**

検証しない理由 (embedded-tls 0.18 + reqwless 0.14 で調べた結果):

- GitHub のアセット配信ホスト `*.githubusercontent.com` (objects. / release-assets.) は 2026-09 時点で
  Let's Encrypt (中間 CA `YR2`) の **RSA 4096 bit** 証明書 (90 日で更新)。github.com は Sectigo
  (`DV E36`) の ECDSA P-256。RSA を扱うには embedded-tls の `rsa` feature (→ `alloc`) が必須で、
  検証しなくても **ClientHello に RSA 署名方式を載せないとハンドシェイク自体が成立しない**。
  そのため `rsa` feature と 8 kB のヒープ (`embedded-alloc`) を入れている。
- reqwless の `TlsVerify::Certificate { ca }` は embedded-tls の `rustpki` 検証器を使うが、これは
  ホスト名をリーフ証明書の **CommonName と完全一致** でしか比較しない (SAN もワイルドカードも非対応)。
  `objects.githubusercontent.com` に対する CN は `*.githubusercontent.com` なので必ず失敗する。
- webpki 経路 (SAN / ワイルドカード対応) は rustls-webpki 0.101 = `ring` 依存で、このターゲットでは使えない。
- CA ピン留めは GitHub 側の CA 変更 (DigiCert → Sectigo → Let's Encrypt と実際に変わっている) で
  更新が止まるリスクがある。

第 3 段階の予定: manifest (と bin の SHA-256) に Ed25519 署名を付け、公開鍵をファームウェアに
埋め込んで検証する。これなら TLS の検証有無に関係なく、鍵を持つ人が作った Release しか受け入れない。

## 7. RAM / フラッシュ

`cargo build --release --bin wifi_ota --features tbyb` (v0.2.0) の `llvm-size`:

| 領域 | サイズ | 内訳 |
|---|---|---|
| `.text` + `.rodata` | 619,340 + 279,040 B ≈ **877 kB** | 1 スロット 1920 kB の 46 % (`wifi_status` は 419 kB)。増分は reqwless / embedded-tls / rsa / p256 / cyw43 |
| `.data` + `.bss` + `.uninit` | 580 + 482,160 + 1,024 B ≈ **472 kB** | LCD フロント 230,068 + バック 153,600、TLS 受信 16,640 + 送信 3,072、HTTP ヘッダ 8,192 + 受信単位 2,048、URL 2,048、TCP 4,096 + 2,048、セクタ作業 8,192、ヒープ 8,192、main タスク 15,912、cyw43 12,688、embassy-net 3,832 など |
| 静的領域の終わり | `0x200751B8` | |
| スタック (0x20080000 まで) | **40,520 B ≈ 39.6 kB** | TLS ハンドシェイク (p256) と rsa の一時領域を含めて足りる見込み。実測はまだ |

`wifi_status` の空きは 0x20080000 − 0x20064724 = 112,860 B。

## 8. 未確認事項 (実機)

- HTTPS 取得全般: GitHub のリダイレクト、Location の長さ (release-assets は JWT 付きで 1.5 kB 前後)、
  TLS 1.3 ハンドシェイク (RSA 4096 の CertificateVerify を `NoVerify` で受ける)、ダウンロード速度。
- フラッシュ書き込み中の cyw43 (割り込み遅延) とダウンロードの共存。
- `reboot(FLASH_UPDATE)` の p0 が `0x10000000 + オフセット` で正しいか (ota_selftest の
  picotool `-x` と同じ形式。ストレージオフセットそのままの可能性が残る)。
- スタック使用量 (43.6 kB の余裕で足りるか)。
- 「巻き戻し」検出と 10 分後の再試行が意図どおり動くか。
- ウォッチドッグの延長 (§5.1) で DHCP 待ちを越えて buy まで到達するか、120 s の締め切り後に
  本当に旧版へ戻るか。v0.2.3 の実機では 0.2.3 が buy に至らず 0.2.1 に戻った (原因未特定。
  0.2.1 の再試行が §5 のバグで起きず、再現観察もできなかった)。v0.2.4 は原因を §5.2 の記録で
  切り分けるための版。
- §5.2 の記録が実機で残るか (SCRATCH5〜7 が PSM リセットを越えて保持されることの確認)。

## 9. リリース履歴

| 版 | 内容 |
|---|---|
| 0.2.0 | 初版 (wifi_ota の OTA 機能、release.yml) |
| 0.2.1 | OTA 更新テスト用。FLASH_UPDATE 起動時に行 1 の版数の隣へ `via OTA` を表示 |
| 0.2.2 | 自動更新の実機テスト用（版数表示の色を変更）。実機で 0.2.1 → 0.2.2 の OTA (ダウンロード・検証・FLASH_UPDATE 起動) を確認したが、DHCP 待ち中に 16.7 s のウォッチドッグが尽きて旧版へ巻き戻った |
| 0.2.3 | TBYB の buy 待ち中に bootrom のウォッチドッグを 2 s ごとに延長し、起動 120 s を自己診断の締め切りにする (§5.1)。LCD の TBYB 表示に経過秒 / 締め切りを表示。実機では 0.2.1 → 0.2.3 の FLASH_UPDATE 後にやはり 0.2.1 に戻った (原因未特定) |
| 0.2.4 | (1) 巻き戻り後の 10 分再試行が 60 s ごとの確認で毎回延びて起きなかったバグを修正 (§5)。(2) buy 待ちの進行を WATCHDOG.SCRATCH5〜7 に記録し、旧版が `WATCHDOG.REASON` / BOOT_INFO 診断と共に LCD に出す (§5.2)。panic / HardFault も記録。(3) LCD の各行を 66 桁に収め、`WDT` 残り秒と再試行の残り秒が読めるようにした (§4) |
| 0.2.5 | TBYB 起動診断付きの OTA 再テスト。実機では 0.2.4 → 0.2.5 の FLASH_UPDATE 後に `selftest-timeout @120.1s join1 fail0 dhcpto1` で巻き戻った。原因: 温かい再起動で CYW43439 が接続中の状態を引き継ぎ、join は通るが DHCP が通らない + DHCP タイムアウト後に再 join しない (§5.3) |
| 0.2.6 | (1) 起動時に WL_REG_ON を 500 ms 落として CYW43439 をコールドスタート、FLASH_UPDATE 再起動前に `leave()` + 電源断。(2) DHCP タイムアウトごとに AP から離脱して再 join。(3) 起動診断に段階 `cyw43-pwr-cycle` / `dhcp-rejoin` を追加 (§5.3)。実機で 0.2.6 への OTA (ダウンロード → 検証 → FLASH_UPDATE → 自己診断 → buy) が通ることを確認 (2026-09-29) |
| 0.2.7 | 表示位置の補正 + 確認用の枠線。実機の写真で左端の 1 文字 (6 px) が全行で欠けていたため、バックバッファをフレーム行へ置く位置 `VISIBLE_X_OFFSET` を 98 → 106 に補正 (HSYNC の取り込みがフレーム行の x=−1 に当たり、バックポーチ 107 clk 後の x=106 が最初の表示画素。`src/lcd/display.rs` のコメント)。画面の外周 1 px に暗い灰色の枠を常に描き、起動から 5 秒間は x 座標の目盛りを出す (§4)。PIO のタイミングは変更なし |
| 0.2.8 | 表示位置 106 を正式版に: 目盛りを削除、ota_selftest_min も 106 に統一、実機で四辺の枠線と目盛りの 0/390 を確認 (2026-09-29) |
| 0.3.0 | **ネットワーク・ティッカー** ([ticker.md](ticker.md))。OTA / TBYB / 接続管理を `src/ota/app.rs` に共通化し、新 bin `ticker` (NTP 時計 + Open-Meteo 天気 + GitHub の `message.txt` を流す表示、美咲フォント) を追加。Release の `manifest.json` は `ticker.bin` を指すので、0.2.8 の `wifi_ota` は OTA でそのまま `ticker` に切り替わる。`wifi_ota` の挙動は変えていない |
| 0.3.1 | `ticker` の日本語フォントを美咲 8×8 の 2 倍表示から東雲 14 ドット (Public Domain) の等倍に変更。実機の写真で 2 px の線が LCD 上で太く潰れて見えたため、線 1 px のフォントにした ([ticker.md §5](ticker.md))。流れる文字の帯の下の区切り線が状態行 1 の文字に重なっていた配置も修正。`wifi_ota` の挙動は変えていない |
| 0.4.0 | `ticker` に SD の写真のスライドショー背景とガラス風の新しい画面 ([ticker.md](ticker.md) §1)、PC の画面シミュレータ `tools/ui-sim` ([ui-sim.md](ui-sim.md))。LCD のバックバッファを RGB666 `u32` から RGB565 `u16` にした (垂直ブランキングのコピーで表引きして 18 bit に広げる。全 bin 共通、`wifi_ota` の RAM も 76.8 kB 空く)。OTA / TBYB の手順は変えていない |
| 0.4.1 | `ticker` が起動 1 分ほどで固まる不具合 (最初の HTTPS の TLS ハンドシェイクでスタック溢れ) を修正し、固まったら自分で戻るようにした ([ticker.md §7.1 / §8](ticker.md))。スタックの上端を 0x2008_2000 (SRAM8/9) にしたので全 bin の空きスタックが 8 kB 増える。`ota::http::fetch` はリダイレクト先を `chunk` に一旦写す (2 kB のローカル変数を無くした)。`BootStatus::write_tbyb_line` の `WDT` 残り秒は buy 待ち / 巻き戻り待ちのときだけ出す。boot_trace に段階 `Running` と異常終了 0xE2〜0xE6、SCRATCH0/1 の付加情報を追加 (SCRATCH5〜7 の形は同じなので旧版も読める)。picotool の USB reset interface でのリセット前に記録を消す。`wifi_ota` の OTA / TBYB の手順は変えていない |
| 0.4.2 | `ticker` の **OTA 到達保証** ([ticker.md §8](ticker.md))。共有の `ota::app`: buy 条件を `boot_policy::BuyGate` (Wi-Fi + DHCP + OTA の manifest 確認が TLS + HTTP を最後まで通った + bin ごとの機能の一巡 + 25 s) に変え、締め切りを 120 → 180 s。buy 待ちの間も manifest の確認だけは行い (`CheckMode::check_only`、新しい版は `NewerAvailable` として buy 後にダウンロード)、通信の失敗なら 10 s ごとに試し直す。`wifi_ota` も同じ buy 条件 (機能の一巡は無し) を使い、ウォッチドッグは従来どおり延長タスク。`OtaPhase` に `NewerAvailable` / `Blocked`、boot_trace に段階 `recovery` / `ota-ok` / `fallback` / `sd-init` を追加。SD の転送に期限 (`sdcard::set_deadline`) |
| 0.5.0 | `ticker` に **設定ページ** ([settings-server.md](settings-server.md)): 同じ LAN のブラウザから地域 / 表示 / 流れる文字 / 写真を変える HTTP サーバ (取得タスクの中で 1 要求ずつ、最初の OTA 確認の後だけ、回復モードでは動かない)。buy 条件の一巡に `web`、生存確認に `Who::Web` (20 s)、boot_trace に段階 `web` と異常終了 0xE7 (`WDT-WEB`)。SD に書き込めるようにした (`sdcard::SdVolumeDevice`)。`wifi_ota` の OTA / TBYB の手順は変えていない |
| 0.5.1 | `ticker`: 設定ページの URL とアクセスコードを **流れる文字の中に毎周** 入れる (1 分だけの下の帯の案内をやめた)。設定の部分があるときは切れ目なく繰り返して流す。`ticker.txt` の `show_settings=0` / 設定ページの切り替えで入れない。待ち受け前 / 回復モードでは入れない ([ticker.md §1.3](ticker.md))。OTA / TBYB / buy 条件 / 回復モードの手順は変えていない |
