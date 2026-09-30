//! 起動の方針 (0.4.2〜「OTA 到達保証」、ハードウェアに依存しない部分)
//!
//! 0.4.0 はスタック溢れで固まり、OTA の経路そのものが壊れていたので自分では直せなかった
//! (docs/ticker.md §8)。0.4.2 は「どんな壊れ方をしても、ウォッチドッグで再起動 → 最新の版を確認 →
//! 更新、までは必ず進む」ことを次の 3 段で保証する。この module はその判定だけを持ち、
//! `tools/ticker-tests` がホストで起動の流れごと模擬して確かめる (`core` だけに依存)。
//!
//! 1. **TBYB の buy 条件を強くする** ([`BuyGate`]): OTA で入った新しい版は、Wi-Fi + DHCP、OTA の
//!    manifest 確認 (TLS + HTTP を最後まで)、他の機能を一巡 (NTP / 天気 / 文字 / SD の設定 / 最初の写真)、
//!    その後 [`BUY_SETTLE_MS`] の健全な稼働、が揃うまで buy しない。途中で止まる / 落ちる版は buy されず、
//!    bootrom が前の (動いていた) 版へ戻す。
//! 2. **回復モード** ([`decide`] → [`Mode::Recovery`]): buy の後で落ちるようになった版は、
//!    [`RECOVERY_AFTER`] 回続けて異常終了したら、Wi-Fi + OTA だけの最小構成で起動する (SD も写真も
//!    天気も使わない)。
//! 3. **他方区画へ戻る** ([`Mode::Fallback`]): 回復モードでも [`FALLBACK_AFTER`] 回続けて落ちたら、
//!    他方区画 (前に buy された版) を FLASH_UPDATE 起動する。
//!
//! 記録の置き場所 (WATCHDOG.SCRATCH、電源断で消える):
//!
//! | レジスタ | 内容 |
//! |---|---|
//! | SCRATCH1 | [`BootState`] (連続異常終了の回数 / 回復モード中か / 他方区画へ戻したか) |
//! | SCRATCH0 | 他方区画へ戻す直前だけ [`fallback_marker`] (戻した版)。次の起動が読んでから `boot_trace::arm` が消す |
//!
//! 永続する記録 (フラッシュの data 区画、[`Record`]): Wi-Fi の資格情報の写し (回復モードが SD を読まずに
//! 済むように) と、他方区画へ戻す原因になった版 ([`Record::blocked`]、その版は二度と入れない)。

// ============================================================
// 版数
// ============================================================

/// IMAGE_DEF の版数語 (major << 16 | minor × 100 + patch)。`image_def::IMAGE_DEF_VERSION_WORD` と同じ形
pub const fn version_word(major: u16, minor: u16, patch: u16) -> u32 {
    (major as u32) << 16 | (minor as u32 * 100 + patch as u32)
}

/// 版数語 → (major, minor, patch)
pub const fn version_parts(word: u32) -> (u16, u16, u16) {
    let minor_patch = (word & 0xffff) as u16;
    ((word >> 16) as u16, minor_patch / 100, minor_patch % 100)
}

// ============================================================
// 連続異常終了の状態 (SCRATCH1)
// ============================================================

/// この回数続けて異常終了したら回復モード
pub const RECOVERY_AFTER: u8 = 2;
/// 回復モードでこの回数続けて異常終了したら他方区画へ戻す
pub const FALLBACK_AFTER: u8 = 3;
/// 通常モードで OTA 確認が通ってからこの時間異常なく動いたら回数を 0 に戻す
pub const HEALTHY_CLEAR_MS: u32 = 10 * 60 * 1000;
/// 回復モードの OTA 確認の周期
pub const RECOVERY_OTA_INTERVAL_MS: u32 = 60 * 1000;
/// 回復モードで OTA 確認が通り (新しい版が無く) この時間動いたら、通常モードをもう一度試す
pub const RECOVERY_NORMAL_RETRY_MS: u32 = 10 * 60 * 1000;

/// SCRATCH1 の上位 8 bit (0.4.2〜)
const STATE_MAGIC: u32 = 0xA5 << 24;
/// 0.4.1 の SCRATCH1 (`0xC0DE_nnnn` = 連続回数)。0.4.1 から戻ってきた / 0.4.1 へ戻したときも読めるように
const LEGACY_MAGIC: u32 = 0xC0DE_0000;
const FLAG_IN_RECOVERY: u32 = 1 << 16;
const FLAG_FELL_BACK: u32 = 1 << 17;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootState {
    /// 通常モードで続けて異常終了した回数
    pub crash_streak: u8,
    /// 回復モードで続けて異常終了した回数
    pub recovery_streak: u8,
    /// この起動 (次の起動から見れば前回) は回復モード
    pub in_recovery: bool,
    /// 他方区画へ戻そうとした (二度はしない)
    pub fell_back: bool,
}

