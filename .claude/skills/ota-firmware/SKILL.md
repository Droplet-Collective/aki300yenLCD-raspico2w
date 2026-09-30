---
name: ota-firmware
description: OTA で配る / 更新されるファームウェア (ticker、wifi_ota、今後の新しい bin) を変更・追加・リリースするとき、または src/ota、src/boot_policy.rs、src/supervisor.rs、src/lcd、src/web (設定ページ)、メモリ配置 (memory.x、スタック、static バッファ)、リリースワークフロー (.github/workflows/build.yml / release.yml) を触るときは必ず使う。OTA 到達保証を壊さないためのチェックリスト。
---

# OTA ファームウェアの変更チェックリスト

Pico 2 W (RP2350) の実機は、Release の `manifest.json` が指す bin (v0.3.0〜 `ticker.bin`) を OTA で取りに行く。
**OTA の経路が壊れた版を一度でも buy させると、USB (BOOTSEL + UF2) でしか直せない** (0.4.0 で実際に起きた)。
詳細は [docs/ticker.md §8](../../../docs/ticker.md) (OTA 到達保証)、[docs/ota-design.md](../../../docs/ota-design.md)、
[docs/wifi-ota.md](../../../docs/wifi-ota.md)。ここは作業の手順と、壊してはいけないものの一覧だけ。

## 1. 絶対条件

> 「今後OTA書き込みできない部分でハングしないようにしてください。最低限ウォッチドックで再起動して
> 最新ファームを確認してアップデートできるところまでは必ず動くようにしてほしいです」 (おちょこ、2026-09-30)

どんな壊れ方 (panic / HardFault / スタック溢れ / ハング / 割り込みごと停止 / I/O の無応答) をしても、
**ウォッチドッグでリセット → 最新の版を確認 → 更新** まで必ず到達すること。
この条件を満たせない変更は、機能がどれだけ良くても出さない。迷ったら §2 の不変条件と §4 の確認で判断する。

## 2. 壊してはいけない不変条件

### 2.1 TBYB の buy 条件 (`src/boot_policy.rs` の `BuyGate` / `Round` / `classify_check`)

buy 待ちの版は次が **全部** 揃ってから `BUY_SETTLE_MS` (25 s) 健全に動いたときだけ `explicit_buy` する。
締め切り `BUY_DEADLINE_MS` (180 s) を過ぎたら buy せず、ウォッチドッグで旧版に戻す。

- (a) Wi-Fi join + DHCP で IP 取得。
- (b) **証明された (PROVED) OTA 確認**: `classify_check` が `CheckOutcome::Proved` を返したもの。
  = manifest.json を解釈できた、404 (Release 無し)、またはリダイレクトを追った後の確定した HTTP ステータス (5xx 等、`CheckFailure::FinalStatus`)。
  DNS / TCP / TLS / 時間切れ (`Transport`)、ヘッダ溢れ・構文エラー・切れた manifest (`BadResponse`) は **証拠にならない**。
  buy 待ち中は `PENDING_OTA_RETRY_MS` (10 s) ごとに再試行、manifest を読むだけ (ダウンロードしない)。
- (c) `Round` の全項目を 1 回ずつ試した (成否不問、落ちずに戻った): `sd_config` / `web` (0.5.0〜 設定ページの待ち受け) / `ntp` / `weather` / `message` / `slideshow`。
  ticker では `src/bin/ticker.rs` main ループの `Round { .. }` 組み立て (`ROUND_*` / `slideshow::FIRST_DONE`)。
  **起動時に動く機能を足したら、必ず `Round` に項目を足して buy 条件に入れる** (`Round::first_missing` の表示名も)。
- (d) 生存確認が揃っている (`supervisor::all_alive()` と `alive(Who::Render, 2_000)`)。途切れたら 25 s を数え直す。
- 実行部は `ota::app` の `buy_tick` (ticker / wifi_ota 共用。wifi_ota は機能の一巡が無いので `Round::DONE`)。

### 2.2 ウォッチドッグと生存確認 (`src/supervisor.rs`、`src/ticker/health.rs`)

- ticker は **どの起動種別でも** `embassy_rp::init` の直後に `supervisor::start_early()` (8 s、`WATCHDOG_TIMEOUT_US`)。
  その前に置いてよいのは `set_stack_limit()` / `paint_stack()` だけ。
- 初期化中は各段階の前に `supervisor::feed_init()`。描画タスク起動後は `supervisor::start(Limits::TICKER)` に切り替え、
  以後の再ロードは LCD フレーム割り込みの `on_frame` だけが、全タスクの `beat` が `Limits` 以内のときに行う。
  **無条件にウォッチドッグを再ロードするタスクやループを足さない。**
