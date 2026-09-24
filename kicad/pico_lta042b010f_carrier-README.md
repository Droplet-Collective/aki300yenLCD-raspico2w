# Raspberry Pi Pico 2W / LTA042B010F KiCad reviewed draft

KiCad 9.0.1で開ける、銅箔配線前のキャリア基板ドラフトです。回路図は、MC34063の電源回路を実線配線で表し、Pico／LCD間の多本数RGB666信号だけをネット名で整理しています。

## 成果物

- `pico_lta042b010f_carrier.kicad_pro` — KiCadプロジェクト
- `pico_lta042b010f_carrier.kicad_sch` — 更新済み回路図
- `pico_lta042b010f_carrier.kicad_pcb` — 120 mm x 70 mm、部品配置済み・未配線
- `pico_lta042b010f_carrier.kicad_sym` / `sym-lib-table` — 埋め込み記号とローカル記号ライブラリ設定
- `pico_lta042b010f_carrier-kicad-native-reviewed.pdf` — KiCad純正のA4 PDF
- `pico_lta042b010f_carrier-kicad-native-reviewed-preview.png` — 上記PDFをそのままラスタライズした確認画像
- `pico_lta042b010f_carrier-pcb-3d-top.png` — KiCad純正PCB 3Dトップ表示
- `pico_lta042b010f_carrier-bom.csv` — 秋月調達候補を含む、Ref単位のBOM
- `pico_lta042b010f_carrier-reviewed-erc.txt` / `pico_lta042b010f_carrier-reviewed-drc.txt` — 検査結果
- `pico_lta042b010f_carrier-junction-cleanup.txt` — 接続点整理ルールと結果

## 置き換えた部品とフットプリント

| Ref | 回路記号／値 | 採用フットプリント |
|---|---|---|
| U1 | Raspberry Pi Pico 2W | `Module:RaspberryPi_Pico_W_SMD_HandSolder` |
| J1 | LTA042B010F 36P FFC | `Connector_FFC-FPC:TE_3-1734839-6_1x36-1MP_P0.5mm_Horizontal` |
| U3 | MC34063AD | `Package_SO:SOIC-8_3.9x4.9mm_P1.27mm` |
| R7 | 0.47 Ω ±1%、1 W current sense、軸リード | `Resistor_THT:R_Axial_DIN0207_L6.3mm_D2.5mm_P10.16mm_Horizontal` |
| L1 | 100 µH、NR10050T101M | `Inductor_SMD:L_Taiyo-Yuden_NR-10050_9.8x10.0mm_HandSoldering` |
| D4/D6/D7 | 1N5819 Schottky、DO-41 | `Diode_THT:D_DO-41_SOD81_P10.16mm_Horizontal` |
| R8 | 200 Ω、0603/1608 | `Resistor_SMD:R_0603_1608Metric` |
| R9/R11/R16 | 10 kΩ、±0.1%、0805/2012 | `Resistor_SMD:R_0805_2012Metric` |
| R10 | 1 kΩ、±0.1%、0805/2012 | `Resistor_SMD:R_0805_2012Metric` |
| C7 | 470 pF、50 V、C0G、2.5 mmピッチ | `Capacitor_THT:C_Disc_D5.0mm_W2.5mm_P2.50mm` |
| D5/D8 | 緑／赤3 mm砲弾型LED、2.54 mmピッチ | `LED_THT:LED_D3.0mm` |
| C6 | 220 µF、35 V、8 mm径、3.5 mmピッチ | `Capacitor_THT:CP_Radial_D8.0mm_P3.50mm` |
| C8 | 47 µF、35 V、5 mm径、2.0 mmピッチ | `Capacitor_THT:CP_Radial_D5.0mm_P2.00mm` |
| C9/C11 | 10 µF、50 V、5 mm径、2.0 mmピッチ | `Capacitor_THT:CP_Radial_D5.0mm_P2.00mm` |
| RV1 | 10 kΩトリマ | `Potentiometer_THT:Potentiometer_Bourns_3296W_Vertical` |
| J2/J3 | 2P電源コネクタ | `Connector_PinHeader_2.54mm:PinHeader_1x02_P2.54mm_Vertical` |

上表の部品は、指定された秋月品を優先し、残りも秋月で調達できる候補へ寄せています。D5/D8は秋月の単品販売品（緑111635、赤111577）に合わせた3 mm砲弾型です。C7は秋月131236（470 pF、50 V、C0G、±5%、2.5 mmピッチ）です。C6の指定品（秋月コード111758）は販売終了のため、BOMには指定品を残し、同寸法・同定格の現行候補コード102718を注記しています。J1はLCDモジュールそのものではなく基板側のTE製FFCコネクタであり、指定リンク内に対応するコネクタ品がないため、型番・接触面・ピン1方向の現物確認を残しています。

Picoのフットプリントは、Raspberry Pi公式資料のPico W/Pico 2 W共通の40ピン・2.54 mm配置を前提に、KiCad公式フットプリントライブラリのSMD手はんだ版を採用しています。Pico 2 Wのアンテナ側は基板端から離して配置しています。FFCの接触面、ピン1方向、実際のケーブル型番は現物照合が必要です。

## 検査結果

- ERC: **エラー0、警告0**
- 回路図ネットリスト: +5V、+13V8、-13V8、GND、Pico/LCD RGB・同期信号、MC34063周辺の接続を確認済み
- PCB DRC: **未配線85件**。これは「アートワーク前」の状態によるものです。今回の部品置換後、配置由来のコートヤード重なり・基板端クリアランスは0件です。残る35件は、シルク重なり16件、シルク下の銅箔警告8件、KiCadライブラリ部品を基板ファイルへ展開した際の照合警告11件です。銅箔配線後にシルクと最終DRCを仕上げます。