impl BootState {
    pub fn decode(word: u32) -> Self {
        if word & 0xff00_0000 == STATE_MAGIC {
            Self {
                crash_streak: (word & 0xff) as u8,
                recovery_streak: ((word >> 8) & 0xff) as u8,
                in_recovery: word & FLAG_IN_RECOVERY != 0,
                fell_back: word & FLAG_FELL_BACK != 0,
            }
        } else if word & 0xffff_0000 == LEGACY_MAGIC {
            Self {
                crash_streak: (word & 0xffff).min(0xff) as u8,
                ..Self::default()
            }
        } else {
            Self::default()
        }
    }

    pub fn encode(self) -> u32 {
        STATE_MAGIC
            | u32::from(self.crash_streak)
            | u32::from(self.recovery_streak) << 8
            | if self.in_recovery { FLAG_IN_RECOVERY } else { 0 }
            | if self.fell_back { FLAG_FELL_BACK } else { 0 }
    }

    /// 回復モードから通常モードを試すときの状態 (もう 1 回落ちたらすぐ回復モードに戻る)
    pub fn for_normal_retry(self) -> Self {
        Self {
            crash_streak: RECOVERY_AFTER - 1,
            recovery_streak: 0,
            in_recovery: false,
            fell_back: self.fell_back,
        }
    }

    /// 通常モードで OTA 確認 + [`HEALTHY_CLEAR_MS`] 動いた (全部 0 に戻す)
    pub fn healthy() -> Self {
        Self::default()
    }
}

// ============================================================
// 他方区画へ戻した印 (SCRATCH0)
// ============================================================

const MARKER_MAGIC: u32 = 0xFB << 24;

/// 他方区画へ FLASH_UPDATE 起動する直前に SCRATCH0 へ書く値 (`from` = 戻す原因になった自分の版数語)。
/// bootrom は SCRATCH0 / 1 に書かない。panic の `&Location` (0x10…)、LR (0x10… / 0xFF…)、
/// `health::SUPERVISED_MARK` (0x53…) とは上位 8 bit で区別できる。
pub const fn fallback_marker(from: u32) -> u32 {
    MARKER_MAGIC | (from >> 16 & 0xf) << 20 | (from & 0xffff)
}

/// SCRATCH0 の値が [`fallback_marker`] なら、戻した版の版数語
pub const fn decode_fallback_marker(word: u32) -> Option<u32> {
    if word & 0xff00_0000 == MARKER_MAGIC && word & 0x000f_0000 == 0 {
        Some((word >> 20 & 0xf) << 16 | (word & 0xffff))
    } else {
        None
    }
}

// ============================================================
// 起動の判定
// ============================================================

/// 前回の起動が残した記録 (boot_trace の SCRATCH5〜7 と WATCHDOG.REASON から)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrevBoot {
    /// 記録を書いた版の版数語 (記録が無ければ None)
    pub trace_version: Option<u32>,
    /// panic / HardFault / スタック溢れ / 停止 / 未登録の割り込みを記録していた
    pub fault_recorded: bool,
    /// リセット理由がウォッチドッグの時間切れ (記録できないまま止まった)
    pub watchdog_timeout: bool,
}

