//! 設定ページ (0.5.0〜): 同じ LAN のブラウザから画像 / 地域 / 表示の設定を変える HTTP サーバ
//!
//! - [`server`] — 待ち受けと要求の処理 (embassy-net、SD、ticker への依頼)。ticker の取得タスクの中で動く
//! - [`http`] / [`form`] / [`auth`] / [`upload`] / [`json`] — ハードウェアに依存しない部品。`tools/ticker-tests`
//!   がホストでテストする (要求ヘッダの解釈、フォームの復号、アクセスコードと締め出し、BMP の検査と 8.3 の名前、JSON)
//!
//! 使い方 / API / 安全のしくみは docs/settings-server.md。

pub mod auth;
pub mod form;
pub mod http;
pub mod json;
pub mod server;
pub mod upload;
