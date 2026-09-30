//! 設定ページの HTTP サーバ (0.5.0〜、ticker の取得タスクの中で動く)。使い方と API は docs/settings-server.md。
//!
//! # 動き方 (OTA 到達保証との関係、docs/ticker.md §8)
//!
//! - **取得タスク (`jobs_task`) の中で 1 要求ずつ** 処理する。取得タスクは毎周 OTA 確認を最初に見るので、
//!   OTA 確認 / ダウンロードの間はサーバは動かない (要求はソケットで待たされる)。OTA のためのタスクと
//!   HTTPS のバッファ (`NetBuffers`、≈ 30 kB) を増やさずに済み、サーバの作業領域もそのバッファを借りる
//!   (OTA / 天気 / 文字の取得とは同時に動かないので)。新しい static は待ち受けソケットの 2 kB だけ。
//! - 待ち受けは **最初の OTA 確認が通ってから** (ticker 側で `OTA_PROVED` を見て [`Server::new`])。
//!   回復モードでは作らない。
//! - 1 つの要求の処理は [`Who::Web`] の生存確認付き: 読み書き 1 回 [`IO_TIMEOUT`]、SD の 1 回の操作
//!   `SD_OP_DEADLINE_MS`、要求全体 [`REQUEST_DEADLINE`] (写真の追加は [`UPLOAD_DEADLINE`]) で打ち切り、
//!   進むたびに `supervisor::beat(Who::Web)`。待ち受けに戻ったら `supervisor::park(Who::Web)`。
//! - 接続は 1 本ずつ (keep-alive あり、[`KEEP_ALIVE_IDLE`] で閉じる)。ページの JS は要求を 1 本ずつ順に送る。
//! - SD はスライドショーと `slideshow::SD_LOCK` で分け合う。取れなければ 503 (`SD_LOCK_TIMEOUT`)。

use core::fmt::Write as _;
use core::ptr::addr_of_mut;
use core::sync::atomic::Ordering;

use embassy_net::tcp::{State, TcpSocket};
use embassy_net::Stack;
use embassy_time::{Duration, Instant, with_timeout};
use heapless::{String, Vec};

use super::auth::{self, AuthError, Guard};
use super::form;
use super::http::{self, Head, HeadError, Method};
use super::json::Json;
use super::upload;
use crate::ota::app::NetBuffers;
use crate::sdcard::{self, SdVolumeManager};
use crate::supervisor;
use crate::ticker::config::{self, CONFIG_MAX, TickerConfig, Update};
use crate::ticker::health::Who;
use crate::ticker::slideshow::{self, SD_OP_DEADLINE_MS};

pub const PORT: u16 = 80;
/// 待ち受けソケットのバッファ (RAM が足りないので小さい。LAN なら写真 1 枚 115 kB が 1〜2 s)
pub const SOCKET_RX: usize = 1024;
pub const SOCKET_TX: usize = 1024;
/// 1 回の読み書きの打ち切り
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// 1 つの要求全体の打ち切り (写真の追加以外)
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(15);
/// 写真の追加 (115 kB を受けて SD に書く) の打ち切り
pub const UPLOAD_DEADLINE: Duration = Duration::from_secs(60);
/// keep-alive の接続を何もせず保つ時間
pub const KEEP_ALIVE_IDLE: Duration = Duration::from_secs(5);
/// SD をスライドショーから借りるまで待つ時間 (写真 1 枚の読み込みは 2〜4 s)
pub const SD_LOCK_TIMEOUT: Duration = Duration::from_secs(6);
/// SD を使った後、スライドショーに次の写真を読ませない時間 (サムネイルを続けて読む間)
pub const SD_HOLD_MS: u32 = 5_000;
/// 設定の本文 (フォーム) の上限
pub const FORM_MAX: usize = 4096;
/// 「LCD にコードを表示」を受け付ける間隔
pub const SHOW_CODE_INTERVAL: Duration = Duration::from_secs(10);

/// gzip した設定ページ (build.rs が web/settings/index.html から作る)
static PAGE_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/settings.html.gz"));

static mut SOCKET_RX_BUF: [u8; SOCKET_RX] = [0; SOCKET_RX];
static mut SOCKET_TX_BUF: [u8; SOCKET_TX] = [0; SOCKET_TX];

/// サーバが ticker に頼むこと (状態の JSON、設定の反映、再起動など)
pub trait App {
    /// この端末の IPv4 アドレス (まだ無ければ None)
    fn ip(&self) -> Option<[u8; 4]>;
    /// `/api/status` の中身 (オブジェクトの中のフィールドだけを書く)
    fn write_status(&mut self, j: &mut Json<'_>);
    /// 今の設定 (保存した内容をその場で反映したもの)
    fn config(&self) -> &TickerConfig;
    /// `message=` の文字 (この端末で決めた流れる文字。無ければ空)
    fn write_local_message(&self, j: &mut Json<'_>);
    /// 保存した `ticker.txt` の内容をその場で反映する (`message` は `message=` の文字)
    fn apply(&mut self, new: &TickerConfig, message: Option<&str>);
    /// LCD に案内 (URL とコード) を出す
    fn show_code(&mut self);
    /// 再起動を頼む (TBYB の buy 待ちなどで断るときは理由)
    fn reboot(&mut self) -> Result<(), &'static str>;
    /// すぐに OTA 確認を頼む
    fn ota_check(&mut self);
}

/// 要求 1 つの結果 (接続を続けるか)
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    KeepAlive,
    Close,
}

/// 作業領域 (取得タスクの `NetBuffers` を借りる。OTA / 取得と同時には使わない)
struct Work<'a> {
    /// 要求ヘッダ (+ 一緒に届いた本文の先頭)
    head: &'a mut [u8],
    /// 応答の本文 (JSON)
    out: &'a mut [u8],
    /// 応答ヘッダ
    hdr: &'a mut [u8],
    /// SD の 1 ブロック
    block: &'a mut [u8],
    /// フォームの本文 / ticker.txt の新旧 / 読み戻し / 値の復号
    scratch: &'a mut [u8],
}

