# 東雲フォント (Shinonome font) 14 ドットのビットマップテーブル

`ticker` bin が日本語を描くために埋め込む 14 ドットゴシック体 (全角 14×14、半角 7×14)。
v0.3.0 までの美咲フォント 8×8 の 2 倍表示 (線が 2 px で太く潰れて見えた) を、等倍の 1 px 線に置き換えた (v0.3.1)。

- 出典: /efont/ プロジェクト「東雲フォント」0.9.11 のゴシック体 14 ドット
  (`shnmk14` JIS X 0208 全角 + `shnm7x14r` JIS X 0201 半角)。<http://openlab.ring.gr.jp/efont/shinonome/>
  取得元は Debian / Ubuntu の `xfonts-shinonome` 1:0.9.11-7 パッケージ (PCF) → `pcf2bdf` で BDF に戻した。
- ライセンス: **Public Domain** (作者が権利を行使しないと宣言、改造・変換・組込み・再配布自由、無保証)。
  原文は [`LICENSE`](LICENSE)。設計の由来 (k14 ベースのゴシック) は [`DESIGN.14.txt`](DESIGN.14.txt)。
- 変換: `tools/bdf2bin.py --height 14 --out fonts/shinonome shnmk14.bdf:jisx0208 shnm7x14r.bdf:jisx0201`
  (JIS 区点 → Unicode は EUC-JP として変換。〜/～、−/－、¥/\ などは同じグリフを両方の符号位置に載せる)

| ファイル | 内容 | サイズ |
|---|---|---|
| `codes.bin` | Unicode スカラー値 (u16 LE、昇順、7,047 個) | 14,094 B |
| `glyphs.bin` | 14 行 × 2 B のビットマップ (28 B / グリフ、行 0 が上、bit15 が左) | 197,316 B |
| `widths.bin` | 送り幅 (半角 7 / 全角 14) | 7,047 B |

計 218,457 B。ファームウェア側の読み手は `src/font/shinonome.rs` (二分探索 + 等倍描画)。
