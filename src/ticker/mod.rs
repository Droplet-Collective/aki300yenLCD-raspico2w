//! ネットワーク・ティッカー (`src/bin/ticker.rs`) の部品
//!
//! - [`civil`] — UNIX 時刻 → 年月日・曜日・時分秒 (純粋な計算、ホストのテストあり)
//! - [`config`] — SD カードの `ticker.txt` (key=value) の解釈と既定値 (東京)
//! - [`weather`] — Open-Meteo の JSON の解釈と WMO 天気コードの日本語化
//! - [`sntp`] — SNTP (RFC 4330) の 48 バイトパケットの組み立て / 解釈 (純粋)、[`sntp_net`]: embassy-net (UDP) での問い合わせ
//! - [`digits`] — 時計用の大きな数字 (5×7 ドット、拡大描画)
//!
//! `civil` / `config` / `weather` / `digits` / `sntp` は `core` (+ heapless / serde) だけに
//! 依存し、`tools/ticker-tests` がホストでテストする。

pub mod civil;
pub mod config;
pub mod digits;
pub mod sntp;
pub mod slideshow;
pub mod sntp_net;
pub mod weather;