impl<'a> Work<'a> {
    fn new(bufs: &'a mut NetBuffers) -> Self {
        let (head, out) = bufs.http_rx.split_at_mut(http::HEAD_MAX);
        Self {
            head,
            out,
            hdr: &mut bufs.chunk[..],
            block: &mut bufs.tls_tx[..512],
            scratch: &mut bufs.tls_rx[..],
        }
    }
}

/// 失敗の応答 (状態コード + 日本語の理由。ページがそのまま出す)
struct Fail(u16, &'static str);

pub struct Server {
    socket: TcpSocket<'static>,
    sd: Option<&'static SdVolumeManager>,
    guard: Guard,
    /// keep-alive の接続で最後に要求を処理した時刻
    last_active: Instant,
    /// 閉じ始めた時刻 (FIN を送った。相手が閉じるか 2 s で片付けて待ち受けに戻る)
    closing_since: Option<Instant>,
    last_show_code: Option<Instant>,
    listening_once: bool,
}

impl Server {
    /// 待ち受けソケットを作る (1 回だけ呼ぶ。バッファは static)
    pub fn new(stack: Stack<'static>, sd: Option<&'static SdVolumeManager>, code: u32) -> Self {
        // Safety: Server は取得タスクで 1 つだけ作る (呼び出し側が Option で守る)
        let (rx, tx) = unsafe { (&mut *addr_of_mut!(SOCKET_RX_BUF), &mut *addr_of_mut!(SOCKET_TX_BUF)) };
        let mut socket = TcpSocket::new(stack, rx, tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        Self {
            socket,
            sd,
            guard: Guard::new(code),
            last_active: Instant::now(),
            closing_since: None,
            last_show_code: None,
            listening_once: false,
        }
    }

    pub fn code(&self) -> u32 {
        self.guard.code()
    }

    /// 待ち受けを始めた (一度でも)
    pub fn is_listening(&self) -> bool {
        self.listening_once
    }

    /// 毎周の片付け: 閉じた接続を待ち受けに戻す、keep-alive の時間切れ、相手が閉じた接続
    pub fn maintain(&mut self) {
        let state = self.socket.state();
        if let Some(since) = self.closing_since {
            if matches!(state, State::TimeWait | State::Closed) || since.elapsed() >= Duration::from_secs(2) {
                self.socket.abort();
                self.closing_since = None;
            } else {
                return;
            }
        }
        match self.socket.state() {
            State::Closed => {
                // listen だけして戻る (accept の future は待たない。落としても Listen のまま)
                let _ = embassy_futures::poll_once(self.socket.accept(PORT));
                if self.socket.state() == State::Listen && !self.listening_once {
                    self.listening_once = true;
                    defmt::info!("web: listening on port {}", PORT);
                }
            }
            // 相手が閉じた / keep-alive で何も来ない
            State::Established | State::CloseWait
                if !self.socket.can_recv() && (state == State::CloseWait || self.last_active.elapsed() >= KEEP_ALIVE_IDLE) =>
            {
                self.begin_close();
            }
            _ => {}
        }
    }

    /// 要求が届いている
    pub fn has_request(&self) -> bool {
        self.closing_since.is_none()
            && matches!(self.socket.state(), State::Established | State::CloseWait)
            && self.socket.can_recv()
    }

    /// 次の要求が届くか `d` が過ぎるまで待つ (取得タスクの周期の待ちの代わり)
    pub async fn wait(&self, d: Duration) {
        if self.socket.can_recv() {
            // 届いているのに今周は処理しなかった (OTA / 再起動待ち / ネットワーク断): すぐ戻ると空回りする
            embassy_time::Timer::after(d).await;
        } else {
            let _ = with_timeout(d, self.socket.wait_read_ready()).await;
        }
    }

    fn begin_close(&mut self) {
        self.socket.close();
        self.closing_since = Some(Instant::now());
    }

    /// 届いている要求を 1 つ処理する (生存確認 `Who::Web` 付き)
    pub async fn serve(&mut self, bufs: &mut NetBuffers, app: &mut impl App) {
        supervisor::beat(Who::Web);
        let mut w = Work::new(bufs);
        let next = self.handle(&mut w, app).await;
        self.last_active = Instant::now();
        if next == Next::Close {
            let _ = with_timeout(Duration::from_secs(2), self.socket.flush()).await;
            self.begin_close();
        }
        supervisor::park(Who::Web);
    }

    async fn handle(&mut self, w: &mut Work<'_>, app: &mut impl App) -> Next {
        let started = Instant::now();
        // --- 要求ヘッダ (5 s 以内に空行まで) ---
        let mut len = 0;
        loop {
            match http::find_head_end(&w.head[..len]) {
                Some(_) => break,
                None if len >= w.head.len() => {
                    let _ = self.error(w, Fail(431, "要求ヘッダが長すぎます"), true).await;
                    return Next::Close;
                }
                None => {}
            }
            if started.elapsed() >= IO_TIMEOUT {
                return Next::Close;
            }
            match with_timeout(IO_TIMEOUT, self.socket.read(&mut w.head[len..])).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return Next::Close,
                Ok(Ok(n)) => len += n,
            }
            supervisor::beat(Who::Web);
        }
        // parse_head は w.head を借りるので、必要な値を先に写す
        let head = match http::parse_head(&w.head[..len]) {
            Ok(h) => h,
            Err(e) => {
                let fail = match e {
                    HeadError::TooLarge => Fail(431, "要求ヘッダが長すぎます"),
                    HeadError::Unsupported => Fail(501, "対応していない本文の形式です"),
                    _ => Fail(400, "要求の形が正しくありません"),
                };
                let _ = self.error_raw(w.hdr, w.out, fail, true).await;
                return Next::Close;
            }
        };
        let req = Req::from_head(&head, len);
        // 本文の後ろまで読んでしまった (パイプライン化された次の要求): 次の要求の頭が分からないので、応答したら閉じる
        let overread = len > req.head_len + req.body_len as usize;
        let result = self.route(w, app, &req, started).await;
        match result {
            Ok(next) => {
                if req.close || overread {
                    Next::Close
                } else {
                    next
                }
            }
            Err(fail) => {
                // 本文を読み残した要求は接続ごと閉じる (次の要求の頭が分からない)
                let _ = self.error(w, fail, true).await;
                Next::Close
            }
        }
    }

    async fn route(&mut self, w: &mut Work<'_>, app: &mut impl App, req: &Req, started: Instant) -> Result<Next, Fail> {
        // ヘッダは w.head を指すので、使う値を先に写してから本文 / 応答に進む
        let ip = app.ip().ok_or(Fail(503, "IP アドレスがまだありません"))?;
        let (route, code, same_origin) = {
            let head = req.view(w.head);
            // DNS rebinding 対策: Host はこの端末の IP アドレスだけ (GET も)
            if !auth::host_is_board(head.host, ip) {
                return Err(Fail(403, "Host がこの端末の IP アドレスではありません"));
            }
            let mut code: Option<String<16>> = None;
            if let Some(c) = head.code {
                let mut s: String<16> = String::new();
                let _ = s.push_str(c.get(..c.len().min(16)).unwrap_or(""));
                code = Some(s);
            }
            (
                Route::from(&head),
                code,
                auth::same_origin(head.host, head.origin, head.fetch_site),
            )
        };
        if !route.is_post() && req.body_len > 0 && !matches!(route, Route::Options) {
            return Err(Fail(400, "本文は受け付けません"));
        }
        if route.is_post() && !same_origin {
            return Err(Fail(403, "よそのページからの要求は受け付けません"));
        }
        if route.needs_code() {
            let now_ms = Instant::now().as_millis() as u32;
            match self.guard.check(now_ms, code.as_ref().map(|c| c.as_str())) {
                Ok(()) => {}
                Err(AuthError::Missing) => return Err(Fail(401, "アクセスコードが要ります")),
                Err(AuthError::Wrong { left }) => {
                    defmt::warn!("web: wrong access code ({} left)", left);
                    return Err(Fail(401, "アクセスコードが違います"));
                }
                Err(AuthError::Locked { secs }) => {
                    defmt::warn!("web: locked for {} s", secs);
                    return Err(Fail(429, "コードを続けて間違えたので、しばらく受け付けません"));
                }
            }
        }
        match route {
            Route::Page => {
                self.send(w.hdr, 200, "text/html; charset=utf-8", PAGE_GZ, req.close, true).await?;
                Ok(Next::KeepAlive)
            }
            Route::Status => {
                let mut j = Json::new(w.out);
                j.begin_object();
                app.write_status(&mut j);
                j.end_object();
                self.send_json(w.hdr, &j, req.close).await
            }
            Route::Settings => {
                let mut j = Json::new(w.out);
                write_settings(&mut j, &*app, self.sd.is_some());
                self.send_json(w.hdr, &j, req.close).await
            }
            Route::Images => {
                let sd = self.sd.ok_or(Fail(503, "SD カードがありません"))?;
                let mut j = Json::new(w.out);
                list_images(sd, &mut j, app.config()).await?;
                self.send_json(w.hdr, &j, req.close).await
            }
            Route::Image(name) => self.stream_image(w, &name, req.close, started).await,
            Route::Options => {
                // CORS の事前確認には許可を返さない (よそのサイトからは独自ヘッダ付きの要求を送れない)
                self.send_status(w.hdr, 405, true).await?;
                Ok(Next::Close)
            }
            Route::BadMethod => Err(Fail(405, "対応していないメソッドです")),
            Route::NotFound => Err(Fail(404, "ありません")),
            Route::ShowCode => {
                // コードは要らない (LCD に出すだけ)。短い間隔の連打は断る
                if self.last_show_code.is_some_and(|t| t.elapsed() < SHOW_CODE_INTERVAL) {
                    return Err(Fail(429, "少し待ってからもう一度押してください"));
                }
                self.last_show_code = Some(Instant::now());
                self.read_body(w, req).await?;
                app.show_code();
                self.send_ok(w, req.close).await
            }
            Route::Upload(original) => self.upload(w, app, req, &original).await,
            Route::Auth | Route::SaveSettings | Route::DeleteImage | Route::Reboot | Route::OtaCheck => {
                let body_len = self.read_body(w, req).await?;
                match route {
                    Route::SaveSettings => {
                        let changed = self.save_form(w, app, body_len).await?;
                        let mut j = Json::new(w.out);
                        j.begin_object();
                        j.field_bool("ok", true);
                        j.field_int("changed", changed as i64);
                        j.end_object();
                        self.send_json(w.hdr, &j, req.close).await
                    }
                    Route::DeleteImage => {
                        self.delete_image(w, app, body_len).await?;
                        self.send_ok(w, req.close).await
                    }
                    Route::Reboot => {
                        app.reboot().map_err(|e| Fail(409, e))?;
                        self.send_ok(w, true).await?;
                        Ok(Next::Close)
                    }
                    Route::OtaCheck => {
                        app.ota_check();
                        self.send_ok(w, req.close).await
                    }
                    _ => self.send_ok(w, req.close).await,
                }
            }
        }
    }

    // ============================================================
    // 本文
    // ============================================================

    /// 本文 (`req.body_len` バイト、[`FORM_MAX`] 以下) を `w.scratch[..n]` へ。ヘッダと一緒に届いた分から写す
    async fn read_body(&mut self, w: &mut Work<'_>, req: &Req) -> Result<usize, Fail> {
        let want = req.body_len as usize;
        if want > FORM_MAX {
            return Err(Fail(413, "本文が長すぎます"));
        }
        let pre = req.pre_body(w.head);
        let n = pre.len().min(want);
        w.scratch[..n].copy_from_slice(&pre[..n]);
        if req.expect_continue && n < want {
            self.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
        }
        let mut got = n;
        while got < want {
            match with_timeout(IO_TIMEOUT, self.socket.read(&mut w.scratch[got..want])).await {
                Ok(Ok(0)) | Ok(Err(_)) => return Err(Fail(400, "本文が途中で切れました")),
                Err(_) => return Err(Fail(408, "本文が届きません")),
                Ok(Ok(k)) => got += k,
            }
            supervisor::beat(Who::Web);
        }
        Ok(want)
    }

    // ============================================================
    // 設定の保存
    // ============================================================

    /// フォームの設定を検査して ticker.txt に書き、その場で反映する。戻り値は変えたキーの数
    async fn save_form(&mut self, w: &mut Work<'_>, app: &mut impl App, body_len: usize) -> Result<usize, Fail> {
        let (form_buf, rest) = w.scratch.split_at_mut(FORM_MAX);
        let body = core::str::from_utf8(&form_buf[..body_len]).map_err(|_| Fail(400, "本文が UTF-8 ではありません"))?;
        let (arena, work) = rest.split_at_mut(FORM_MAX);
        let mut updates: Vec<Update<'_>, 12> = Vec::new();
        let mut arena_rest: &mut [u8] = arena;
        for (raw_key, raw_value) in form::pairs(body) {
            let key = SETTING_KEYS
                .iter()
                .copied()
                .find(|k| *k == raw_key)
                .ok_or(Fail(400, "知らない設定の名前です"))?;
            if updates.iter().any(|(k, _)| *k == key) {
                return Err(Fail(400, "同じ設定が 2 回あります"));
            }
            // 百分率符号化を解くと短くなるだけなので、元の長さの領域があれば足りる
            if raw_value.len() > arena_rest.len() {
                return Err(Fail(413, "本文が長すぎます"));
            }
            let (slot, tail) = core::mem::take(&mut arena_rest).split_at_mut(raw_value.len());
            arena_rest = tail;
            let value = form::decode(raw_value, slot).ok_or(Fail(400, "値の符号化が正しくありません"))?;
            let value = value.trim();
            let update = if value.is_empty() {
                // images= / message= は空にすると行を消す (ルートの全部 / message_url に戻る)
                if key == "images" || key == "message" {
                    None
                } else {
                    return Err(Fail(422, "空にできない設定です"));
                }
            } else if config::valid_value(key, value) {
                Some(value)
            } else {
                return Err(Fail(422, "値が正しくありません"));
            };
            updates.push((key, update)).map_err(|_| Fail(400, "設定が多すぎます"))?;
        }
        if updates.is_empty() {
            return Ok(0);
        }
        let sd = self.sd.ok_or(Fail(503, "SD カードが無いので保存できません"))?;
        let n = updates.len();
        save_ticker_txt(sd, &updates, work, app).await?;
        Ok(n)
    }

    async fn delete_image(&mut self, w: &mut Work<'_>, app: &mut impl App, body_len: usize) -> Result<(), Fail> {
        let (form_buf, rest) = w.scratch.split_at_mut(FORM_MAX);
        let body = core::str::from_utf8(&form_buf[..body_len]).map_err(|_| Fail(400, "本文が UTF-8 ではありません"))?;
        let mut name_buf = [0u8; 16];
        let name = form::pairs(body)
            .find(|(k, _)| *k == "name")
            .and_then(|(_, v)| form::decode(v, &mut name_buf))
            .ok_or(Fail(400, "name がありません"))?;
        if !upload::is_bmp_name(name) {
            return Err(Fail(422, "写真の名前が正しくありません"));
        }
        let mut name_s: String<12> = String::new();
        let _ = name_s.push_str(name);
        let sd = self.sd.ok_or(Fail(503, "SD カードがありません"))?;
        {
            let _sd = lock_sd().await?;
            let result = sdcard::with_deadline(SD_OP_DEADLINE_MS, || {
                let volume = sd.open_volume(embedded_sdmmc::VolumeIdx(0)).map_err(|_| "volume")?;
                let root = volume.open_root_dir().map_err(|_| "root")?;
                root.delete_entry_in_dir(name_s.as_str()).map_err(|_| "delete")
            });
            hold_slideshow();
            result.map_err(|_| Fail(404, "消せませんでした (ありません)"))?;
        }
        supervisor::beat(Who::Web);
        defmt::info!("web: deleted {}", name_s.as_str());
        // images= にあれば外す
        let images = app.config().images.clone();
        if !images.is_empty() {
            let mut list: String<{ config::IMAGES_MAX }> = String::new();
            for n in config::image_names(&images).filter(|n| !n.eq_ignore_ascii_case(&name_s)) {
                if !list.is_empty() {
                    let _ = list.push(',');
                }
                let _ = list.push_str(n);
            }
            let update: Update<'_> = ("images", (!list.is_empty()).then_some(list.as_str()));
            save_ticker_txt(sd, &[update], rest, app).await?;
        }
        slideshow::RELOAD.store(true, Ordering::Relaxed);
        Ok(())
    }

    // ============================================================
    // 写真の追加 (400×96 の 24 bit BMP を SD へ流し込む)
    // ============================================================

    async fn upload(&mut self, w: &mut Work<'_>, app: &mut impl App, req: &Req, original: &str) -> Result<Next, Fail> {
        if req.body_len != upload::UPLOAD_SIZE {
            return Err(Fail(413, "400×96 の 24 bit BMP (115,254 バイト) だけ受け付けます"));
        }
        let sd = self.sd.ok_or(Fail(503, "SD カードがありません"))?;
        let started = Instant::now();
        let guard = lock_sd().await?;
        let _session = SessionDeadline::start(UPLOAD_DEADLINE);
        // 使える名前を決める (数の上限も)
        let mut count = 0usize;
        let listed = sdcard::with_deadline(SD_OP_DEADLINE_MS * 2, || sdcard::for_each_root_bmp(sd, |_, _| count += 1));
        if listed.is_err() {
            return Err(Fail(503, "SD を読めません"));
        }
        if count >= upload::MAX_FILES {
            return Err(Fail(409, "写真は 16 枚までです。先に消してください"));
        }
        let name = pick_name(sd, original).ok_or(Fail(409, "名前を決められませんでした"))?;
        supervisor::beat(Who::Web);

        // --- ヘッダ 54 バイトを受けて確かめる (ブロックの先頭に置き、そのまま SD に書く) ---
        let mut got = {
            let pre = req.pre_body(w.head);
            let n = pre.len().min(upload::HEADER);
            w.block[..n].copy_from_slice(&pre[..n]);
            n
        };
        if req.expect_continue {
            self.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?;
        }
        while got < upload::HEADER {
            got += self.read_some(&mut w.block[got..upload::HEADER], started).await?;
        }
        upload::check_header(&w.block[..upload::HEADER], req.body_len).map_err(|e| {
            defmt::warn!("web: upload rejected: {}", e);
            Fail(415, "400×96 の 24 bit BMP ではありません")
        })?;

        // --- SD に書く (512 B ずつ)。失敗したら消す ---
        let volume = sdcard::with_deadline(SD_OP_DEADLINE_MS, || sd.open_volume(embedded_sdmmc::VolumeIdx(0)))
            .map_err(|_| Fail(503, "SD を開けません"))?;
        let root = volume.open_root_dir().map_err(|_| Fail(503, "SD を開けません"))?;
        let file = sdcard::with_deadline(SD_OP_DEADLINE_MS, || {
            root.open_file_in_dir(name.as_str(), embedded_sdmmc::Mode::ReadWriteCreate)
        })
        .map_err(|_| Fail(503, "SD にファイルを作れません"))?;
        let mut blocks = BlockWriter { fill: upload::HEADER, written: 0 };
        let pre = req.pre_body(w.head);
        let extra = &pre[pre.len().min(upload::HEADER)..];
        let mut result = blocks.push(&file, w.block, extra);
        let total = upload::UPLOAD_SIZE as usize;
        while result.is_ok() && blocks.written + blocks.fill < total {
            let room = (512 - blocks.fill).min(total - blocks.written - blocks.fill);
            let fill = blocks.fill;
            match self.read_some(&mut w.block[fill..fill + room], started).await {
                Ok(n) => {
                    blocks.fill += n;
                    if blocks.fill == 512 {
                        result = blocks.flush(&file, w.block);
                        supervisor::beat(Who::Jobs);
                    }
                }
                Err(fail) => result = Err(fail),
            }
        }
        if result.is_ok() {
            result = blocks.flush(&file, w.block);
        }
        let closed = sdcard::with_deadline(SD_OP_DEADLINE_MS, || file.close());
        if result.is_err() || closed.is_err() {
            let _ = sdcard::with_deadline(SD_OP_DEADLINE_MS, || root.delete_entry_in_dir(name.as_str()));
            drop(root);
            drop(volume);
            hold_slideshow();
            drop(guard);
            defmt::warn!("web: upload of {} failed, partial file removed", name.as_str());
            return Err(result.err().unwrap_or(Fail(503, "SD に書けません")));
        }
        drop(root);
        drop(volume);
        hold_slideshow();
        drop(_session);
        drop(guard);
        defmt::info!("web: uploaded {} in {} ms", name.as_str(), started.elapsed().as_millis());

        // images= があれば最後に足す (無ければルートの全部を使うので何もしない)
        let images = app.config().images.clone();
        if !images.is_empty() && config::image_names(&images).count() < config::MAX_IMAGES {
            let mut list: String<{ config::IMAGES_MAX }> = String::new();
            let _ = list.push_str(&images);
            if list.push(',').is_ok() && list.push_str(&name).is_ok() {
                let (_, rest) = w.scratch.split_at_mut(FORM_MAX);
                save_ticker_txt(sd, &[("images", Some(list.as_str()))], rest, app).await?;
            }
        }
        slideshow::RELOAD.store(true, Ordering::Relaxed);
        let mut j = Json::new(w.out);
        j.begin_object();
        j.field_bool("ok", true);
        j.field_str("name", &name);
        j.end_object();
        self.send_json(w.hdr, &j, req.close).await
    }

    async fn read_some(&mut self, buf: &mut [u8], started: Instant) -> Result<usize, Fail> {
        if started.elapsed() >= UPLOAD_DEADLINE {
            return Err(Fail(408, "時間切れです"));
        }
        match with_timeout(IO_TIMEOUT, self.socket.read(buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => Err(Fail(400, "本文が途中で切れました")),
            Err(_) => Err(Fail(408, "本文が届きません")),
            Ok(Ok(n)) => {
                supervisor::beat(Who::Web);
                Ok(n)
            }
        }
    }

    // ============================================================
    // 写真の送信 (SD から 512 B ずつ)
    // ============================================================

    async fn stream_image(&mut self, w: &mut Work<'_>, name: &str, close: bool, started: Instant) -> Result<Next, Fail> {
        let sd = self.sd.ok_or(Fail(503, "SD カードがありません"))?;
        let guard = lock_sd().await?;
        let _session = SessionDeadline::start(Duration::from_secs(30));
        let volume = sdcard::with_deadline(SD_OP_DEADLINE_MS, || sd.open_volume(embedded_sdmmc::VolumeIdx(0)))
            .map_err(|_| Fail(503, "SD を開けません"))?;
        let root = volume.open_root_dir().map_err(|_| Fail(503, "SD を開けません"))?;
        let file = sdcard::with_deadline(SD_OP_DEADLINE_MS, || root.open_file_in_dir(name, embedded_sdmmc::Mode::ReadOnly))
            .map_err(|_| Fail(404, "写真がありません"))?;
        let total = file.length() as usize;
        let mut hdr = HeaderWriter::new(w.hdr);
        hdr.status(200);
        hdr.line("Content-Type: image/bmp");
        hdr.length(total);
        hdr.common(close);
        let n = hdr.finish();
        self.write_all_raw(&w.hdr[..n]).await.map_err(|_| Fail(0, ""))?;
        let mut sent = 0;
        while sent < total {
            if started.elapsed() >= Duration::from_secs(30) {
                return Ok(Next::Close);
            }
            let want = (total - sent).min(512);
            let got = sdcard::with_deadline(SD_OP_DEADLINE_MS, || file.read(&mut w.block[..want]));
            let got = match got {
                Ok(0) | Err(_) => return Ok(Next::Close), // ヘッダは送った後なので、切るしかない
                Ok(n) => n,
            };
            if self.write_all_raw(&w.block[..got]).await.is_err() {
                return Ok(Next::Close);
            }
            sent += got;
            supervisor::beat(Who::Web);
        }
        drop(file);
        drop(root);
        drop(volume);
        hold_slideshow();
        drop(guard);
        Ok(Next::KeepAlive)
    }

    // ============================================================
    // 応答
    // ============================================================

    async fn write_all_raw(&mut self, mut data: &[u8]) -> Result<(), ()> {
        while !data.is_empty() {
            match with_timeout(IO_TIMEOUT, self.socket.write(data)).await {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return Err(()),
                Ok(Ok(n)) => data = &data[n..],
            }
            supervisor::beat(Who::Web);
        }
        Ok(())
    }

    async fn write_all(&mut self, data: &[u8]) -> Result<(), Fail> {
        self.write_all_raw(data).await.map_err(|_| Fail(0, ""))
    }

    async fn send(&mut self, hdr_buf: &mut [u8], status: u16, ctype: &str, body: &[u8], close: bool, gzip: bool) -> Result<(), Fail> {
        let mut hdr = HeaderWriter::new(hdr_buf);
        hdr.status(status);
        hdr.content_type(ctype);
        if gzip {
            hdr.line("Content-Encoding: gzip");
            hdr.line("Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'self' blob: data:; connect-src 'self' https://geocoding-api.open-meteo.com; form-action 'none'; frame-ancestors 'none'; base-uri 'none'");
        }
        hdr.length(body.len());
        hdr.common(close);
        let n = hdr.finish();
        self.write_all(&hdr_buf[..n]).await?;
        self.write_all(body).await?;
        Ok(())
    }

    async fn send_json(&mut self, hdr_buf: &mut [u8], j: &Json<'_>, close: bool) -> Result<Next, Fail> {
        if j.overflow {
            return Err(Fail(500, "応答が長すぎます"));
        }
        self.send(hdr_buf, 200, "application/json; charset=utf-8", j.as_bytes(), close, false).await?;
        Ok(Next::KeepAlive)
    }

    async fn send_ok(&mut self, w: &mut Work<'_>, close: bool) -> Result<Next, Fail> {
        let mut j = Json::new(w.out);
        j.begin_object();
        j.field_bool("ok", true);
        j.end_object();
        self.send_json(w.hdr, &j, close).await
    }

    async fn send_status(&mut self, hdr_buf: &mut [u8], status: u16, close: bool) -> Result<(), Fail> {
        let mut hdr = HeaderWriter::new(hdr_buf);
        hdr.status(status);
        hdr.line("Allow: GET, POST");
        hdr.length(0);
        hdr.common(close);
        let n = hdr.finish();
        self.write_all(&hdr_buf[..n]).await
    }

    async fn error(&mut self, w: &mut Work<'_>, fail: Fail, close: bool) -> Result<(), Fail> {
        self.error_raw(w.hdr, w.out, fail, close).await
    }

    async fn error_raw(&mut self, hdr_buf: &mut [u8], out: &mut [u8], fail: Fail, close: bool) -> Result<(), Fail> {
        if fail.0 == 0 {
            return Ok(()); // 送信の失敗: もう送れない
        }
        defmt::warn!("web: {} {}", fail.0, fail.1);
        let mut j = Json::new(out);
        j.begin_object();
        j.field_bool("ok", false);
        j.field_int("status", i64::from(fail.0));
        j.field_str("error", fail.1);
        if fail.0 == 429
            && let Some(secs) = self.guard.locked(Instant::now().as_millis() as u32)
        {
            j.field_int("retry_after", i64::from(secs));
        }
        j.end_object();
        let len = j.len();
        let mut hdr = HeaderWriter::new(hdr_buf);
        hdr.status(fail.0);
        hdr.content_type("application/json; charset=utf-8");
        hdr.length(len);
        hdr.common(close);
        let n = hdr.finish();
        self.write_all(&hdr_buf[..n]).await?;
        self.write_all(&out[..len]).await
    }
}

/// 追加する写真の名前: 元の名前から作った 8.3 → 同じ名前があれば `~2`〜`~9` → `IMG00001.BMP`〜
fn pick_name(sd: &SdVolumeManager, original: &str) -> Option<String<12>> {
    let exists = |n: &str| -> bool {
        sdcard::with_deadline(SD_OP_DEADLINE_MS, || {
            sd.open_volume(embedded_sdmmc::VolumeIdx(0))
                .ok()
                .and_then(|v| v.open_root_dir().ok().map(|r| r.find_directory_entry(n).is_ok()))
                .unwrap_or(true)
        })
    };
    if let Some(name) = upload::short_name_from(original) {
        if !exists(&name) {
            return Some(name);
        }
        for i in 2..10 {
            let v = upload::variant(&name, i);
            if !exists(&v) {
                return Some(v);
            }
        }
    }
    (1..1000).map(upload::numbered).find(|v| !exists(v))
}

/// SD へ 512 B のブロック単位で書く (ファイルの先頭から。ブロックの途中から書くと読み直しが要るので揃える)
struct BlockWriter {
    /// `block` に溜まっている長さ
    fill: usize,
    /// 書き終えた長さ
    written: usize,
}

impl BlockWriter {
    /// `data` を溜め、512 B になるたびに書く
    fn push(&mut self, file: &sdcard::SdFile<'_>, block: &mut [u8], mut data: &[u8]) -> Result<(), Fail> {
        while !data.is_empty() {
            let n = data.len().min(512 - self.fill);
            block[self.fill..self.fill + n].copy_from_slice(&data[..n]);
            self.fill += n;
            data = &data[n..];
            if self.fill == 512 {
                self.flush(file, block)?;
            }
        }
        Ok(())
    }

    fn flush(&mut self, file: &sdcard::SdFile<'_>, block: &[u8]) -> Result<(), Fail> {
        if self.fill == 0 {
            return Ok(());
        }
        sdcard::with_deadline(SD_OP_DEADLINE_MS, || file.write(&block[..self.fill])).map_err(|_| Fail(503, "SD に書けません"))?;
        self.written += self.fill;
        self.fill = 0;
        supervisor::beat(Who::Web);
        Ok(())
    }
}

/// 要求の行き先 (ヘッダから決める。値は写して持つ)
enum Route {
    Page,
    Status,
    Settings,
    Images,
    Image(String<12>),
    Options,
    BadMethod,
    NotFound,
    ShowCode,
    Auth,
    SaveSettings,
    DeleteImage,
    Upload(String<64>),
    Reboot,
    OtaCheck,
}

impl Route {
    fn from(head: &Head<'_>) -> Self {
        match head.method {
            Method::Get => match head.path {
                "/" | "/index.html" => Route::Page,
                "/api/status" => Route::Status,
                "/api/settings" => Route::Settings,
                "/api/images" => Route::Images,
                p => match p.strip_prefix("/img/") {
                    Some(name) if upload::is_bmp_name(name) => {
                        let mut n: String<12> = String::new();
                        let _ = n.push_str(name);
                        Route::Image(n)
                    }
                    _ => Route::NotFound,
                },
            },
            Method::Options => Route::Options,
            Method::Other => Route::BadMethod,
            Method::Post => match head.path {
                "/api/show-code" => Route::ShowCode,
                "/api/auth" => Route::Auth,
                "/api/settings" => Route::SaveSettings,
                "/api/images/delete" => Route::DeleteImage,
                "/api/reboot" => Route::Reboot,
                "/api/ota-check" => Route::OtaCheck,
                "/api/upload" => {
                    let mut original: String<64> = String::new();
                    if let Some(raw) = http::query_param(head.query, "name") {
                        let mut tmp = [0u8; 64];
                        if let Some(n) = form::decode(raw, &mut tmp) {
                            for ch in n.chars() {
                                if original.push(ch).is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    Route::Upload(original)
                }
                _ => Route::NotFound,
            },
        }
    }

    fn is_post(&self) -> bool {
        matches!(
            self,
            Route::ShowCode | Route::Auth | Route::SaveSettings | Route::DeleteImage | Route::Upload(_) | Route::Reboot | Route::OtaCheck
        )
    }

    /// アクセスコードが要る (状態を変える要求。LCD に案内を出すだけの show-code は要らない)
    fn needs_code(&self) -> bool {
        self.is_post() && !matches!(self, Route::ShowCode)
    }
}

/// 設定ページが送ってよい設定の名前 (ticker.txt のキー。debug_crash / sdfast は手で書くものなので受けない)
const SETTING_KEYS: [&str; 11] = [
    "place", "lat", "lon", "tz", "layout", "slide", "status", "scroll", "message", "message_url", "images",
];

/// 要求ヘッダから写した値 (本文を読む間、`w.head` を借りたままにしないため、位置だけを持つ)
struct Req {
    head_len: usize,
    len: usize,
    body_len: u32,
    close: bool,
    expect_continue: bool,
}

impl Req {
    fn from_head(h: &Head<'_>, len: usize) -> Self {
        Self {
            head_len: h.head_len,
            len,
            body_len: h.content_length.unwrap_or(0),
            close: h.close,
            expect_continue: h.expect_continue,
        }
    }

    /// 解釈し直したヘッダ (同じ内容なので必ず成功する)
    fn view<'b>(&self, head: &'b [u8]) -> Head<'b> {
        http::parse_head(&head[..self.len]).unwrap_or(Head {
            method: Method::Other,
            path: "",
            query: "",
            host: None,
            origin: None,
            fetch_site: None,
            content_type: None,
            content_length: None,
            code: None,
            close: true,
            expect_continue: false,
            head_len: self.head_len,
        })
    }

    /// ヘッダと一緒に届いた本文の先頭
    fn pre_body<'b>(&self, head: &'b [u8]) -> &'b [u8] {
        let end = self.len.min(self.head_len + self.body_len as usize);
        &head[self.head_len.min(end)..end]
    }
}

/// 応答ヘッダを組み立てる (溢れたら切れるが、ヘッダは 1 kB 未満)
struct HeaderWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> HeaderWriter<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, s: &str) {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
    }

    fn line(&mut self, s: &str) {
        self.push(s);
        self.push("\r\n");
    }

    fn status(&mut self, status: u16) {
        let mut s: String<48> = String::new();
        let _ = write!(s, "HTTP/1.1 {} {}", status, http::reason(status));
        self.line(&s);
    }

    fn content_type(&mut self, ctype: &str) {
        self.push("Content-Type: ");
        self.line(ctype);
    }

    fn length(&mut self, len: usize) {
        let mut s: String<32> = String::new();
        let _ = write!(s, "Content-Length: {}", len);
        self.line(&s);
    }

    fn common(&mut self, close: bool) {
        self.line("Cache-Control: no-store");
        self.line("X-Content-Type-Options: nosniff");
        self.line("X-Frame-Options: DENY");
        self.line("Referrer-Policy: no-referrer");
        self.line(if close { "Connection: close" } else { "Connection: keep-alive" });
    }

    fn finish(mut self) -> usize {
        self.push("\r\n");
        self.len
    }
}