/// 前回が「この版の異常終了」か。他の版の記録 (TBYB で試した新しい版が巻き戻った等) は数えない。
/// 0.4.2〜はウォッチドッグを main の最初から動かすので、この版の記録が残っていてリセット理由が
/// ウォッチドッグの時間切れなら、記録も残せずに止まった (ロックアップ、割り込み禁止のまま待ち続けた等)。
pub fn is_fault(prev: &PrevBoot, own_version: u32) -> bool {
    prev.trace_version == Some(own_version) && (prev.fault_recorded || prev.watchdog_timeout)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BootInputs {
    pub own_version: u32,
    /// 電源投入 / RUN ピン / デバッガ (WATCHDOG.REASON が 0。SCRATCH も消えている)
    pub hw_reset: bool,
    /// `reboot(FLASH_UPDATE)` 由来の起動 (OTA で書いた版、または他方区画へ戻した版)
    pub flash_update_boot: bool,
    /// TBYB の buy 待ち (OTA で届いたばかり。buy 条件を満たすまで旧版が残っている)
    pub tbyb_pending: bool,
    pub prev: PrevBoot,
    /// SCRATCH1
    pub state_word: u32,
    /// SCRATCH0 (`boot_trace::arm` の前に読んだ値)
    pub marker_word: u32,
    /// 他方区画 (A/B) が分かる
    pub other_slot_known: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 通常モード (TBYB の buy 待ちも含む)
    Normal,
    /// 回復モード: Wi-Fi + OTA だけ
    Recovery,
    /// 他方区画へ FLASH_UPDATE 起動する (すぐに)
    Fallback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BootPlan {
    pub mode: Mode,
    /// SCRATCH1 に書く値
    pub state: BootState,
    /// SCRATCH1 を書くか (TBYB の buy 待ちは書かない: 巻き戻ったとき旧版の回数を壊さないため)
    pub write_state: bool,
    /// 前回はこの版の異常終了だった
    pub fault: bool,
    /// 他の版がここへ戻してきた (その版の版数語)。その版は二度と入れない
    pub fell_back_from: Option<u32>,
    /// 他方区画へ戻そうとしたが、この版がまた起動した (他方区画に起動できるイメージが無い)
    pub fallback_failed: bool,
}

/// 起動の方針を決める。main の最初 (`boot_trace::arm` の前) に 1 回呼ぶ。
///
/// - TBYB の buy 待ち: 常に通常モード (buy 条件で試す)。SCRATCH1 は書かない。
/// - 電源投入: 回数は 0 から。
/// - FLASH_UPDATE 起動: OTA で入った版は 0 から。SCRATCH0 に他の版の [`fallback_marker`] があれば
///   「その版がここへ戻してきた」、自分の版の印なら「戻そうとしたが戻れなかった」(以後は回復モードのまま)。
/// - それ以外 (ウォッチドッグ / 強制リセット): SCRATCH1 を引き継ぎ、前回がこの版の異常終了なら数える。
pub fn decide(inp: &BootInputs) -> BootPlan {
    if inp.tbyb_pending {
        return BootPlan {
            mode: Mode::Normal,
            state: BootState::default(),
            write_state: false,
            fault: false,
            fell_back_from: None,
            fallback_failed: false,
        };
    }
    let marker = decode_fallback_marker(inp.marker_word);
    let mut fell_back_from = None;
    let mut fallback_failed = false;
    let mut state = if inp.hw_reset {
        BootState::default()
    } else if inp.flash_update_boot {
        match marker {
            Some(from) if from == inp.own_version => {
                fallback_failed = true;
                let mut s = BootState::decode(inp.state_word);
                s.fell_back = true;
                s.crash_streak = s.crash_streak.max(RECOVERY_AFTER);
                s.recovery_streak = 0;
                s
            }
            Some(from) => {
                fell_back_from = Some(from);
                BootState::default()
            }
            None => BootState::default(),
        }
    } else {
        BootState::decode(inp.state_word)
    };
    // FLASH_UPDATE 起動の記録 (SCRATCH5 = bootrom の値) や電源投入後は前回の異常終了ではない
    let fault = !inp.hw_reset && !inp.flash_update_boot && is_fault(&inp.prev, inp.own_version);
    if fault {
        if state.in_recovery {
            state.recovery_streak = state.recovery_streak.saturating_add(1);
        } else {
            state.crash_streak = state.crash_streak.saturating_add(1);
        }
    }
    let mode = if state.crash_streak >= RECOVERY_AFTER || state.in_recovery {
        if state.recovery_streak >= FALLBACK_AFTER && !state.fell_back && inp.other_slot_known {
            Mode::Fallback
        } else {
            Mode::Recovery
        }
    } else {
        Mode::Normal
    };
    if mode == Mode::Fallback {
        state.fell_back = true;
        state.recovery_streak = 0;
    }
    state.in_recovery = mode != Mode::Normal;
    BootPlan {
        mode,
        state,
        write_state: true,
        fault,
        fell_back_from,
        fallback_failed,
    }
}

// ============================================================
// TBYB の buy 条件
// ============================================================

/// buy を許す締め切り (起動からの ms)。過ぎたら buy せず、ウォッチドッグの再ロードもやめて旧版へ戻る
pub const BUY_DEADLINE_MS: u32 = 180_000;
/// 条件が揃ってから buy するまでの健全な稼働時間
pub const BUY_SETTLE_MS: u32 = 25_000;
/// buy 待ちの間、OTA 確認が通信の失敗 (DNS / TCP / TLS / 時間切れ) で終わったら次を試すまでの時間
pub const PENDING_OTA_RETRY_MS: u32 = 10_000;

/// 通常モードの「一巡」: 各機能を 1 回ずつ試したか (成否は問わない。落ちずに戻ってきたか)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Round {
    pub sd_config: bool,
    pub ntp: bool,
    pub weather: bool,
    pub message: bool,
    /// 最初の写真の読み込み (写真が無い / SD が無いときは試すものが無いので済み)
    pub slideshow: bool,
    /// 設定ページの HTTP サーバが待ち受けを始めた (0.5.0〜。最初の OTA 確認が通った後に始まる)
    pub web: bool,
}

impl Round {
    /// 一巡を持たない bin (wifi_ota) 用
    pub const DONE: Round = Round {
        sd_config: true,
        ntp: true,
        weather: true,
        message: true,
        slideshow: true,
        web: true,
    };

    pub fn done(&self) -> bool {
        self.first_missing().is_none()
    }

    /// まだ試していない最初の機能 (LCD の `wait:` に出す名前)
    pub fn first_missing(&self) -> Option<&'static str> {
        [
            (self.sd_config, "sd"),
            (self.web, "web"),
            (self.ntp, "ntp"),
            (self.weather, "weather"),
            (self.message, "message"),
            (self.slideshow, "photo"),
        ]
        .into_iter()
        .find(|(done, _)| !done)
        .map(|(_, name)| name)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuyInputs {
    pub now_ms: u32,
    /// (a) join + DHCP で IP がある
    pub network_up: bool,
    /// (b) OTA の manifest 確認が TLS + HTTP を最後まで通った ([`CheckOutcome::Proved`] が一度でも出た)
    pub ota_proved: bool,
    /// (c) 他の機能の一巡
    pub round: Round,
    /// 生存確認 (main / 取得 / 描画) が揃っている
    pub healthy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyStep {
    /// 条件待ち (`missing` = 足りない条件の名前)
    Waiting { missing: &'static str },
    /// 条件は揃った。健全な稼働をあと `left_ms` 待つ
    Settling { left_ms: u32 },
    /// buy する
    Buy,
    /// 締め切りを過ぎた。buy しない (旧版へ戻る)
    TimedOut,
}

/// buy 条件の判定 (TBYB の buy 待ちの間、main ループで毎回 [`tick`](Self::tick) を呼ぶ)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuyGate {
    pub deadline_ms: u32,
    pub settle_ms: u32,
    ready_since: Option<u32>,
    finished: bool,
}

impl BuyGate {
    pub const fn new(deadline_ms: u32, settle_ms: u32) -> Self {
        Self {
            deadline_ms,
            settle_ms,
            ready_since: None,
            finished: false,
        }
    }

    /// 足りない条件の名前 (揃っていれば None)
    pub fn missing(inp: &BuyInputs) -> Option<&'static str> {
        if !inp.network_up {
            Some("wifi")
        } else if !inp.ota_proved {
            Some("ota")
        } else if let Some(name) = inp.round.first_missing() {
            Some(name)
        } else if !inp.healthy {
            Some("health")
        } else {
            None
        }
    }

    /// 1 回の判定。`Buy` / `TimedOut` は 1 度だけ返し、以後は `Waiting` を返す
    pub fn tick(&mut self, inp: &BuyInputs) -> BuyStep {
        if self.finished {
            return BuyStep::Waiting { missing: "done" };
        }
        if inp.now_ms >= self.deadline_ms {
            self.finished = true;
            return BuyStep::TimedOut;
        }
        if let Some(missing) = Self::missing(inp) {
            // 一度揃っても、途中で Wi-Fi が落ちる / 生存確認が途切れたら待ち直す
            self.ready_since = None;
            return BuyStep::Waiting { missing };
        }
        let since = *self.ready_since.get_or_insert(inp.now_ms);
        let waited = inp.now_ms.saturating_sub(since);
        if waited >= self.settle_ms {
            self.finished = true;
            BuyStep::Buy
        } else {
            BuyStep::Settling {
                left_ms: self.settle_ms - waited,
            }
        }
    }
}

/// OTA の manifest 確認の結果の分類 (buy 条件 (b) と、buy 待ち中の再試行の間隔)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    /// manifest.json を解釈できた、または (リダイレクトを追った後の) 確定した HTTP ステータス (404 / 5xx など)
    /// を受けた。この版の TLS + HTTP の経路が最後まで動いた証拠になる
    Proved,
    /// 証拠にならない: DNS / TCP / TLS / 時間切れ、応答ヘッダの溢れ、応答の構文エラー、リダイレクトの不備、
    /// 途中で切れた / 解釈できない manifest。buy 待ちなら [`PENDING_OTA_RETRY_MS`] 後に再試行し、締め切りまで
    /// 通らなければ buy しない (旧版へ戻る)
    Unproved,
}

/// OTA 確認の失敗の種類 (`ota::OtaError` をこれに写して [`classify_check`] に渡す)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckFailure {
    /// リダイレクトを追った後の確定した HTTP ステータス (200 / 302 / 404 以外)
    FinalStatus,
    /// 応答は来たが使えない: ヘッダがバッファに収まらない、構文エラー、Location の不備 / 長すぎ / 多すぎ、
    /// manifest.json が長すぎる / 解釈できない
    BadResponse,
    /// DNS / TCP / TLS / HTTP クライアント内部 / 時間切れ
    Transport,
    /// 手元の問題 (区画、フラッシュ、書いたイメージの検証)。manifest を解釈した後にしか起きない
    Local,
}

