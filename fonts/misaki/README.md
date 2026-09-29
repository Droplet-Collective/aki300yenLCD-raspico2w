# 美咲フォント (Misaki font) のビットマップテーブル

`ticker` bin が日本語を描くために埋め込む 8×8 ドットフォント。

- 出典: 門真なむ (Num Kadoma) 氏「美咲フォント」2021-05-05 版、BDF 形式
  <https://littlelimit.net/misaki.htm> (`misaki_bdf_2021-05-05.zip` の `misaki_gothic.bdf`)
- ライセンス: 同梱の [`misaki.txt`](misaki.txt) 「ライセンス」節 (原文)。
  > These fonts are free softwares. Unlimited permission is granted to use, copy, and distribute it,
  > with or without modification, either commercially and noncommercially.
  > THESE FONTS ARE PROVIDED "AS IS" WITHOUT WARRANTY.
- 変換: `tools/misaki2bin.py misaki_gothic.bdf fonts/misaki` (BDF の BBX オフセットを 8 行枠に展開)

| ファイル | 内容 | サイズ |
|---|---|---|
| `codes.bin` | Unicode スカラー値 (u16 LE、昇順、7,171 個) | 14,342 B |
| `glyphs.bin` | 8×8 ビットマップ (8 B / グリフ、行 0 が上、bit7 が左) | 57,368 B |
| `widths.bin` | 送り幅 (半角 4 / 全角 8) | 7,171 B |

ファームウェア側の読み手は `src/font/misaki.rs` (二分探索 + 倍率付き描画)。
