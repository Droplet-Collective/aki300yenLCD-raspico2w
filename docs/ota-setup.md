# OTA 第 1 段階: パーティションテーブル導入と A/B・TBYB の実機確認手順

設計は [ota-design.md](ota-design.md)。ここでは実機で 1 回だけ行うパーティション
テーブルの導入と、`ota_selftest` bin を使って bootrom の A/B 選択と TBYB を
Wi-Fi 無しで確認する手順をまとめる。

## 用意するもの

- picotool 2.x (`.cargo/config.toml` の runner と同じもの)
- 以下のファイル。CI アーティファクト `firmware-<sha>` にも同じ名前で入っている。
  - `partition/pico2w-ab.uf2` … `scripts/make-partition-table.sh` で生成
    (`PICOTOOL=/path/to/picotool` で picotool を指定できる)
  - `ota_selftest` の ELF (`cargo build --release --bin ota_selftest`) または
    `ota_selftest.uf2` (`scripts/make-ota-image.sh <elf> out/`)

## 1. パーティションテーブルの導入 (1 回だけ)

1. BOOTSEL を押しながら USB を挿す。
2. テーブルを書く。family `absolute` なのでフラッシュ先頭 (slot 0) に入る。

   ```sh
   picotool load -v partition/pico2w-ab.uf2
   picotool partition info
   ```

   期待する表示:

   ```
   partition 0 (A):       00002000->001e2000 ... "app-a", uf2 { 'rp2350-arm-s' }
   partition 1 (B w/ 0):  001e2000->003c2000 ... "app-b", uf2 { 'rp2350-arm-s' }
   partition 2 (A):       003c2000->003fd000 ... "data",  uf2 { 'data' }, arm_boot 0
   ```

3. 注意: 先頭 4 kB が上書きされるので、それまで先頭に直置きしていたファームウェア
   (旧 `wifi_status` など) は起動しなくなる。続けて手順 2 でアプリを書く。
   テーブルを外して元に戻すときは `picotool erase -a` の後に従来どおり UF2 を書く。

## 2. アプリの書き込み

BOOTSEL のまま:

```sh
picotool load -v -x -t elf target/thumbv8m.main-none-eabihf/release/ota_selftest
```

- テーブルがある機体では、family `rp2350-arm-s` のイメージは bootrom の規則で
  **今起動していない方の A/B 区画**に入る (データシート §5.1.18)。空の状態では
  **最初の書き込みは B (P1) に入る** (§5.10.4 NOTE) ので、LCD に `slot B` と出ても正常。
  区画を指定したいときは `-p 0` / `-p 1`。
- `-x` は「フラッシュ更新起動」(FLASH_UPDATE) でリセットする。以後は実行中の
  ファームが picotool の USB reset interface を持つので `picotool load -f ...` や
  `cargo run --release --bin ota_selftest` (runner は `-u -v -x`) で書き換えられる。

## 3. LCD の読み方

```
OTA selftest v0.1.0  IMAGE_DEF 0.100  tbyb-build:no                  up 12s
booted: slot B (P1 app-b)  storage 0x001E2000  type FLASH_UPDATE(4)
boot_info: part=1 tbyb=0x00 diag=0x4C4D0021 p0=0x101E2000
TBYB: not a TBYB boot (nothing to buy)   WDT: off
partition table: 3 partitions
 P0 app-a  0x002000-0x1E1FFF  1920K  arm-s  A
 P1 app-b  0x1E2000-0x3C1FFF  1920K  arm-s  B of P0        ← 起動中の区画は緑
 P2 data   0x3C2000-0x3FCFFF   236K  data   A (not bootable)
```

| 行 | 意味 |
|---|---|
| 1 | `Cargo.toml` の版数、IMAGE_DEF に載った版数 (minor×100+patch)、TBYB フラグ付きビルドか、起動からの秒数 |
| 2 | 起動スロット (0x10000000 が対応するストレージオフセットから判定) と bootrom の起動種別 (`NORMAL`/`BOOTSEL`/`FLASH_UPDATE`…) |
| 3 | `get_sys_info(BOOT_INFO)` の生値。`tbyb=0x01` なら buy 待ち、`0x04` は他方区画の先頭セクタを消去済み |
| 4 | TBYB の状態: `not a TBYB boot` / `pending -> buy in 2.5s` (黄) / `bought OK` (緑) / `FAILED rc=…` (赤) / `skip-buy build` (赤)。`WDT:` は bootrom が仕掛けたウォッチドッグの残り秒 |
| 5〜 | パーティションテーブル。`storage` を含む区画を緑で表示 |