/// OTA 確認 1 回の分類。`failure` = None なら成功 (manifest を解釈した、または 404 = Release 無し)。
/// manifest を解釈した後 (`manifest_parsed`) の失敗 (ダウンロードや検証) は経路が通った後なので `Proved`。
/// 解釈する前の失敗は、確定した HTTP ステータスだけが `Proved`。
pub fn classify_check(manifest_parsed: bool, failure: Option<CheckFailure>) -> CheckOutcome {
    match failure {
        None => CheckOutcome::Proved,
        Some(_) if manifest_parsed => CheckOutcome::Proved,
        Some(CheckFailure::FinalStatus) => CheckOutcome::Proved,
        Some(CheckFailure::BadResponse | CheckFailure::Transport | CheckFailure::Local) => CheckOutcome::Unproved,
    }
}

// ============================================================
// data 区画の記録 (Wi-Fi の資格情報の写し + 入れない版)
// ============================================================

/// 記録の先頭 ("AKP1")
pub const RECORD_MAGIC: u32 = 0x3150_4B41;
/// 記録の長さ (バイト。フラッシュの 1 ページ 256 B に収まる)
pub const RECORD_LEN: usize = 112;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub password: [u8; 64],
    pub password_len: u8,
    /// この版以下は OTA で入れない (他方区画へ戻す原因になった版の版数語。0 = 無し)
    pub blocked: u32,
}