- `explicit_buy` は bootrom がウォッチドッグを止めるので、直後に必ず `supervisor::rearm()` (ticker.rs の buy 処理)。
- **長く動くタスクを足したら生存確認を付ける**: `health::Who` に項目を追加 → `WHO_COUNT` / `Who::ALL` / `label` /
  `Limits::TICKER` / `wdt_stage` + `STAGE_WDT_*` / `supervisor::on_frame` の `Stage` 対応、そのループで `supervisor::beat(Who::…)`。
  時々だけ動く処理 (設定ページの要求、`Who::Web`) は `Who::starts_parked` にして、処理中だけ `beat`、終わったら `supervisor::park`。
- wifi_ota は旧来の `ota::app::tbyb_watchdog_task` (buy 待ち中だけ 2 s ごとに延長) のまま。wifi_ota を再び manifest の bin に
  するなら、先に ticker と同じ早期ウォッチドッグと生存確認を入れること。

### 2.3 OTA 確認が接続後の最初の仕事

- `jobs_task` (`NetWork::Normal`) は `NET_READY` になったら最初に `run_ota_check` (`schedule_first_check_in(0)`)。
  写真の読み込み開始 (`slideshow::START`) も NTP / 天気 / 文字も、最初の OTA 確認の後。
- **OTA 確認より前に新しい処理を入れない。** 接続前の初期化 (SD、LCD、CYW43) に足すものは期限付き + `feed_init` 付きで。
- OTA 専用タスクは作らない (TLS バッファ ≈30 kB を 2 組置く RAM が無い)。取得タスク内で OTA が最優先。

### 2.3.1 設定ページのサーバ (0.5.0〜、`src/web/server.rs`、docs/settings-server.md)

- **取得タスクの中で 1 要求ずつ**。毎周の順番は OTA 確認 > 設定ページの要求 > NTP > 天気 > 文字。OTA 確認 / ダウンロードの間は
  動かない (同じタスク)。専用タスクにしない (作業領域は取得タスクの `NetBuffers` を借りる。新しい static は待ち受けソケットの 2 kB だけ)。
- **待ち受けは最初の OTA 確認が通ってから** (`OTA_PROVED` を見て `Server::new`)。**回復モードでは作らない**。
- 1 つの要求は必ず打ち切る: 読み書き 5 s、要求全体 15 s (写真の追加 60 s)、ヘッダ 2 kB、本文 4 kB / 115,254 B、SD の 1 回の操作 2 s
  (`sdcard::with_deadline`)。`Who::Web` (20 s) は処理中だけ監視 (進むたびに `beat`、終わったら `park`)。
- SD はスライドショーと `slideshow::SD_LOCK` で分け合う (待つのは 6 s まで)。書くときは `TICKER.NEW` → 読み戻し → `TICKER.BAK` →
  `TICKER.TXT` → 読み戻し。状態を変える要求はアクセスコード + Host / Origin の確認。新しい API もこの検査の内側に置く。

### 2.4 回復モード (`boot_policy::decide` → `Mode::Recovery`、`jobs_task` の `NetWork::Recovery`、`src/ui/recovery.rs`)

- 通常モードで `RECOVERY_AFTER` (2) 回続けて異常終了 → Wi-Fi + OTA だけで起動。`RECOVERY_OTA_INTERVAL_MS` (60 s) ごとに確認、
  新しい版が無ければ `RECOVERY_NORMAL_RETRY_MS` (10 分) 後に通常モードを 1 回試す。
- 使うのは ウォッチドッグ + 生存確認、LCD (黒地 `FONT_6X10`)、CYW43 + join + DHCP、OTA だけ。
  **SD / NTP / 天気 / 文字 / 写真 / AA 数字 / 東雲フォント / USB / 設定ページは使わない。回復モードに新しい処理を入れない。**
- Wi-Fi の資格情報は data 区画の写し (`src/persist.rs`、`boot_policy::Record`、CRC 付き)。通常モードが wifi.txt を読めたとき
  `persist::write_if_changed` で更新する。写しが無いときだけ SD の wifi.txt を期限 4 s で読む (`read_wifi_txt_only`)。

### 2.5 他方区画へ戻す + 入れない版

- 回復モードでも `FALLBACK_AFTER` (3) 回落ちたら `fallback_now` → SCRATCH0 に `fallback_marker` → `ab_boot::reboot_flash_update(他方区画)`。
  1 回だけ (`BootState::fell_back`)。
