# DejaVu Sans (時計 / 気温の数字、v0.4.0〜)

`ticker` の時計と気温の数字 (`0-9 : - . ° % /`) は DejaVu Sans (Book) を 4 bit のアンチエイリアスで
ラスタライズした表 `src/ui/aafont_data.rs` を使う (フォントファイルそのものは含めない)。

- 生成: `python3 tools/ui-sim/gen_aafont.py [DejaVuSans.ttf]` (Pillow。既定は Debian / Ubuntu の
  `fonts-dejavu-core` の `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf`)
- 大きさ: 時計 42 px (字の高さ 32 行)、気温 20 px、予報 13 px
- ライセンス: Bitstream Vera Fonts のライセンス (DejaVu の変更はパブリックドメイン)。原文は [LICENSE](LICENSE)。
  改変した「フォント」を配るときは Bitstream / Vera の名前を使わないこと、という条件があるが、ここで配るのは
  ビットマップの表だけで、名前は変えていない (DejaVu のまま) ので条件に反しない。