impl Default for Record {
    fn default() -> Self {
        Self {
            ssid: [0; 32],
            ssid_len: 0,
            password: [0; 64],
            password_len: 0,
            blocked: 0,
        }
    }
}

impl Record {
    pub fn with_credentials(mut self, ssid: &[u8], password: &[u8]) -> Self {
        self.ssid = [0; 32];
        self.password = [0; 64];
        let n = ssid.len().min(32);
        self.ssid[..n].copy_from_slice(&ssid[..n]);
        self.ssid_len = n as u8;
        let n = password.len().min(64);
        self.password[..n].copy_from_slice(&password[..n]);
        self.password_len = n as u8;
        self
    }

    pub fn ssid(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len).min(32)]
    }

    pub fn password(&self) -> &[u8] {
        &self.password[..usize::from(self.password_len).min(64)]
    }

    pub fn has_credentials(&self) -> bool {
        self.ssid_len > 0
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut out = [0u8; RECORD_LEN];
        out[0..4].copy_from_slice(&RECORD_MAGIC.to_le_bytes());
        out[4] = 1; // 形式の版
        out[5] = self.ssid_len;
        out[6] = self.password_len;
        out[8..12].copy_from_slice(&self.blocked.to_le_bytes());
        out[12..44].copy_from_slice(&self.ssid);
        out[44..108].copy_from_slice(&self.password);
        let crc = crc32(&out[..108]);
        out[108..112].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// 壊れている / 書かれていない (0xFF) なら None
    pub fn decode(bytes: &[u8]) -> Option<Record> {
        if bytes.len() < RECORD_LEN {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        if word(0) != RECORD_MAGIC || bytes[4] != 1 || word(108) != crc32(&bytes[..108]) {
            return None;
        }
        let (ssid_len, password_len) = (bytes[5], bytes[6]);
        if ssid_len > 32 || password_len > 64 {
            return None;
        }
        let mut r = Record {
            ssid_len,
            password_len,
            blocked: word(8),
            ..Record::default()
        };
        r.ssid.copy_from_slice(&bytes[12..44]);
        r.password.copy_from_slice(&bytes[44..108]);
        Some(r)
    }

    /// `version` (版数語) を入れてよいか (他方区画へ戻した版以下は入れない)
    pub fn allows(&self, version: u32) -> bool {
        self.blocked == 0 || version > self.blocked
    }
}

/// CRC-32 (IEEE 802.3、ビットごとの計算。112 B に 1 回だけなので表は持たない)
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}