/// SD の処理全体の期限 (途中で落とす File / Volume の後始末も含めて必ず戻るように)。落とすと外す
struct SessionDeadline;

impl SessionDeadline {
    fn start(d: Duration) -> Self {
        sdcard::set_deadline(Some(Instant::now() + d));
        Self
    }
}

impl Drop for SessionDeadline {
    fn drop(&mut self) {
        sdcard::set_deadline(None);
    }
}

async fn lock_sd() -> Result<embassy_sync::mutex::MutexGuard<'static, embassy_sync::blocking_mutex::raw::ThreadModeRawMutex, ()>, Fail> {
    // 待つ間はスライドショーに次の写真を始めさせない
    hold_slideshow();
    let guard = with_timeout(SD_LOCK_TIMEOUT, slideshow::SD_LOCK.lock())
        .await
        .map_err(|_| Fail(503, "SD が写真の読み込みで使用中です。少し待ってください"))?;
    supervisor::beat(Who::Web);
    Ok(guard)
}

fn hold_slideshow() {
    let until = (Instant::now().as_millis() as u32).saturating_add(SD_HOLD_MS);
    slideshow::HOLD_UNTIL_MS.store(until, Ordering::Relaxed);
}

/// `/api/settings`: 今の設定 (Wi-Fi のパスワードは持っていないので出しようがない)
fn write_settings(j: &mut Json<'_>, app: &impl App, sd: bool) {
    let c = app.config();
    j.begin_object();
    j.field_str("place", &c.place);
    j.key("lat");
    j.float(c.lat, 4);
    j.key("lon");
    j.float(c.lon, 4);
    j.field_int("tz_offset_secs", i64::from(c.tz_offset_secs));
    j.field_str(
        "layout",
        match c.layout {
            config::LayoutName::Glass => "glass",
            config::LayoutName::Dock => "dock",
            config::LayoutName::Classic => "classic",
        },
    );
    j.field_int("slide", i64::from(c.slide_secs));
    j.field_str(
        "status",
        match c.status {
            config::StatusMode::Auto => "auto",
            config::StatusMode::Full => "full",
            config::StatusMode::Compact => "compact",
        },
    );
    j.field_int("scroll", i64::from(c.scroll_px));
    j.field_str("message_url", &c.message_url);
    j.field_bool("local_message", c.local_message);
    j.key("message");
    app.write_local_message(j);
    j.field_str("images", &c.images);
    j.field_bool("sd", sd);
    j.end_object();
}