- 戻った版は `decode_fallback_marker` で原因の版を知り、`Record::blocked` に記録。`Record::allows` / `run_ota_check` の
  `mode.blocked` で、その版以下は二度と入れない。この経路と SCRATCH0 / SCRATCH1 の形式 (`BootState::encode`、0.4.1 の
  `LEGACY_MAGIC` も読む) を変えるときは互換性を保つ。

### 2.6 すべての I/O に打ち切り

- SD: `sdcard::set_deadline` (初期化 5 s。カード無しで embedded-sdmmc が ≈25 s 戻らなかった)。
- ネットワーク: `ota::app` の `MANIFEST_TIMEOUT` 30 s / `DOWNLOAD_TIMEOUT` 300 s / `SOCKET_TIMEOUT` 20 s / `DHCP_TIMEOUT` 20 s、
  NTP 5 s × 3 段 × 2 ホスト、天気 / 文字 20 s。**新しい I/O は内側で打ち切る** (外側の `with_timeout` は §3 の理由で避ける)。

## 3. メモリ / スタック

- **`scripts/stack-report.py` を必ず回し、margin ≥ 10 KB を保つ** (CI は margin < 0 でしか落ちないので 10 KB は手で守る)。
  `python3 scripts/stack-report.py target/thumbv8m.main-none-eabihf/release/ticker --path 6`
  (0.4.2: free 38,144 B / 最深 24,444 B / +13.7 kB。0.5.0: free 36,380 B / 最深 23,588 B / +12.8 kB。0.5.1: free 35,572 B / 最深 24,752 B / +10.8 kB — 10 kB まで残り 0.8 kB、次に RAM / スタックを増やす変更は先に削る所を探す。
  RAM を増やす変更は必ず前後を比較して PR に書く)。
  新しいタスクを足したら `ROOTS` に追加する。rustup の llvm-tools を使う (GNU objdump は ARM ELF を読めない)。
- スタックは SRAM8/9 まで (`memory.x` の `_stack_start` = 0x2008_2000)。下端は `supervisor::set_stack_limit` が MSPLIM に設定、
  溢れたら `STACK OVERFLOW` を記録してリセット。flip-link は使わない (RP2350 は RAM の下が XIP 窓で fault しない)。
- 大きな future (OTA 確認 / CYW43 起動 / 接続管理) を待つときは `noinline(..)` (bin からは `pico2w_300yen_lcd::noinline::noinline`) で包む (`src/noinline.rs`)。
  包まないと 1 つの poll フレームに局所変数が並んで 30 kB になった。
- **大きな future を `with_timeout` 等でもう 1 段包まない** (一旦スタックに作ってから移すので取得タスクの poll が ≈30 kB に膨らんだ)。
- 大きな `String<N>` / バッファを値で返したり局所に置いたりしない (0.4.0 は 2 kB の URL 一時領域が効いた)。static か呼び出し側の `&mut`。
- 回復モードを別タスクにしない (OTA future ≈15 kB のタスク領域が 2 つ分要る)。
- 表示 (`src/lcd/display.rs`): DMA が読むもの (画素、同期テーブル、制御ブロックの再ロード元) は **SRAM だけ**
  (フラッシュ消去中に XIP を読むと DMA チャネルが止まり砂嵐)。embassy の `Pio` / `Common` / `StateMachine` は drop しない
  (drop で GPIO の FUNCSEL が外れ白画面)。描いたら必ず `present()`。([docs/ota-design.md §4.1](../../../docs/ota-design.md))

## 4. ファームを触る PR の前に必ず

```sh
cargo build --release                                   # 全 bin
cargo build --release --bin ticker --features tbyb      # OTA で配るイメージ
cargo build --release --bin wifi_ota --features tbyb
cargo clippy --release --bin ticker --features tbyb     # 触ったファイルに新しい警告を出さない
(cd tools/ticker-tests && cargo test --release)         # boot_policy の単体 + boot_sim (起動の流れの模擬)
python3 scripts/stack-report.py target/thumbv8m.main-none-eabihf/release/ticker --path 6
```

- `tools/ticker-tests/src/boot_sim.rs` が全部通ること (0.5.0〜 段階 `Web` とその Crash / Hang も)。**起動時の段階を足したら** `St` / `NORMAL` (/ `RECOVERY`) /
  `stage_name` に足し、`broken_first_round_is_never_bought` 等で Crash / Hang を注入して「buy しない・旧版へ戻る・OTA 確認に届く」を確かめる。
- UI を変えたら `tools/ui-sim` でプレビュー (PNG / GIF) を作り、**OTA の前にユーザーへ見せる** ([docs/ui-sim.md](../../../docs/ui-sim.md))。
  設定ページ (`web/settings/index.html`) を変えたら `tools/settings-mock` の偽の端末で画面写真を撮って見せる。
  回復画面は `scenarios/recovery.json`、buy 待ちは `scenarios/pending.json`。
