# Wi-Fi OTA (第 2 段階): `wifi_ota` で GitHub Release から自己更新する

設計の全体は [ota-design.md](ota-design.md)、パーティションテーブルの導入と A/B・TBYB の
実機確認は [ota-setup.md](ota-setup.md)。ここでは `wifi_ota` bin の使い方と挙動をまとめる。

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
[7] 新版: LCD 走査 + Wi-Fi join + DHCP 完了 → explicit_buy → 確定。通らなければ旧版へ戻る
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
wifi_ota v0.2.1 via OTA  slot B (P1 app-b)  tbyb-build:yes  TBYB: bought OK ← 行 1: 自分の版数 / 区画 / TBYB
OTA: 0.2.0 -> 0.2.1 downloading 45%  196608/435200 B                       ← 行 2: OTA の状態
[====================                          ]                           ← 進捗バー (ダウンロード中のみ)
 SSID / RSSI 棒グラフ (上位 5 件)
```

行 1 の `via OTA` は、今動いているイメージが `reboot(FLASH_UPDATE)` で起動されたとき
(= OTA で書き込んだ側の区画から起動したとき) だけ版数の隣に出る (BOOT_INFO の boot_type)。
USB で入れた版では出ないので、OTA 更新が実際に反映されたかを版数と合わせて一目で確認できる。

行 1 の TBYB 表示 (ota_selftest と同じ色分け):

| 表示 | 意味 |
|---|---|
| `TBYB: no` (灰) | TBYB でない通常起動 |
| `TBYB: pending (buy after Wi-Fi up)  WDT 12.3s` (黄) | TBYB 起動。Wi-Fi + DHCP が通れば buy。`WDT` は bootrom のウォッチドッグ残り秒 |
| `TBYB: bought OK` (緑) | explicit_buy 成功。以後この版が通常起動で選ばれる |
| `TBYB: buy FAILED rc=-N` (赤) | explicit_buy 失敗。ウォッチドッグで旧版へ戻る |

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
| `OTA: 0.2.1 already in slot B but was rolled back; retry boot in 590s` (赤) | 前回の TBYB 起動で buy されなかった (§5) |
| `OTA: TLS failed, retry in 120s` (赤) | 失敗。理由 (`DNS failed` / `network error` / `TLS failed` / `bad HTTP response` / `HTTP 5xx` / `bad manifest.json` / `bad image size` / `size mismatch` / `sha256 mismatch` / `flash readback mismatch` / `flash error` / `timeout`) とバックオフ後の再試行時刻 |

## 5. 失敗時の挙動

| 事象 | 結果 |
|---|---|
| DNS / TCP / TLS / HTTP エラー、タイムアウト (manifest 30 s、ダウンロード 300 s、ソケット無通信 20 s) | LCD に表示。60 s → 120 s → … → 最大 10 min のバックオフで再試行。成功したらバックオフは 60 s に戻る |
| ダウンロード途中で切断・電源断 | 対象区画は先頭セクタを消した状態 (無効)。起動側は無傷。次回また最初から |
| 受信サイズ / SHA-256 / 読み戻しの不一致 | 対象区画の先頭セクタを消して終了。再起動しない。バックオフ後に再試行 |
| 新版が起動しない / ハング / Wi-Fi に繋がらない | 16.7 s のウォッチドッグで旧版へ。旧版は manifest と対象区画の内容が一致することから「巻き戻された」と判断し、10 分後に一度だけ FLASH_UPDATE 起動を再試行 (その後も同じ) |
| explicit_buy 失敗 | LCD に表示。ウォッチドッグで旧版へ |
| 電源断が「書き込み完了〜再起動」の間に起きた | 対象区画は有効な TBYB イメージ。通常起動では選ばれないが、次回起動時の確認で「巻き戻された」扱いになり 10 分後に FLASH_UPDATE 起動する |

`wifi_ota` はウォッチドッグを一切叩かない (embassy の `Watchdog` も使わない) ので、TBYB の
16.7 s は bootrom の設定どおりに働く。

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
| `.data` + `.bss` + `.uninit` | 580 + 478,064 + 1,024 B ≈ **468 kB** | LCD フロント 230,068 + バック 153,600、TLS 受信 16,640 + 送信 3,072、HTTP ヘッダ 4,096 + 受信単位 2,048、URL 2,048、TCP 4,096 + 2,048、セクタ作業 8,192、ヒープ 8,192、main タスク 15,912、cyw43 12,688、embassy-net 3,832 など |
| 静的領域の終わり | `0x200751B8` | |
| スタック (0x20080000 まで) | **44,616 B ≈ 43.6 kB** | TLS ハンドシェイク (p256) と rsa の一時領域を含めて足りる見込み。実測はまだ |

`wifi_status` の空きは 0x20080000 − 0x20064724 = 112,860 B。

## 8. 未確認事項 (実機)

- HTTPS 取得全般: GitHub のリダイレクト、Location の長さ (release-assets は JWT 付きで 1.5 kB 前後)、
  TLS 1.3 ハンドシェイク (RSA 4096 の CertificateVerify を `NoVerify` で受ける)、ダウンロード速度。
- フラッシュ書き込み中の cyw43 (割り込み遅延) とダウンロードの共存。
- `reboot(FLASH_UPDATE)` の p0 が `0x10000000 + オフセット` で正しいか (ota_selftest の
  picotool `-x` と同じ形式。ストレージオフセットそのままの可能性が残る)。
- スタック使用量 (43.6 kB の余裕で足りるか)。
- 「巻き戻し」検出と 10 分後の再試行が意図どおり動くか。

## 9. リリース履歴

| 版 | 内容 |
|---|---|
| 0.2.0 | 初版 (wifi_ota の OTA 機能、release.yml) |
| 0.2.1 | OTA 更新テスト用。FLASH_UPDATE 起動時に行 1 の版数の隣へ `via OTA` を表示 |