/// `/api/images`: SD ルートの BMP (スライドショーが使うもの) と `images=` の順番
async fn list_images(sd: &'static SdVolumeManager, j: &mut Json<'_>, cfg: &TickerConfig) -> Result<(), Fail> {
    let guard = lock_sd().await?;
    j.begin_object();
    j.key("files");
    j.begin_array();
    let result = sdcard::with_deadline(SD_OP_DEADLINE_MS * 2, || {
        sdcard::for_each_root_bmp(sd, |name, size| {
            j.begin_object();
            j.field_str("name", name);
            j.field_int("size", i64::from(size));
            j.end_object();
        })
    });
    hold_slideshow();
    drop(guard);
    result.map_err(|_| Fail(503, "SD を読めません"))?;
    j.end_array();
    j.key("order");
    j.begin_array();
    for n in config::image_names(&cfg.images) {
        j.str(n);
    }
    j.end_array();
    j.field_int("max", upload::MAX_FILES as i64);
    j.field_int("upload_size", i64::from(upload::UPLOAD_SIZE));
    j.end_object();
    Ok(())
}

// ============================================================
// ticker.txt の書き換え (安全な書き方: TICKER.NEW → 読み戻し → TICKER.BAK → TICKER.TXT → 読み戻し)
// ============================================================

