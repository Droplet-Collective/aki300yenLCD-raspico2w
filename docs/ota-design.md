# Wi-Fi OTA (無線ファームウェア更新) 設計

Pico 2 W (RP2350) が GitHub Release から最新ファームウェアを HTTPS で取得し、
RP2350 bootrom の A/B パーティションと Try Before You Buy (TBYB) を使って
安全に入れ替えるための設計。第 1 段階はパーティション構成・版数・
起動スロット表示まで、第 2 段階 (`wifi_ota` bin、使い方は [wifi-ota.md](wifi-ota.md)) で
HTTPS ダウンロードとフラッシュ書き込みを実装した。節番号 `§x.y` は RP2350 データシート
(<https://datasheets.raspberrypi.com/rp2350/rp2350-datasheet.pdf>) を指す。
調査メモの全文は PR の説明に添付した `ota-research.md` を参照。

## 1. 目的と非目標

目的

- USB を挿さずに、電源と Wi-Fi だけでファームウェアを更新できる。
- 更新に失敗しても (電源断・書き込み不良・新版が起動しない) 必ず旧版で起動する。
- 更新の仕組みは bootrom の標準機能 (A/B 版数比較、FLASH_UPDATE 起動、TBYB) に
  乗せ、独自ブートローダを書かない。
- `picotool` での開発フローはそのまま使える (`cargo run` / `picotool load -f`)。

非目標 (今回扱わない)

- RP2350 のセキュアブート / 署名付きイメージ / OTP 書き込み (不可逆なので別途判断)。
- パーティションテーブル自体の更新 (slot 1 を使う A/B は将来課題)。
- Wi-Fi ファームウェア (cyw43 の 231 kB) の分離配布。当面はイメージに同梱。

## 2. 前提

| 項目 | 値 | 出典 |
|---|---|---|
| フラッシュ | 4 MB (W25Q32)、セクタ 4 kB、ページ 256 B | Pico 2 W データシート |
| SRAM | 512 kB + 4 kB × 2 | memory.x |
| 最大イメージ | `wifi_ota` (第 2 段階): text 619,340 + rodata 279,040 B ≈ **877 kB** (`wifi_status` は 419 kB) | `llvm-size` |
| RAM 空き | `wifi_ota`: 静的領域の終わり 0x200751B8 → スタック **44,616 B**。`wifi_status` は 112,860 B | `llvm-size` / `llvm-nm` |
| bootrom API | embassy-rp 0.9 `rom_data` に reboot / get_sys_info / get_partition_table_info / explicit_buy / flash_* が全てある | `embassy-rp-0.9.0/src/rom_data/rp235x.rs` |
| 配布 | GitHub Release (`v*` タグで CI が作成) のアセット | `.github/workflows/build.yml` |
| ツール | picotool 2.3.1 (partition create / load -p / uf2 convert) | `picotool help` |

## 3. パーティション構成

`partition/pico2w-ab.json` (→ `scripts/make-partition-table.sh` → `partition/pico2w-ab.uf2`)。

| 領域 | ストレージオフセット | サイズ | 用途 |
|---|---|---|---|
| slot 0 | 0x000000–0x000FFF | 4 kB | PARTITION_TABLE (§5.1.15) |
| slot 1 | 0x001000–0x001FFF | 4 kB | 予約 (テーブルの A/B 用。未使用) |
| P0 `app-a` | 0x002000–0x1E1FFF | 1920 kB | アプリ A。family `rp2350-arm-s`。S/NS/BL rw |
| P1 `app-b` | 0x1E2000–0x3C1FFF | 1920 kB | アプリ B。`link: ["a", 0]` で P0 の B (§5.1.7) |
| P2 `data` | 0x3C2000–0x3FCFFF | 236 kB | 将来の設定 / OTA マニフェスト保存用。family `data`、arm/riscv 起動では無視 |
| 未区画 | 0x3FD000–0x3FFFFF | 12 kB | 空き (picotool が BTStack flash bank / RP2350-E10 用に警告する末尾 3 セクタ) |

根拠

- 1 スロット 1920 kB は第 2 段階の `wifi_ota` (877 kB、TLS/HTTP/rsa 込み) の 2.2 倍。
- データシート §5.10.4 の例 (2044 kB × 2) から、将来の設定保存領域として `data` を
  切り出した。owner リンク (§5.1.18.1) は付けず、A/B 共通の領域として使う。
- `memory.x` の `FLASH LENGTH` を 1920K にした。イメージは常に 0x10000000 で
  リンクし、どのスロットに置かれても QMI アドレス変換 (§5.1.19) で 0x10000000 に
  見えるため、スロット別ビルドは不要。LENGTH を絞ることでスロット超過をリンク時に
  検出する。`.start_block` / `.end_block` の配置は embassy-rp の memory.x と同じで
  変更不要 (IMAGE_DEF は先頭 4 kB 内 0x10000114 にある)。
- パーティションテーブル無し (従来通り先頭に直置き) でも各 bin はそのまま起動する。

## 4. 起動と版数選択

### IMAGE_DEF と版数

- embassy-rp の既定 IMAGE_DEF を `imagedef-none` feature で外し、`src/image_def.rs` の
  `firmware_image_def!()` マクロで各 bin に **VERSION 項目付き IMAGE_DEF** (§5.9.2.1,
  `Block<3>` = IMAGE_TYPE + VERSION) と picotool 用 binary_info (名前・版数・ビルド種別)
  を埋め込む。bin 側に static を置くので、リンカがライブラリのオブジェクトを捨てて
  IMAGE_DEF が消える事故がない (release/dev 両方で `picotool info -a` に出ることを確認)。
- 版数は `Cargo.toml` の `version` から生成: major はそのまま、minor = Cargo minor × 100 +
  patch (`0.1.0` → IMAGE_DEF `0.100`、`0.1.1` → `0.101`)。bootrom は
  (rollback).major.minor を辞書順比較する (§5.1.6) ので patch < 100 で semver 順と一致。
  rollback 版数はセキュア化していないチップでは無視されるため付けない (§5.1.11)。
- `--features tbyb` で IMAGE_TYPE に TBYB フラグ (0x8000, §5.9.3.1) を立てる。
  OTA で書き込むイメージだけがこれを使う。

### bootrom の選択規則 (§5.1.13, §5.1.16, §5.1.17)

1. 通常起動: A/B の両方に有効な IMAGE_DEF があれば版数の高い方。**TBYB フラグ付きは
   非 TBYB より常に劣後** (FLASH_UPDATE 起動以外では選ばれない)。
2. FLASH_UPDATE 起動 (`reboot(0x0004, ..., p0 = 更新領域先頭)`): p0 が区画先頭に一致
   すればその区画を版数に関係なく優先。ダウングレード時は他方区画の先頭セクタを消して
   永続化 (非 TBYB は起動時に、TBYB は explicit_buy 時に)。
3. TBYB イメージは 16.7 s (24 bit × 1 µs) のウォッチドッグ下で起動し、
   `explicit_buy()` (§5.4.8.4) で確定。確定時に自分の TBYB フラグを消し、
   他方区画の先頭セクタを消去する。呼ばなければリブートして旧イメージへ戻る。

### 実行中の自己認識 (`src/ab_boot.rs`)

- `get_sys_info(BOOT_INFO)` (§5.4.8.17) から起動種別・起動区画・TBYB 状態
  (`BUY_PENDING`) を得る。`flash_runtime_to_storage_addr(0x10000000)` (§5.4.8.13) で
  自区画の先頭オフセットも得て二重に確認する。
- `get_partition_table_info` (§5.4.8.16) でテーブルを読み、A/B/名前を表示に使う。
- `pick_ab_partition` は buy 待ち中に呼ぶと explicit_buy が使う消去アドレスを壊す
  (pico-sdk `rom_pick_ab_update_partition` の注記) ため **使わない**。
- `explicit_buy` は IMAGE_DEF を含む自セクタを消去・再書き込みするので、割り込み禁止
  (フラッシュ上の ISR を走らせない) で呼び、終了後に XIP キャッシュをフラッシュする。
  LCD の DMA は RAM しか読まない (§4.1 の条件) ので止めなくてよい。

### 4.1 表示とフラッシュ操作の共存

LCD 走査 (`src/lcd/display.rs`) は PIO0 SM0/SM1 + DMA CH0〜CH3 の **CPU 不介入の自走リング**
(`CH0 → CH2 → CH0`、`CH1 → CH3 → CH1` の chain_to で毎フレーム先頭アドレスを書き戻す) で、
割り込みハンドラや async タスクによるフレーム毎の再武装は無い。したがって割り込み禁止で CPU が
数百 ms 止まっても走査は続く。**条件は「DMA が読むメモリが全て SRAM にあること」**である。

- フラッシュ消去・書き込み中は QMI がダイレクトモードになり、XIP 窓 (0x1000_0000〜) への
  **DMA アクセスはバスフォールト**を返す (§5.4.8.10, §12.14.5)。バスエラーを受けた DMA チャネルは
  エラーフラグを消すまで停止したまま (§12.6.7.1) で、chain も起きない。
- 第 1 段階の実機試験でこれが起きた: SM1 (HSYNC/VSYNC) 用のタイミングデータが不変 `static` として
  `.rodata` = フラッシュに置かれていたため、`explicit_buy` のセクタ消去で CH1 が停止し、画素 (SRAM) だけ
  出続けて砂嵐になった (電源再投入まで復帰しない)。修正で `#[link_section = ".data…"]` により RAM へ
  移し、フロントバッファ (bss)・アドレス書き戻し元 (bss) と合わせて DMA の読み出し元は全て SRAM になった。
- 停止時間の目安 (W25Q32): セクタ消去 typ 45 ms / max 400 ms、256 B ページ書き込み typ 0.7 ms。
  SM0/SM1 の TX FIFO は 8 ワードしかない (SM1 で 4 行 ≈ 0.6 ms) ので、FIFO で吸収する設計は不可能。
  再武装を割り込みに頼る設計も、フラッシュ操作中は割り込み禁止なので不可。DMA リング + 全 SRAM が唯一の解。
- 描画は 400×96 のバックバッファに行い、`Display::present()` が垂直ブランキング (16 行 ≈ 2.4 ms) 中に
  フロントへコピーする (フレーム先頭は CH2 完了の `DMA_IRQ_1` で検出。この割り込みは描画の
  ペース合わせ専用で、走査維持には関与しない)。走査中のバッファへ直接描いていた以前の方式は、
  再描画ごとに 1 フレームだけ新旧・黒・位置ずれの行が混ざって見えていた。
- 第 2 段階の OTA 書き込み (約 100 セクタ消去 + 約 1,700 ページ書き込み) も同じ条件で走査に影響しない。
  cyw43 側の割り込み遅延は別問題 (§5 補足)。

## 5. 更新フロー (第 2 段階 `wifi_ota` で実装済)

```
[起動] → 自己診断 (LCD 走査開始、SD、Wi-Fi 接続) → TBYB なら explicit_buy
   ↓ 一定時間ごと (例: 起動 1 分後、その後 1 時間ごと)
[1] 版数確認   GET https://github.com/<o>/<r>/releases/latest/download/manifest.json
               (302 → objects.githubusercontent.com へ再接続) → {version, bin, size, sha256}
               → FIRMWARE_VERSION と semver 比較。新しくなければ終了
[2] 書き込み先  BOOT_INFO / storage offset から「今起動している区画」を求め、
               他方 (P0↔P1) を対象にする
[3] 転送       bin を 4 kB ずつ受信 → 対象セクタを erase → 256 B ページ単位で program
               (embassy_rp::flash::Flash::blocking_erase/write、in_ram + 割り込み禁止)
               受信しながら SHA-256 を計算。size 超過・切断は中断 (対象区画は壊れて
               いてもよい: 起動側は無傷)
[4] 検証       SHA-256 一致 + 書き戻し読み比較。読み戻しは ATRANS を通さない窓
               0x1C000000 + オフセット (§2.2 XIP_NOCACHE_NOALLOC_NOTRANSLATE) で行う
               (0x10000000 窓は自区画しか見えない)
[5] 再起動     reboot(FLASH_UPDATE | NO_RETURN, 100 ms, 0x10000000 + 対象区画オフセット, 0)
[6] 新版起動   TBYB 付きなので bootrom がウォッチドッグ (16.7 s) 下で起動。buy 待ちの間は
               2 s ごとに WATCHDOG.LOAD を再ロードして延長。自己診断 OK → explicit_buy → 確定。
               起動 120 s までに NG → 延長をやめて旧版に戻る。ハング → 16.7 s で旧版に戻る
```

補足

- 実装は `src/ota/` (manifest / http / slot) と `src/bin/wifi_ota.rs`。書き込みのアドレスは
  `embassy_rp::flash::Flash::blocking_erase / blocking_write` の offset = bootrom
  `flash_range_erase / flash_range_program` の addr = **ストレージアドレス** (§5.4.8.10/11
  「offset from start of flash」、ATRANS は掛からない)。読み戻しは `0x1C000000 + オフセット`
  (§2.2.2 Table 10 XIP_NOCACHE_NOALLOC_NOTRANSLATE)。`Flash::blocking_read` は 0x10000000 の
  変換付き窓を読むので他方区画には使えない。
- [3] の先頭セクタは受信前に消去して無効化し、[4] の検証が全て通ってから最後に書く。
- 対象区画に manifest と同じ SHA-256 のイメージが既にあれば「前回 TBYB で buy されずに
  戻ってきた」と判断してダウンロードせず、最初にそう判定してから 10 分後に FLASH_UPDATE 起動を
  再試行する (v0.2.3 までは 60 s ごとの確認で再試行時刻が毎回延び、再試行しなかった)。
- 自己診断 [6] は「LCD 走査中 (起動 2 s 以上) + Wi-Fi join + DHCP で IP 取得」。bootrom の
  ウォッチドッグは 16.7 s (24 bit × 1 µs、ハードウェア上限) で join + DHCP に足りないことが
  実機で分かった (v0.2.2) ので、buy 待ちの間は `WATCHDOG.LOAD = 0xFFFFFF` を 2 s ごとに書いて
  延長する (§5.1.17 が認める方法。`CTRL` / `SCRATCH` は触らない)。起動 120 s を締め切りとし、
  それまでに通らなければ延長をやめて旧版へ戻す。詳細は
  [wifi-ota.md §5.1](wifi-ota.md#51-自己診断とウォッチドッグ)。
- フラッシュ操作中 (セクタ消去 数十〜数百 ms) は XIP が止まり、DMA からの XIP 読み出しは
  バスフォールトになる。LCD は §4.1 の条件 (DMA の読み出し元が全て SRAM) を満たしているので
  乱れない。cyw43 側は PIO SPI の DMA が停止中に完了しても割り込みが遅れるだけで、
  erase を 1 セクタずつ挟めばドライバのタイムアウト内に収まる見込み (要実測)。
- 版数比較は manifest の `version` (例 `0.1.1`) と `FIRMWARE_VERSION` を semver で比較。
  IMAGE_DEF の版数はそこから機械的に決まるので bootrom の選択と食い違わない。
- 対象区画の先頭セクタ (IMAGE_DEF) は最後に書く。途中で電源が落ちても bootrom は
  不完全なイメージを認識しない。

## 6. 失敗時の挙動

| 事象 | 結果 |
|---|---|
| ダウンロード中に切断・電源断 | 対象区画のみ不完全。起動側は無傷。次回また試す |
| SHA-256 不一致 | FLASH_UPDATE 再起動しない。対象区画の先頭セクタを消しておく |
| 新版が起動しない / ハング / パニック | bootrom のウォッチドッグ (16.7 s) で旧版へ。新版は TBYB のまま残り通常起動では選ばれない (延長タスクも止まるので、延長中のハングでも同じ)。v0.2.4 から新版は進行 / panic / HardFault を WATCHDOG.SCRATCH5〜7 に記録し、旧版がそれを LCD に出す ([wifi-ota.md §5.2](wifi-ota.md#52-起動診断-v024-巻き戻りの原因を旧版の画面で読む)) |
| 新版は起きるが Wi-Fi 等の自己診断 NG | 起動 120 s まではウォッチドッグを延長して待つ。それでも通らなければ延長をやめ、explicit_buy も呼ばない → 最長 16.7 s 後に同上 |
| 温かい再起動で CYW43439 が接続中の状態を引き継ぎ、join は通るが DHCP が通らない | `reboot(FLASH_UPDATE)` は RP2350 だけをリセットし、CYW43439 は通電・接続したまま。cyw43 の init は WL_REG_ON を 20 ms しか落とさないため内部状態が残ることがある (v0.2.5 の実機: `join1 dhcpto1` のまま 120 s で巻き戻り)。v0.2.6 から起動時に WL_REG_ON を 500 ms 落としてコールドスタートさせ、再起動前にも `leave()` + 電源断、DHCP タイムアウトごとに再 join する ([wifi-ota.md §5.3](wifi-ota.md#53-温かい再起動と-cyw43439-の状態-v026-join-は通るのに-dhcp-が通らない)) |
| explicit_buy が失敗 (負値) | LCD にエラー表示。bootrom は explicit_buy の冒頭でウォッチドッグを止めるので自動では戻らず、次の電源投入 (通常起動) で旧版が選ばれる |
| 旧版より低い版数を書いた (ダウングレード) | FLASH_UPDATE で起動し、buy 時に他方先頭セクタが消える。以後は低い版が起動 |
| パーティションテーブル破損 | ハッシュ付きなので bootrom が無効と判断 → 起動不能。復旧は BOOTSEL で再投入 |

## 7. ビルドと配布

- 各 bin は 1 つの ELF から `scripts/make-ota-image.sh` で
  `<name>.bin` (objcopy -O binary、OTA 配布用)、`<name>.uf2` (family rp2350-arm-s)、
  `<name>.sha256` を作る。スロット依存はない。
- CI (`build.yml`) は全 bin の ELF/UF2/.bin と `pico2w-ab.uf2` をアーティファクトに
  入れ、`v*` タグでは Release に UF2 / .bin / .sha256 を添付する。
- 第 2 段階の Release アセット: `wifi_ota.bin` / `wifi_ota.uf2` (`--features tbyb`)、
  `wifi_ota-plain.uf2` (TBYB 無し、Wi-Fi 未設定の機体への初回用)、
  `manifest.json` (`scripts/make-manifest.sh`:
  `{"version":"0.2.0","bin":"wifi_ota.bin","size":…,"sha256":"…"}`)。アセット名は版数を含めない
  (実機は常に `releases/latest/download/<name>` を取る)。
  タグと `Cargo.toml` の version の一致は CI で検査する (不一致ならビルド失敗)。
- 版数を上げる手順: `Cargo.toml` の `version` を変更 → ビルド → `picotool info` で
  `version: 0.101` などを確認 → `git tag v0.1.1`。

## 8. 初回セットアップ (詳細は docs/ota-setup.md)

1. BOOTSEL で接続し `picotool load -v partition/pico2w-ab.uf2` (1 回だけ)。
2. `picotool load -v -x -t elf target/thumbv8m.main-none-eabihf/release/ota_selftest`。
3. 以後は `picotool load -f ...` / `cargo run` / OTA のいずれでも更新できる。

## 9. セキュリティ

- TLS: embedded-tls は TLS 1.3 のみ (GitHub 各ホストは対応)。`TlsVerify::None` は
  経路上の攻撃者が任意のイメージを配れる (=任意コード実行) ため、公開ネットワークで
  使うなら証明書検証が必要。CA ピン留めは GitHub 側の CA 変更で更新が止まるリスクが
  ある。manifest の SHA-256 は破損検出であり、同じ経路で取る限り改竄対策にはならない。
- **第 2 段階の決定: `TlsVerify::None` (検証なし)、自宅 LAN 限定。** 理由は
  [wifi-ota.md §6](wifi-ota.md#6-セキュリティ-重要): アセット配信ホスト `*.githubusercontent.com`
  は Let's Encrypt の RSA 4096 証明書で、embedded-tls 0.18 は `rsa` feature (alloc 必須) 無しでは
  ハンドシェイクすら成立しない。reqwless 0.14 の証明書検証 (rustpki) はホスト名を CN 完全一致で
  しか見ないためワイルドカード証明書を受け付けず、webpki 経路は ring 依存で使えない。
  第 3 段階で manifest への Ed25519 署名 (公開鍵をファームに埋め込み) を実装する。
  RP2350 のセキュアブート (OTP) は不可逆なので採用しない。
- 更新元の URL・リポジトリ名はファームウェアに固定 (SD カードからは読まない)。

## 10. 段階計画

| 段階 | 内容 | 状態 |
|---|---|---|
| 1 | パーティションテーブル、版数付き IMAGE_DEF、`ab_boot` ラッパ、`ota_selftest` bin、スクリプト、CI、文書。A/B 選択・FLASH_UPDATE 起動・TBYB + explicit_buy は v0.1.0→v0.1.1 で実機確認済。表示層をフラッシュ操作と共存できる形に修正 (§4.1、v0.1.4/v0.1.5 で実機確認済) | 実装済・実機確認済 |
| 2 | `wifi_ota` bin: manifest 取得 → bin ダウンロード → 他方区画へ書き込み → 検証 → FLASH_UPDATE → 自己診断 → buy。LCD への進捗・版数表示、60 s 周期 + バックオフ。TLS は検証なし (§9)。CI が Release に `wifi_ota.bin` + `manifest.json` を添付 | 実装済 (実機未確認、[wifi-ota.md §8](wifi-ota.md#8-未確認事項-実機)) |
| 3 | manifest への Ed25519 署名 / 失敗回数の記録 (data 区画) / 巻き戻し検出の永続化 | 未着手 |

## 11. 未確認事項

- ~~実機での bootrom 挙動全般 (版数選択、FLASH_UPDATE、TBYB のウォッチドッグ、
  explicit_buy の戻り値)~~ → `ota_selftest` v0.1.0〜v0.1.5 で確認済 (docs/ota-setup.md)。
- `reboot(FLASH_UPDATE)` を **ファームウェアから** 呼んだときの p0 (`0x10000000 + オフセット`) が
  正しいか (picotool `-x` と同じ形式にしている。ストレージオフセットそのままの可能性もある)。
- `flash_runtime_to_storage_addr` の戻り値が 0x10000000 を含むか (両方に対応済)。
- ~~explicit_buy 中に LCD の DMA/PIO が乱れないか~~ → 原因 (§4.1) を修正し v0.1.4/v0.1.5 で乱れないことを確認済。
- ~~embedded-tls の証明書検証 (webpki) が GitHub の証明書チェーン (ECDSA/RSA) で使えるか~~ → 使えない (§9)。
- cyw43 ドライバがフラッシュ消去中の割り込み遅延に耐えるか (第 2 段階の実機試験で確認)。
- ~~第 2 段階の HTTPS 取得・書き込み・検証・FLASH_UPDATE・自己診断の一連の流れ全体~~ →
  0.2.1 → 0.2.2 で HTTPS 取得・書き込み・検証・FLASH_UPDATE 起動まで実機確認済。自己診断は
  DHCP 待ちで 16.7 s を超えて巻き戻ったため、v0.2.3 でウォッチドッグの延長を追加
  ([wifi-ota.md §5.1](wifi-ota.md#51-自己診断とウォッチドッグ)、§8)。