- PR 本文に: スタックの前後、ticker.bin のサイズ、boot_sim の結果、実機で未確認のこと。

## 5. リリース手順

1. `Cargo.toml` の `version` を上げる (IMAGE_DEF は x.(y×100+z)、`src/image_def.rs`。patch < 100)。docs の履歴も更新。
2. PR → CI 緑 (build ジョブ内のホストテストとスタック検査が Release の関門)。
3. rebase merge (`gh api -X PUT .../pulls/N/merge -f merge_method=rebase`)。
4. `release.yml` を dispatch:
   `gh api -X POST repos/Droplet-Collective/aki300yenLCD-raspico2w/actions/workflows/release.yml/dispatches -f ref=main -f 'inputs[version]=X.Y.Z'`
   (プロキシ経由のタグ push は 403。`gh pr create` / `gh release download` も GraphQL で 403 なので REST を使う)。
5. Release の `manifest.json` を確認: `bin` が OTA の bin (`ticker.bin`)、`size` と `sha256` がアセットと一致。
   **稼働中の機体は manifest の `bin` をそのまま取りに行く** (名前を変えると全台がそれに切り替わる)。
6. UF2 は `scripts/make-ota-image.sh` が `--abs-block 0x103FFF00` を付ける (RP2350-E10。付けないとテーブルのある機体で D&D が無言で失敗)。
7. コミットは fnc765 名義: `GIT_AUTHOR_NAME=fnc765 GIT_AUTHOR_EMAIL=84061221+fnc765@users.noreply.github.com`
   (+ `GIT_COMMITTER_*` 同じ) を毎回付ける (コンテナの `GIT_CONFIG_*` が local config を上書きするため)。

## 6. リリースの後

- ユーザーに伝える: buy 待ち中は状態行に `ticker vX.Y.Z via OTA slot A/B TBYB:pending N/180s wait:<項目>`、
  buy 後は `TBYB:bought OK`、OTA 行は `OTA: up to date (latest X.Y.Z)`。**写真を頼み**、届いたら表示を確認する。
- 実機で未確認のことを列挙して伝える (推測で「直った」と言わない)。
- 回復の試験: SD の `ticker.txt` に `debug_crash=boot` / `ota` / `slideshow` (buy 済みの通常起動でだけ効く)。
  2 回落ちて回復モード画面 → 60 s ごとの OTA 確認を確認。試験後は必ず行を消してもらう。

## 7. 既知の限界

- 両方の区画が壊れていて回復モードの OTA も通らない → USB (BOOTSEL + `ticker.uf2` の D&D) でしか直せない。
- 戻り先の版が入れない版の仕組みより古い (0.4.1) と、落ちる版を 10 分おきに入れ直しうる (どの周回でも OTA 確認はする)。
- buy 待ちでは ダウンロード → 書き込み の経路は試せない (戻り先の区画を壊すため)。
- 異常終了の回数は SCRATCH (電源断で 0)。TLS はサーバ証明書を検証しない (`TlsVerify::None`)。
- 一覧は [docs/ticker.md §8.5](../../../docs/ticker.md)。

## 8. 過去の事故 (同じ轍を踏まない)

- **0.4.0**: 背景バッファ 76.8 KB でスタックが 28 KB に減り、最初の TLS で溢れて `FRAME_WAKER` を破壊 → HardFault `loop {}`、
  buy 後でウォッチドッグ無し、OTA も同じ TLS 経路で自力更新不可 → USB 復旧。→ [docs/ticker.md §7.1 / §8.6](../../../docs/ticker.md)
- **0.2.x TBYB**: bootrom の TBYB ウォッチドッグは最大 16.7 s で join + DHCP が収まらず巻き戻った → 延長 + 締め切り。
  → [docs/wifi-ota.md §5.1](../../../docs/wifi-ota.md)
- **0.2.6 CYW43**: 温かい再起動で CYW43439 が状態を引き継ぎ DHCP が通らない → WL_REG_ON を 500 ms 落とす (`wifi::start`)。
  → [docs/wifi-ota.md §5.3](../../../docs/wifi-ota.md)
- **RP2350-E10**: テーブルのある A2 機体に絶対ブロック無しの UF2 を D&D しても何も起きない → `--abs-block 0x103FFF00`。
  → [docs/ota-setup.md §2.1](../../../docs/ota-setup.md)
- **LCD**: 同期テーブルが flash にあり explicit_buy で砂嵐 / `Pio` の drop で白画面 → §3 の表示の規則。
