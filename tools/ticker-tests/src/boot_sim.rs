//! 起動の流れの模擬 (0.4.2〜「OTA 到達保証」の確認)
//!
//! 本物の `boot_policy` (起動の方針、buy 条件、SCRATCH の語、data 区画の記録) を使い、まわりの
//! ハードウェアを簡単な模型にして、故障を注入したときに
//!
//! - 壊れた版 (最初の一巡のどこかで止まる / 落ちる) を **buy しない**、
//! - どの状態からでも **限られた時間 / 起動回数のうちに OTA 確認 (TLS + HTTP) まで進む**、
//! - 直した版を配れば、それが **動く**、
//!
//! ことを確かめる。模型にしたもの:
//!
//! - bootrom (pico-bootrom-rp2350 の `varm_flash_boot.c` / `varm_launch_image.c` を読んで写した):
//!   通常起動は buy 済み (TBYB でない) 正しいイメージのうち版数の大きい方。FLASH_UPDATE 起動は対象区画に
//!   正しいイメージがあれば版数によらずそれ (無ければ通常と同じ選び方)。対象の方が版数が小さければ他方を
//!   消す (TBYB でなければ起動時、TBYB なら buy 時)。TBYB のイメージは FLASH_UPDATE の対象のときだけ起動する。
//! - WATCHDOG.SCRATCH0 / 1 (リセットで残る、電源断で消える)、SCRATCH5〜7 の記録 (FLASH_UPDATE で消える)。
//! - ファームウェアの段階と所要時間、ウォッチドッグ (初期化中の停止は 8 s で記録なし、タスクの停止は 90 s で記録あり)、
//!   OTA (60 s ごと、巻き戻った版の再試行 10 分後、ダウンロード 60 s)、回復モード (60 s ごとの確認、10 分で通常を再試行)。

use crate::boot_policy::{
    self, BUY_DEADLINE_MS, BUY_SETTLE_MS, BootInputs, BootState, BuyGate, BuyInputs, BuyStep, CheckFailure, CheckOutcome,
    HEALTHY_CLEAR_MS, Mode,
    PENDING_OTA_RETRY_MS, PrevBoot, RECOVERY_NORMAL_RETRY_MS, RECOVERY_OTA_INTERVAL_MS, Record, Round,
};

// ============================================================
// 模型
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Display,
    Sd,
    Cyw43,
    Join,
    Dhcp,
    OtaTls,
    Ntp,
    Weather,
    Message,
    Slideshow,
}

/// 通常モードの段階と所要時間 (ms)
const NORMAL: [(St, u64); 10] = [
    (St::Display, 100),
    (St::Sd, 800),
    (St::Cyw43, 1_500),
    (St::Join, 3_000),
    (St::Dhcp, 2_000),
    (St::OtaTls, 6_000),
    (St::Ntp, 1_000),
    (St::Weather, 3_000),
    (St::Message, 3_000),
    (St::Slideshow, 2_000),
];
/// 回復モード (SD / NTP / 天気 / 文字 / 写真は無い)
const RECOVERY: [(St, u64); 5] = [
    (St::Display, 100),
    (St::Cyw43, 1_500),
    (St::Join, 3_000),
    (St::Dhcp, 2_000),
    (St::OtaTls, 6_000),
];