/// ticker.txt に `updates` を当てて書き、その場で反映する。`work` は 6 kB 以上の作業領域
async fn save_ticker_txt(sd: &'static SdVolumeManager, updates: &[Update<'_>], work: &mut [u8], app: &mut impl App) -> Result<(), Fail> {
    let (old, rest) = work.split_at_mut(CONFIG_MAX);
    let (new, rest) = rest.split_at_mut(CONFIG_MAX);
    let (verify, _) = rest.split_at_mut(CONFIG_MAX);
    let guard = lock_sd().await?;
    let _session = SessionDeadline::start(Duration::from_secs(10));
    // 今の内容 (無ければ空から)
    let old_len = match sdcard::with_deadline(SD_OP_DEADLINE_MS, || sdcard::read_root_file(sd, "TICKER.TXT", old)) {
        Ok(n) if n >= CONFIG_MAX => return Err(Fail(413, "ticker.txt が長すぎます (1536 バイトまで)")),
        Ok(n) => Some(n),
        Err(sdcard::ReadError::NotFound) => None,
        Err(sdcard::ReadError::Other(_)) => return Err(Fail(503, "ticker.txt を読めません")),
    };
    supervisor::beat(Who::Web);
    let base: &[u8] = match old_len {
        Some(n) => &old[..n],
        None => "# ticker.txt (設定ページで作成。書き方は docs/ticker.md)\n".as_bytes(),
    };
    let new_len = config::rewrite(base, updates, new).map_err(|e| match e {
        config::RewriteError::TooLong => Fail(413, "ticker.txt が長くなりすぎます (1536 バイトまで)"),
        config::RewriteError::NotUtf8 => Fail(422, "ticker.txt が UTF-8 ではありません"),
    })?;
    let new = &new[..new_len];
    // 1. TICKER.NEW に書いて読み戻す (ここで失敗しても TICKER.TXT はそのまま)
    write_file(sd, "TICKER.NEW", new, verify).map_err(|_| Fail(503, "SD に書けません (TICKER.NEW)"))?;
    supervisor::beat(Who::Web);
    // 2. 今の内容を TICKER.BAK へ
    if let Some(n) = old_len {
        write_file(sd, "TICKER.BAK", &old[..n], verify).map_err(|_| Fail(503, "SD に書けません (TICKER.BAK)"))?;
        supervisor::beat(Who::Web);
    }
    // 3. TICKER.TXT を書き換えて読み戻す。失敗したら TICKER.NEW (新しい内容) を残す (起動時にそれを読む)
    if write_file(sd, "TICKER.TXT", new, verify).is_err() {
        hold_slideshow();
        return Err(Fail(503, "ticker.txt の書き込みに失敗しました (TICKER.NEW に残しました)"));
    }
    supervisor::beat(Who::Web);
    // 4. TICKER.NEW を消す
    let _ = sdcard::with_deadline(SD_OP_DEADLINE_MS, || {
        let volume = sd.open_volume(embedded_sdmmc::VolumeIdx(0)).map_err(|_| ())?;
        let root = volume.open_root_dir().map_err(|_| ())?;
        root.delete_entry_in_dir("TICKER.NEW").map_err(|_| ())
    });
    hold_slideshow();
    drop(guard);
    let (cfg, _) = TickerConfig::parse(new);
    let message = config::message_text(new);
    defmt::info!("web: ticker.txt saved ({} bytes)", new_len);
    app.apply(&cfg, message);
    Ok(())
}

/// `name` を `data` で作り直し (無ければ作る)、読み戻して一致を確かめる
fn write_file(sd: &SdVolumeManager, name: &str, data: &[u8], verify: &mut [u8]) -> Result<(), ()> {
    sdcard::with_deadline(SD_OP_DEADLINE_MS, || {
        let volume = sd.open_volume(embedded_sdmmc::VolumeIdx(0)).map_err(|_| ())?;
        let root = volume.open_root_dir().map_err(|_| ())?;
        let file = root
            .open_file_in_dir(name, embedded_sdmmc::Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| ())?;
        file.write(data).map_err(|_| ())?;
        file.close().map_err(|_| ())
    })?;
    let n = sdcard::with_deadline(SD_OP_DEADLINE_MS, || sdcard::read_root_file(sd, name, verify)).map_err(|_| ())?;
    if n == data.len() && verify[..n] == *data { Ok(()) } else { Err(()) }
}