## 回路図の接続点整理

- 候補107件を検査し、重複・直線上の不要点を除去して構造的なワイヤ同士の分岐19件だけを保持
- 部品ピン端子上の接続点は0件。ピンへはワイヤ端点を正確に合わせ、接続点はワイヤ同士のT字・十字分岐に限定
- 4方向接続は0件。J2/C6/RV1のGNDは端子側ではなくGNDレール側または下側のラベルで接続
- 変更前後でネット定義44件、`(nets ...)`のピン集合一致、ERCエラー／警告0を確認
- 完全重複・同一直線上の部分重複ワイヤは0件。J2/R8/U3周辺の冗長な折返し・部品本体を横切る配線を除去

## 回路図の視覚整理

- 部品のReference／Valueを部品の回転とは独立して水平配置し、配線上から移動
- R7、L1、D4、R9、R11、D5、D6、D7、C11、D8など、縦向き・上下反転・隣接文字の重なりを修正
- J2はピン1を上側の+5V、ピン2を下側のGNDへ整理し、C6を左下へ配置
- R8のDC線はU3上側を迂回し、IPK検出線は+5V Vin線と同じ高さを共有せずU3右側へ迂回
- R9/R10のフィードバック配線は部分重複をなくし、RV1のGNDは+3V3枝を横切らず下側ラベルへ接続
- J2周辺、MC34063周辺、RV1の+3V3表示を整理し、短い明示配線で確認できる状態に変更
- J1をMC34063電源ブロックの右側へ分離し、GNDレールがFFC記号本体を横切らないLCD/FFCインターフェース配置へ変更。J2の`5V INPUT`とD5の`GREEN LED`も近接部品から退避
- 電源フラグのシンボルは保持しつつ、`PWR_FLAG` の冗長な文字列を非表示にし、電源ネット名との重なりを解消
- +13V8上側レールの明らかに余った末端配線を削除し、実際の分岐位置で終端
- 回路図をA3からA4横向きへ変更し、回路本体を拡大してタイトルブロックとの干渉を解消
- 内部接続補助ラベルは電気的接続を保持するため残していますが、通常の画像表示では読めない極小サイズです
- KiCad純正PDFは1ページ、整理後ERCはエラー0／警告0、ネットリストの`(nets ...)`差分は0です

U3は、SwEをGNDへ意図的に接続するこの回路でERCを成立させるため、埋め込み記号の該当ピンをpassiveとして扱っています。そのためプロジェクトでは、この意図的な記号差分だけ`lib_symbol_mismatch`を無視しています。実機のMC34063AD品種とデータシートを再確認してください。

## 電源回路の前提

- +5 V入力をMC34063ADで昇圧し、+13.8 Vを生成
- C9、D6、D7、C11で反転チャージポンプを構成し、-13.8 Vを生成
- Pico 3V3_OUTをLCDのDVDD/AVDD、+5 VをGVDD、+13.8 VをVGON、-13.8 VをVSSへ接続
- VCPP_ADJは10 kΩトリマで調整
- 実機投入前に、ダイオード極性、電解コンデンサ極性、インダクタの飽和電流、+13.8 V/-13.8 Vの負荷時電圧・リップルを確認

L1は元の150 µHから指定品の100 µHへ変更しています。これはフットプリントだけの置換ではなく昇圧回路の定数変更なので、実機では発振周波数・電流制限・出力電圧・インダクタ温度を再確認してください。

PCB Editorで **Update PCB from Schematic (F8)** を実行し、現物に合わせて外形・コネクタ位置・Picoアンテナ周辺・電源／GND配線を確定した後、銅箔配線と最終DRCへ進みます。

## 参考資料

- [Raspberry Pi Pico-series公式ドキュメント](https://www.raspberrypi.com/documentation/microcontrollers/pico-series.html)
- [Raspberry Pi Pico 2 W公式データシート](https://datasheets.raspberrypi.com/picow/pico-2-w-datasheet.pdf)
- [KiCad公式フットプリントライブラリ](https://gitlab.com/kicad/libraries/kicad-footprints/)
- [KiCad公式Pico W SMD手はんだフットプリント](https://gitlab.com/kicad/libraries/kicad-footprints/-/blob/master/Module.pretty/RaspberryPi_Pico_W_SMD_HandSolder.kicad_mod?ref_type=heads)
- [KiCad 9回路図エディターマニュアル](https://docs.kicad.org/9.0/ja/eeschema/eeschema.html)
- [KiCad 9 CLIマニュアル](https://docs.kicad.org/9.0/en/cli/cli.html)
- [秋月電子 NR10050T101M（コード108325）](https://akizukidenshi.com/catalog/g/g108325/)
- [秋月電子 35ZLH220MEFCCT8X11.5（コード111758、販売終了）](https://akizukidenshi.com/catalog/g/g111758/)
- [秋月電子 35PX47MEFC5X11（コード117887）](https://akizukidenshi.com/catalog/g/g117887/)
- [秋月電子 1N5819（コード117244）](https://akizukidenshi.com/catalog/g/g117244/)
- [秋月電子 MC34063AD（コード117573）](https://akizukidenshi.com/catalog/g/g117573/)
- [秋月電子 Raspberry Pi Pico 2 W SC1633（コード130330）](https://akizukidenshi.com/catalog/g/g130330/)
- [onsemi MC34063A公式データシート](https://www.onsemi.com/download/data-sheet/pdf/mc34063a-d.pdf)
- [Diodes Incorporated 1N5819公式ページ](https://www.diodes.com/part/view/1N5819)
- [pol: ESP32-S3で秋月300円液晶を動かす](https://pol.hateblo.jp/entry/2023/11/27/001524)
- [なる研: LTA042B010F解析](https://naruken.cweb.tk/labo/naruken/lta042b010f/)