`partition table: error -14 (PRECONDITION_NOT_MET)` はテーブル未ロード
(テーブル無しで先頭に直置きした場合など)。`storage err` も同様。

## 4. A/B + TBYB を picotool で模擬する

Wi-Fi を使わずに「新版を他方スロットへ書き、FLASH_UPDATE で起動し、自己診断後に確定
する」流れを確認する。以下は手順 2 で B に v0.1.0 が入っている前提。

### 4.1 新版を TBYB で入れて確定させる

1. `Cargo.toml` の `version` を `0.1.1` にしてビルド (TBYB フラグ付き):

   ```sh
   cargo build --release --bin ota_selftest --features tbyb
   picotool info -a -t elf target/thumbv8m.main-none-eabihf/release/ota_selftest
   #  version: 0.101 / tbyb: not bought と出る
   ```

2. 実行中の機体へ書く (`-f` で BOOTSEL へ移行、`-x` で FLASH_UPDATE 起動):

   ```sh
   picotool load -f -v -x -t elf target/thumbv8m.main-none-eabihf/release/ota_selftest
   ```

   B が起動中なので A (P0) に入る。`picotool partition info -m rp2350-arm-s` で行き先を
   事前に確認できる。

3. LCD: `slot A`, `IMAGE_DEF 0.101`, `tbyb-build:yes`, `type FLASH_UPDATE(4)`,
   `TBYB: pending -> buy in 3.0s`, `WDT: 16.x s left` → 3 秒後 `TBYB: bought OK`。
4. 電源を入れ直す (または `picotool reboot -f`)。`slot A`, `0.101`, `type NORMAL(0)`,
   `TBYB: not a TBYB boot` になれば、bootrom が確定済みの新版を版数で選んでいる。

### 4.2 自己診断に失敗した (buy しない) 場合の巻き戻し

1. `version` を `0.1.2` にして `--features tbyb,skip-buy` でビルドし、4.1 と同様に
   `picotool load -f -v -x -t elf ...`。今度は B (P1) に入る。
2. LCD: `slot B`, `0.102`, `TBYB: pending, skip-buy build -> rollback by WDT` (赤)、
   `WDT:` が減っていく。約 16.7 秒後にリセットし、`slot A`, `0.101`, `type NORMAL(0)`
   に戻れば巻き戻し成功。
3. B には TBYB フラグ付きの 0.102 が残るが、通常起動では選ばれない。次の書き込みも
   B が対象になる。

### 4.3 ダウングレード

`version` を `0.1.0` に戻し (TBYB なし) `picotool load -f -v -x` すると、FLASH_UPDATE
起動で低い版数が優先され、その際に他方 (0.101) の先頭セクタが消される (§5.1.16)。
以後は 0.100 だけが起動する。TBYB 付きで同じことをすると消去は buy 時に行われる。

### 4.4 起動診断

起動に失敗したときは BOOTSEL に落ちるので、`picotool info -d`、
`picotool partition info`、`picotool reboot -g 0` (P0 の診断) を使う。
`ota_selftest` の 3 行目 `diag=` は前回起動の診断ワード (下位 16 bit = A、上位 = B。
0x40 CHOSEN、0x4000 IMAGE_LAUNCHED、0x8000 IMAGE_CONDITION_FAILURE など §5.4.8.17)。

## 5. 従来の使い方との関係

- 全 bin に版数付き IMAGE_DEF が入ったので、パーティションテーブルの有無に関係なく
  `picotool info` で名前と版数が見える。テーブル無しで先頭に直置きしても起動する。
- `memory.x` の FLASH は 1 スロット分 (1920K) に絞った。これを超えるとリンクエラーになる。