/// 初期化中 (生存確認の監視が始まる前) の段階。ここで止まると 8 s でウォッチドッグ (記録なし)
fn is_init(st: St) -> bool {
    matches!(st, St::Display | St::Sd)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fail {
    /// その段階で panic / HardFault (記録してすぐリセット)
    Crash(St),
    /// その段階で止まる (初期化中ならウォッチドッグ 8 s、以後は生存確認 90 s)
    Hang(St),
    /// 一巡が終わってから `ms` 後に落ちる (毎回)
    CrashAfter(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    pub version: u32,
    /// 通常モードの故障
    pub fail: Option<Fail>,
    /// 回復モードの故障
    pub recovery_fail: Option<Fail>,
}

impl Image {
    pub fn good(version: u32) -> Self {
        Self {
            version,
            fail: None,
            recovery_fail: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    img: Image,
    /// TBYB のまま (buy されていない)
    tbyb: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Github {
    Ok,
    /// TLS は通るが 5xx (確定したステータス = 経路の証明になる)
    Http5xx,
    /// DNS / TCP / TLS が通らない
    Down,
    /// 応答は来るが使えない (ヘッダがバッファに溢れる、構文エラー、manifest が切れている)
    BadResponse,
}

pub struct Env {
    /// この時間帯 (ms、[from, until)) は Wi-Fi に入れない
    pub network_down: (u64, u64),
    /// GitHub の状態 (時刻 → 状態)
    pub github: fn(u64) -> Github,
    /// Release (公開時刻, イメージ)。manifest は公開済みの最後のもの
    pub releases: Vec<(u64, Image)>,
    /// 電源を入れ直す時刻
    pub power_cycles: Vec<u64>,
}

impl Env {
    fn latest(&self, t: u64) -> Option<Image> {
        self.releases.iter().filter(|(at, _)| *at <= t).map(|(_, img)| *img).last()
    }
    fn net_up(&self, t: u64) -> bool {
        !(self.network_down.0..self.network_down.1).contains(&t)
    }
    /// Wi-Fi に入れる時刻 (join + DHCP の 5 s 込み)
    fn net_up_at(&self, t: u64) -> u64 {
        if self.net_up(t) { t } else { self.network_down.1 + 5_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reset {
    Hw,
    /// ウォッチドッグの時間切れ (記録できないまま止まった、TBYB の締め切り)
    WdtTimer,
    /// 強制リセット (panic などの記録の後、回復モードの通常再試行)
    Force,
    /// reboot(FLASH_UPDATE) で区画 n へ
    FlashUpdate(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    Boot { t: u64, version: u32, mode: Mode, pending: bool },
    /// OTA 確認が TLS + HTTP を通った
    Proved { t: u64, version: u32, recovery: bool },
    Bought { t: u64, version: u32 },
    Download { t: u64, version: u32 },
    Fallback { t: u64, from: u32 },
    Bricked { t: u64 },
}

pub struct Board {
    slots: [Option<Slot>; 2],
    s0: u32,
    s1: u32,
    /// SCRATCH5〜7 の記録: (書いた版, 異常終了を記録したか)
    trace: Option<(u32, bool)>,
    record: Record,
    pub t: u64,
    pub log: Vec<Ev>,
    /// 起動回数の上限 (これを超えたら「きつい繰り返し」として失敗)
    pub max_boots: usize,
    /// buy した版
    pub bought: Vec<u32>,
    pub boots: usize,
}

/// 1 回の起動の結果
enum End {
    Reset(Reset),
    /// 模擬の終わりまで動いた
    Done,
}

impl Board {
    pub fn new(slot_a: Image) -> Self {
        Self {
            slots: [Some(Slot { img: slot_a, tbyb: false }), None],
            s0: 0,
            s1: 0,
            trace: None,
            record: Record::default(),
            t: 0,
            log: Vec::new(),
            bought: Vec::new(),
            boots: 0,
            max_boots: 2_000,
        }
    }

    /// bootrom: 起動する区画と、その起動が TBYB の buy 待ちか、buy 時に消す区画
    fn pick(&mut self, reset: Reset) -> Option<(usize, bool, bool, Option<usize>)> {
        let valid = |s: &Option<Slot>| s.is_some();
        let normal_pick = |slots: &[Option<Slot>; 2]| {
            (0..2)
                .filter(|&i| slots[i].is_some_and(|s| !s.tbyb))
                .max_by_key(|&i| slots[i].unwrap().img.version)
        };
        match reset {
            Reset::FlashUpdate(target) if valid(&self.slots[target]) => {
                let other = 1 - target;
                let chosen = self.slots[target].unwrap();
                let other_greater = self.slots[other].is_some_and(|o| o.img.version > chosen.img.version);
                let mut erase_at_buy = None;
                if other_greater {
                    if chosen.tbyb {
                        erase_at_buy = Some(other);
                    } else {
                        // 版数の巻き戻し: TBYB でないイメージは起動時に他方を消す
                        self.slots[other] = None;
                    }
                }
                Some((target, chosen.tbyb, true, erase_at_buy))
            }
            Reset::FlashUpdate(_) => normal_pick(&self.slots).map(|i| (i, false, true, None)),
            _ => normal_pick(&self.slots).map(|i| (i, false, false, None)),
        }
    }

    /// 模擬を `t_end` まで回す
    pub fn run(&mut self, env: &Env, t_end: u64) {
        let mut reset = Reset::Hw;
        let mut power = env.power_cycles.clone();
        power.sort();
        while self.t < t_end {
            self.boots += 1;
            assert!(self.boots < self.max_boots, "too many boots (tight loop?)");
            if let Some(&pc) = power.first()
                && self.t >= pc
            {
                power.remove(0);
                reset = Reset::Hw;
            }
            if reset == Reset::Hw {
                self.s0 = 0;
                self.s1 = 0;
                self.trace = None;
            }
            if matches!(reset, Reset::FlashUpdate(_)) {
                // bootrom の reboot() が SCRATCH5〜7 を書く (記録は消える)
                self.trace = None;
            }
            let next_power = power.first().copied().unwrap_or(u64::MAX).min(t_end);
            match self.boot(env, reset, next_power) {
                End::Reset(r) => reset = r,
                End::Done => {
                    if self.t >= t_end {
                        return;
                    }
                    reset = Reset::Hw;
                }
            }
        }
    }

    fn boot(&mut self, env: &Env, reset: Reset, until: u64) -> End {
        let Some((slot, pending, flash_update, erase_at_buy)) = self.pick(reset) else {
            self.log.push(Ev::Bricked { t: self.t });
            self.t = u64::MAX;
            return End::Done;
        };
        let img = self.slots[slot].unwrap().img;
        let own = img.version;
        let inputs = BootInputs {
            own_version: own,
            hw_reset: reset == Reset::Hw,
            flash_update_boot: flash_update,
            tbyb_pending: pending,
            prev: PrevBoot {
                trace_version: self.trace.map(|(v, _)| v),
                fault_recorded: self.trace.is_some_and(|(_, f)| f),
                watchdog_timeout: reset == Reset::WdtTimer,
            },
            state_word: self.s1,
            marker_word: self.s0,
            other_slot_known: true,
        };
        let plan = boot_policy::decide(&inputs);
        if plan.write_state {
            self.s1 = plan.state.encode();
        }
        // boot_trace::arm
        self.s0 = 0;
        self.trace = Some((own, false));
        self.log.push(Ev::Boot {
            t: self.t,
            version: own,
            mode: plan.mode,
            pending,
        });
        match plan.mode {
            Mode::Fallback => {
                self.log.push(Ev::Fallback { t: self.t, from: own });
                self.s0 = boot_policy::fallback_marker(own);
                self.t += 50;
                End::Reset(Reset::FlashUpdate(1 - slot))
            }
            Mode::Recovery => self.recovery(env, slot, img, plan.state, until),
            Mode::Normal => {
                if let Some(from) = plan.fell_back_from {
                    self.record.blocked = self.record.blocked.max(from);
                }
                self.normal(env, slot, img, pending, erase_at_buy, until)
            }
        }
    }

    /// 落ちた / 止まった (段階 `st` で `fail`)。戻り値は次のリセット
    fn fail_at(&mut self, fail: Fail, st: St, start: u64) -> Reset {
        match fail {
            Fail::Hang(_) if is_init(st) => {
                self.t = start + 8_000;
                Reset::WdtTimer
            }
            Fail::Hang(_) => {
                self.t = start + 90_000;
                self.trace = self.trace.map(|(v, _)| (v, true));
                Reset::Force
            }
            _ => {
                // 記録してすぐリセット (bootrom + 起動の分を含めて 100 ms)
                self.t = start + 100;
                self.trace = self.trace.map(|(v, _)| (v, true));
                Reset::Force
            }
        }
    }

    fn hits(fail: Option<Fail>, st: St) -> Option<Fail> {
        match fail {
            Some(f @ (Fail::Crash(s) | Fail::Hang(s))) if s == st => Some(f),
            _ => None,
        }
    }

    /// OTA 確認 1 回 (TLS + HTTP)。戻り値: 経路が通ったか (本物の `classify_check` で分類する)
    fn ota_proved(&self, env: &Env) -> bool {
        if !env.net_up(self.t) {
            return false;
        }
        let (parsed, failure) = match (env.github)(self.t) {
            Github::Ok => (true, None),
            Github::Http5xx => (false, Some(CheckFailure::FinalStatus)),
            Github::BadResponse => (false, Some(CheckFailure::BadResponse)),
            Github::Down => (false, Some(CheckFailure::Transport)),
        };
        boot_policy::classify_check(parsed, failure) == CheckOutcome::Proved
    }

    /// 確認で見つかった入れるべき版 (5xx なら manifest は読めない)
    fn newer(&self, env: &Env, own: u32) -> Option<Image> {
        if (env.github)(self.t) != Github::Ok {
            return None;
        }
        env.latest(self.t)
            .filter(|l| l.version > own && self.record.allows(l.version))
    }

    /// 新しい版を他方区画へ書く (同じ版が TBYB のまま残っていれば巻き戻った版なので 10 分後に再試行)。
    /// 戻り値: 今すぐ FLASH_UPDATE 起動するか / 再試行の時刻
    fn install(&mut self, slot: usize, img: Image) -> Result<Reset, u64> {
        let other = 1 - slot;
        if self.slots[other].is_some_and(|s| s.img == img && s.tbyb) {
            return Err(self.t + 600_000);
        }
        self.t += 60_000;
        self.log.push(Ev::Download {
            t: self.t,
            version: img.version,
        });
        self.slots[other] = Some(Slot { img, tbyb: true });
        self.t += 2_000;
        Ok(Reset::FlashUpdate(other))
    }

    fn normal(&mut self, env: &Env, slot: usize, img: Image, pending: bool, erase_at_buy: Option<usize>, until: u64) -> End {
        let own = img.version;
        let boot_at = self.t;
        let mut gate = BuyGate::new(BUY_DEADLINE_MS, BUY_SETTLE_MS);
        let mut proved = false;
        let mut proved_at = 0;
        let mut newer_seen = false;
        let mut rejected_retry: Option<u64> = None;
        let mut round = Round {
            sd_config: false,
            ..Round::default()
        };
        let deadline = boot_at + u64::from(BUY_DEADLINE_MS);
        for (st, dur) in NORMAL {
            let start = self.t;
            if let Some(f) = Self::hits(img.fail, st) {
                return End::Reset(self.fail_at(f, st, start));
            }
            self.t += dur;
            match st {
                St::Sd => round.sd_config = true,
                St::Join => {
                    // Wi-Fi が戻るまで待つ (buy 待ちは締め切りまで)
                    let up_at = env.net_up_at(self.t);
                    if pending && up_at > deadline {
                        self.t = deadline + 8_000;
                        return End::Reset(Reset::WdtTimer);
                    }
                    self.t = up_at;
                    if self.t >= until {
                        return End::Done;
                    }
                }
                St::OtaTls => loop {
                    if self.ota_proved(env) {
                        proved = true;
                        proved_at = self.t;
                        self.log.push(Ev::Proved {
                            t: self.t,
                            version: own,
                            recovery: false,
                        });
                        if let Some(n) = self.newer(env, own) {
                            if pending {
                                newer_seen = true;
                            } else {
                                match self.install(slot, n) {
                                    Ok(r) => return End::Reset(r),
                                    Err(retry) => rejected_retry = Some(retry),
                                }
                            }
                        }
                        break;
                    }
                    // 通信の失敗: buy 待ちは 10 s ごと、通常は 60 s ごとに試し直す (模擬ではその間ほかの取得も待つ)
                    self.t += if pending { u64::from(PENDING_OTA_RETRY_MS) } else { 60_000 };
                    if pending && self.t >= deadline {
                        self.t = deadline + 8_000;
                        return End::Reset(Reset::WdtTimer);
                    }
                    if self.t >= until {
                        return End::Done;
                    }
                },
                St::Ntp => round.ntp = true,
                St::Weather => round.weather = true,
                St::Message => round.message = true,
                St::Slideshow => round.slideshow = true,
                _ => {}
            }
        }
        let round_done_at = self.t;
        let crash_at = match img.fail {
            Some(Fail::CrashAfter(ms)) => Some(round_done_at + ms),
            _ => None,
        };
        // --- TBYB: buy 条件 (本物の BuyGate を 1 s ごとに回す) ---
        if pending {
            loop {
                if crash_at.is_some_and(|c| self.t >= c) {
                    self.trace = self.trace.map(|(v, _)| (v, true));
                    return End::Reset(Reset::Force);
                }
                let step = gate.tick(&BuyInputs {
                    now_ms: (self.t - boot_at) as u32,
                    network_up: env.net_up(self.t),
                    ota_proved: proved,
                    round,
                    healthy: true,
                });
                match step {
                    BuyStep::Buy => break,
                    BuyStep::TimedOut => {
                        self.t += 8_000;
                        return End::Reset(Reset::WdtTimer);
                    }
                    _ => self.t += 1_000,
                }
            }
            self.slots[slot].as_mut().unwrap().tbyb = false;
            if let Some(o) = erase_at_buy {
                self.slots[o] = None;
            }
            self.s1 = BootState::default().encode();
            self.bought.push(own);
            self.log.push(Ev::Bought { t: self.t, version: own });
            if newer_seen && let Some(n) = self.newer(env, own) {
                match self.install(slot, n) {
                    Ok(r) => return End::Reset(r),
                    Err(_) => {}
                }
            }
        }
        // --- 通常運転: 60 s ごとの OTA 確認、落ちる版は落ちる ---
        let mut cleared = false;
        loop {
            let next_check = self.t + 60_000;
            // 落ちる / 模擬の終わり / 巻き戻った版の再試行 / 次の確認、の早い方
            if let Some(c) = crash_at
                && c <= next_check
            {
                if proved && !cleared && c >= proved_at + u64::from(HEALTHY_CLEAR_MS) {
                    self.s1 = BootState::healthy().encode();
                }
                self.t = c.max(self.t);
                self.trace = self.trace.map(|(v, _)| (v, true));
                return End::Reset(Reset::Force);
            }
            if next_check >= until {
                self.t = until;
                return End::Done;
            }
            self.t = next_check;
            if proved && !cleared && self.t >= proved_at + u64::from(HEALTHY_CLEAR_MS) {
                self.s1 = BootState::healthy().encode();
                cleared = true;
            }
            if let Some(r) = rejected_retry
                && self.t >= r
            {
                self.t += 2_000;
                return End::Reset(Reset::FlashUpdate(1 - slot));
            }
            if self.ota_proved(env) {
                if !proved {
                    proved = true;
                    proved_at = self.t;
                }
                self.log.push(Ev::Proved {
                    t: self.t,
                    version: own,
                    recovery: false,
                });
                if let Some(n) = self.newer(env, own) {
                    match self.install(slot, n) {
                        Ok(r) => return End::Reset(r),
                        Err(retry) => {
                            rejected_retry.get_or_insert(retry);
                        }
                    }
                }
            }
        }
    }

    fn recovery(&mut self, env: &Env, slot: usize, img: Image, state: BootState, until: u64) -> End {
        let own = img.version;
        for (st, dur) in RECOVERY {
            let start = self.t;
            if let Some(f) = Self::hits(img.recovery_fail, st) {
                return End::Reset(self.fail_at(f, st, start));
            }
            self.t += dur;
            if st == St::Join {
                self.t = env.net_up_at(self.t);
                if self.t >= until {
                    return End::Done;
                }
            }
        }
        let started = self.t;
        let crash_at = match img.recovery_fail {
            Some(Fail::CrashAfter(ms)) => Some(started + ms),
            _ => None,
        };
        let mut normal_retry_at: Option<u64> = None;
        let mut rejected_retry: Option<u64> = None;
        loop {
            if crash_at.is_some_and(|c| self.t >= c) {
                self.trace = self.trace.map(|(v, _)| (v, true));
                return End::Reset(Reset::Force);
            }
            if self.t >= until {
                return End::Done;
            }
            if let Some(r) = rejected_retry
                && self.t >= r
            {
                return End::Reset(Reset::FlashUpdate(1 - slot));
            }
            if let Some(at) = normal_retry_at
                && self.t >= at
            {
                // 通常モードをもう一度 (もう 1 回落ちたらすぐ回復モード)。意図したリセットなので記録は消す
                self.s1 = state.for_normal_retry().encode();
                self.trace = None;
                return End::Reset(Reset::Force);
            }
            if self.ota_proved(env) {
                self.log.push(Ev::Proved {
                    t: self.t,
                    version: own,
                    recovery: true,
                });
                match self.newer(env, own) {
                    Some(n) => match self.install(slot, n) {
                        Ok(r) => return End::Reset(r),
                        Err(retry) => {
                            rejected_retry.get_or_insert(retry);
                        }
                    },
                    None => {
                        normal_retry_at.get_or_insert(self.t + u64::from(RECOVERY_NORMAL_RETRY_MS));
                    }
                }
            }
            self.t += u64::from(RECOVERY_OTA_INTERVAL_MS);
        }
    }

    /// いま (最後に) 動いている版
    pub fn running(&self) -> Option<u32> {
        self.log.iter().rev().find_map(|e| match e {
            Ev::Boot { version, .. } => Some(*version),
            _ => None,
        })
    }

    /// 最後に buy 済みとして起動している版 (TBYB の buy 待ちでない起動)
    pub fn proved_times(&self) -> Vec<u64> {
        self.log
            .iter()
            .filter_map(|e| match e {
                Ev::Proved { t, .. } => Some(*t),
                _ => None,
            })
            .collect()
    }

    pub fn boots_of(&self, version: u32) -> usize {
        self.log
            .iter()
            .filter(|e| matches!(e, Ev::Boot { version: v, .. } if *v == version))
            .count()
    }
}

// ============================================================
// 故障の組み合わせ
// ============================================================

const MIN: u64 = 60_000;
const HOUR: u64 = 60 * MIN;
const V1: u32 = boot_policy::version_word(0, 4, 2);
const V2: u32 = boot_policy::version_word(0, 4, 3);
const V3: u32 = boot_policy::version_word(0, 4, 4);

fn github_ok(_: u64) -> Github {
    Github::Ok
}

/// v1 (良い版) が動いているところへ v2 (故障入り) を配り、1 時間後に v3 (直した版) を配る
fn scenario(v2: Image, env_mod: impl FnOnce(&mut Env)) -> Board {
    let mut env = Env {
        network_down: (0, 0),
        github: github_ok,
        releases: vec![(0, Image::good(V1)), (10 * MIN, v2), (HOUR, Image::good(V3))],
        power_cycles: Vec::new(),
    };
    env_mod(&mut env);
    let mut b = Board::new(Image::good(V1));
    b.run(&env, 3 * HOUR);
    b
}

/// OTA 確認 (TLS + HTTP が通った) の間隔の最大 (起動から、最後の確認から終わりまで、も含める)
fn max_gap(b: &Board, from: u64, to: u64) -> u64 {
    let mut times: Vec<u64> = b.proved_times().into_iter().filter(|&t| t >= from && t <= to).collect();
    times.insert(0, from);
    times.push(to);
    times.windows(2).map(|w| w[1] - w[0]).max().unwrap()
}

fn stage_name(st: St) -> &'static str {
    match st {
        St::Display => "display",
        St::Sd => "sd",
        St::Cyw43 => "cyw43",
        St::Join => "join",
        St::Dhcp => "dhcp",
        St::OtaTls => "ota-tls",
        St::Ntp => "ntp",
        St::Weather => "weather",
        St::Message => "message",
        St::Slideshow => "slideshow",
    }
}

#[test]
fn good_update_is_bought_and_kept() {
    let b = scenario(Image::good(V2), |_| {});
    assert_eq!(b.bought, [V2, V3]);
    assert_eq!(b.running(), Some(V3));
    // buy 待ち (TBYB) は OTA 確認 + 一巡 + 25 s: 起動から 30〜60 s で buy する
    let boot = b
        .log
        .iter()
        .find_map(|e| match e {
            Ev::Boot { t, version: V2, pending: true, .. } => Some(*t),
            _ => None,
        })
        .unwrap();
    let bought = b
        .log
        .iter()
        .find_map(|e| match e {
            Ev::Bought { t, version: V2 } => Some(*t),
            _ => None,
        })
        .unwrap();
    let pending_ms = bought - boot;
    assert!((40_000..=60_000).contains(&pending_ms), "{pending_ms}");
}

/// 最初の一巡のどこかで落ちる / 止まる版は buy されず、旧版へ戻り、直した版が来れば入れ替わる
#[test]
fn broken_first_round_is_never_bought() {
    let stages = NORMAL.map(|(st, _)| st);
    let mut matrix = Vec::new();
    for st in stages {
        for fail in [Fail::Crash(st), Fail::Hang(st)] {
            let v2 = Image {
                version: V2,
                fail: Some(fail),
                recovery_fail: None,
            };
            let b = scenario(v2, |_| {});
            assert!(!b.bought.contains(&V2), "{fail:?}: bought the broken build");
            assert_eq!(b.running(), Some(V3), "{fail:?}: did not end on the fixed build");
            // 巻き戻った版の再試行は 10 分ごと (きつい繰り返しにならない): 10 分〜1 時間で 6 回以下
            let tries = b.boots_of(V2);
            assert!((1..=6).contains(&tries), "{fail:?}: {tries} trial boots of v2");
            // OTA 確認は 11 分以上空かない (旧版は 60 s ごと、再試行の起動の間も)
            let gap = max_gap(&b, 0, 3 * HOUR);
            assert!(gap <= 11 * MIN, "{fail:?}: gap {} s", gap / 1000);
            matrix.push(format!("{:?} {}: tries {tries}, max OTA gap {} s", fail, stage_name(st), gap / 1000));
        }
    }
    // 早く落ちる版ほど締め切り前に旧版へ戻る (締め切り 180 s + 8 s を超えない)
    println!("{}", matrix.join("\n"));
}

/// buy の後で落ちるようになった版: 2 回で回復モード、回復モードは 60 s ごとに OTA を確認し、直した版で直る
#[test]
fn post_buy_crash_loop_recovers_via_recovery_mode() {
    for after in [30_000, 5 * MIN, 20 * MIN] {
        let v2 = Image {
            version: V2,
            fail: Some(Fail::CrashAfter(after)),
            recovery_fail: None,
        };
        let b = scenario(v2, |_| {});
        assert!(b.bought.contains(&V2), "{after}: buy conditions passed, so it is bought");
        assert_eq!(b.running(), Some(V3), "{after}");
        let recovery_boots = b
            .log
            .iter()
            .filter(|e| matches!(e, Ev::Boot { version: V2, mode: Mode::Recovery, .. }))
            .count();
        if after < u64::from(HEALTHY_CLEAR_MS) {
            assert!(recovery_boots >= 1, "{after}: never entered recovery");
        }
        let v3_boot = b
            .log
            .iter()
            .find_map(|e| match e {
                Ev::Boot { t, version: V3, .. } => Some(*t),
                _ => None,
            })
            .unwrap();
        println!(
            "post-buy crash after {} s: {} recovery boots, {} boots of v2, v3 running at {} min, max OTA gap {} s",
            after / 1000,
            recovery_boots,
            b.boots_of(V2),
            v3_boot / MIN,
            max_gap(&b, 0, v3_boot) / 1000
        );
        assert!(v3_boot <= HOUR + 25 * MIN, "{after}: v3 took until {} min", v3_boot / MIN);
        assert!(max_gap(&b, 0, v3_boot) <= 25 * MIN, "{after}: gap {} s", max_gap(&b, 0, v3_boot) / 1000);
    }
}

/// 回復モードでも落ちる: 3 回で他方区画 (v1) へ戻り、v1 は v2 を入れ直さず、v3 を待って入れる
#[test]
fn recovery_crash_falls_back_and_blocks() {
    for rfail in [Fail::Crash(St::Cyw43), Fail::Hang(St::Display), Fail::Crash(St::OtaTls), Fail::CrashAfter(30_000)] {
        let v2 = Image {
            version: V2,
            fail: Some(Fail::CrashAfter(MIN)),
            recovery_fail: Some(rfail),
        };
        let b = scenario(v2, |_| {});
        assert!(b.log.iter().any(|e| matches!(e, Ev::Fallback { from: V2, .. })), "{rfail:?}: no fallback");
        let fb = b
            .log
            .iter()
            .find_map(|e| match e {
                Ev::Fallback { t, .. } => Some(*t),
                _ => None,
            })
            .unwrap();
        // 戻った後は v2 を二度と落とさない / 起動しない
        assert!(
            !b.log.iter().any(|e| matches!(e, Ev::Download { t, version: V2 } if *t > fb)),
            "{rfail:?}: re-downloaded the blocked build"
        );
        assert_eq!(b.running(), Some(V3), "{rfail:?}");
        assert_eq!(b.record.blocked, V2);
        println!(
            "recovery {rfail:?}: fallback at {} min, v2 boots {}, v3 bought {:?}",
            fb / MIN,
            b.boots_of(V2),
            b.bought
        );
    }
}

/// 起動時に Wi-Fi が無い: buy せず旧版へ (許容)。後で Wi-Fi が戻れば再試行で buy する
#[test]
fn network_down_during_pending_rolls_back_then_retries() {
    // v1 が v2 を落として FLASH_UPDATE 起動する頃 (11 分) から 20 分まで Wi-Fi が無い
    let b = scenario(Image::good(V2), |env| env.network_down = (11 * MIN, 20 * MIN));
    assert!(b.bought.contains(&V2));
    assert_eq!(b.running(), Some(V3));
    // v2 は 1 回目 (Wi-Fi なし) は buy されず戻った
    assert!(b.boots_of(V2) >= 2);
}

/// GitHub が 5xx: TLS + HTTP は通っているので buy 条件 (b) を満たす
#[test]
fn github_5xx_still_proves_tls() {
    fn gh(t: u64) -> Github {
        if (10 * MIN + 30_000..40 * MIN).contains(&t) { Github::Http5xx } else { Github::Ok }
    }
    let b = scenario(Image::good(V2), |env| env.github = gh);
    assert!(b.bought.contains(&V2));
    assert_eq!(b.running(), Some(V3));
}

/// GitHub に届かない (DNS / TLS): buy 待ちは締め切りまで 10 s ごとに試し、届かなければ旧版へ戻る
#[test]
fn github_down_during_pending_rolls_back() {
    fn gh(t: u64) -> Github {
        if (11 * MIN + 30_000..25 * MIN).contains(&t) { Github::Down } else { Github::Ok }
    }
    let b = scenario(Image::good(V2), |env| env.github = gh);
    assert!(b.bought.contains(&V2), "bought after GitHub came back");
    assert!(b.boots_of(V2) >= 2, "first trial rolled back");
    assert_eq!(b.running(), Some(V3));
}

/// 応答は来るが使えない (ヘッダの溢れ / 構文エラー / 切れた manifest): 経路の証拠にならないので buy しない
#[test]
fn github_bad_response_during_pending_is_not_proof() {
    fn gh(t: u64) -> Github {
        if (11 * MIN + 30_000..25 * MIN).contains(&t) { Github::BadResponse } else { Github::Ok }
    }
    let b = scenario(Image::good(V2), |env| env.github = gh);
    // 1 回目の試行 (11〜14 分) は buy されずに戻り、25 分以降の再試行で buy される
    let first_buy = b
        .log
        .iter()
        .find_map(|e| match e {
            Ev::Bought { t, version: V2 } => Some(*t),
            _ => None,
        })
        .unwrap();
    assert!(first_buy >= 25 * MIN, "bought at {} s on a bad response", first_buy / 1000);
    assert!(b.boots_of(V2) >= 2);
    assert_eq!(b.running(), Some(V3));
}

/// 他方区画が空のまま落ち続ける (最初の版しか無い): 他方区画へは戻れず、回復モードで OTA 確認を続け、配られた版で直る
#[test]
fn crash_loop_with_empty_other_slot_keeps_checking() {
    let mut env = Env {
        network_down: (0, 0),
        github: github_ok,
        releases: vec![(0, Image::good(V1)), (2 * HOUR, Image::good(V2))],
        power_cycles: Vec::new(),
    };
    env.releases[0].1.fail = Some(Fail::CrashAfter(MIN));
    env.releases[0].1.recovery_fail = Some(Fail::CrashAfter(2 * MIN));
    let v1 = env.releases[0].1;
    let mut b = Board::new(v1);
    b.run(&env, 4 * HOUR);
    assert!(b.log.iter().any(|e| matches!(e, Ev::Fallback { .. })));
    assert_eq!(b.running(), Some(V2));
    assert!(max_gap(&b, 0, 2 * HOUR) <= 5 * MIN, "gap {} s", max_gap(&b, 0, 2 * HOUR) / 1000);
    assert!(b.boots < 500);
}

/// 電源の入れ直しは回数を 0 に戻す (SCRATCH は消える)。途中で入れ直しても直した版にたどり着く
#[test]
fn power_cycle_midway() {
    let v2 = Image {
        version: V2,
        fail: Some(Fail::CrashAfter(2 * MIN)),
        recovery_fail: None,
    };
    let b = scenario(v2, |env| env.power_cycles = vec![30 * MIN, 50 * MIN]);
    assert_eq!(b.running(), Some(V3));
}

/// 両方の区画が壊れていて回復モードの OTA も通らない場合だけは、USB で書き直すしかない (既知の限界)
#[test]
fn both_slots_broken_is_the_documented_limit() {
    let mut env = Env {
        network_down: (0, 0),
        github: github_ok,
        releases: vec![(0, Image::good(V1)), (HOUR, Image::good(V2))],
        power_cycles: Vec::new(),
    };
    let broken = |v| Image {
        version: v,
        fail: Some(Fail::Crash(St::Display)),
        recovery_fail: Some(Fail::Crash(St::Display)),
    };
    env.releases[0].1 = broken(V1);
    let mut b = Board::new(broken(V1));
    b.max_boots = 100_000;
    b.run(&env, 5 * MIN);
    assert!(b.proved_times().is_empty());
    // 他方区画へ戻そうとするのは 1 回だけ (空なので同じ版がまた起動し、以後は回復モードのまま)
    assert_eq!(b.log.iter().filter(|e| matches!(e, Ev::Fallback { .. })).count(), 1);
}
