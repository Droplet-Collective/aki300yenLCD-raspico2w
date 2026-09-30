# CLAUDE.md

Raspberry Pi Pico 2 W (RP2350) + 300 円 LCD (400×96) のファームウェア (Rust / Embassy)。
GitHub Release の `manifest.json` から Wi-Fi OTA で自己更新する。

## 最重要ルール

**OTA で配るファームや `src/ota`・`src/boot_policy.rs`・`src/supervisor.rs`・`src/lcd`・`src/web` (設定ページ)・メモリ配置・リリースワークフローを
触るときは、必ず最初に [`.claude/skills/ota-firmware/SKILL.md`](.claude/skills/ota-firmware/SKILL.md) を読んで従うこと。**

どんな壊れ方をした版でも「ウォッチドッグで再起動 → 最新ファームを確認 → 更新」まで必ず進むこと (OTA 到達保証) が絶対条件。
これを壊す変更は出さない。

## 概要

- bin (`src/bin/`): `ticker` (Release の OTA イメージ、`--features tbyb`)、`wifi_ota` (OTA の最小構成)、`wifi_status`、
  `ota_selftest` / `ota_selftest_min`、`sd_bmp_viewer`、`layer*` (LCD 駆動の検証用)。一覧は [README.md](README.md)。
- 共用ライブラリ: `src/ota/` (OTA + TBYB)、`src/web/` (設定ページの HTTP サーバ、0.5.0〜、[docs/settings-server.md](docs/settings-server.md))、`src/boot_policy.rs` (起動の方針、buy 条件)、`src/supervisor.rs` (ウォッチドッグ、
  スタック)、`src/lcd/` (PIO + DMA 走査)、`src/ui/` (描画、`tools/ui-sim` と共用)。
- ドキュメントは `docs/` (日本語): OTA 到達保証は [docs/ticker.md §8](docs/ticker.md)、設計は [docs/ota-design.md](docs/ota-design.md)、
  画面シミュレータは [docs/ui-sim.md](docs/ui-sim.md)。
- ホストのテスト: `tools/ticker-tests` (boot_sim を含む)、画面プレビュー: `tools/ui-sim`、スタック: `scripts/stack-report.py`、
  設定ページのプレビュー: `tools/settings-mock` (偽の端末 + Playwright の画面写真)。

## git / リリース

- コミットは **fnc765 名義**: 毎回 `GIT_AUTHOR_NAME=fnc765 GIT_AUTHOR_EMAIL=84061221+fnc765@users.noreply.github.com`
  と同じ値の `GIT_COMMITTER_NAME` / `GIT_COMMITTER_EMAIL` を付けて `git commit` する (コンテナの環境変数が git config を上書きするため)。
- ブランチ + PR、CI 緑の後に rebase merge。
- Release は **`release.yml` の workflow_dispatch** で作る (`inputs[version]=X.Y.Z`、`Cargo.toml` の version と一致させる)。
  タグの push はプロキシで 403。手順は SKILL.md §5。
