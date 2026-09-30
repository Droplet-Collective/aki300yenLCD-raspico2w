//! 状態を変える要求の検査 (0.5.0〜、ホストでテストする): アクセスコード、総当たりの抑止、送り元の確認
//!
//! 同じ LAN の中でも、ブラウザで開いた別のサイトが `http://192.168.x.y/` へ要求を送らせることはできる
//! (CSRF)。そこで状態を変える要求 (POST) には次を全部求める (docs/settings-server.md「安全のしくみ」):
//!
//! 1. `X-Ticker-Code` ヘッダに、起動ごとに作る 6 桁のアクセスコード (LCD にだけ出す)。独自ヘッダなので
//!    よそのサイトからは CORS の事前確認 (OPTIONS) が要り、サーバはそれに許可を返さない
//! 2. `Host` がこの端末の IP アドレス (DNS rebinding で別の名前から来た要求を断る。GET も同じ)
//! 3. `Origin` があれば `http://<Host>` と一致 (`Sec-Fetch-Site` があれば `same-origin` か `none`)
//! 4. コードを [`MAX_FAILURES`] 回続けて間違えたら [`LOCK_BASE_MS`] 受け付けない (次からは倍、上限 [`LOCK_MAX_MS`])

/// 続けて間違えてよい回数
pub const MAX_FAILURES: u8 = 5;
/// 最初の締め出し (ms)
pub const LOCK_BASE_MS: u32 = 30_000;
/// 締め出しの上限 (ms)
pub const LOCK_MAX_MS: u32 = 15 * 60_000;

/// アクセスコード (6 桁) を乱数から作る
pub fn code_from_random(random: u32) -> u32 {
    100_000 + random % 900_000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// コードが無い (401)
    Missing,
    /// 違う (401)。あと何回で締め出すか
    Wrong { left: u8 },
    /// 締め出し中 (429)。あと何秒
    Locked { secs: u32 },
}

/// アクセスコードと、間違いの数え方
#[derive(Clone, Copy, Debug)]
pub struct Guard {
    code: u32,
    failures: u8,
    /// 締め出しの終わり (起動からの ms、0 = 無し)
    locked_until: u32,
    /// 次の締め出しの長さ
    next_lock_ms: u32,
}

impl Guard {
    pub const fn new(code: u32) -> Self {
        Self {
            code,
            failures: 0,
            locked_until: 0,
            next_lock_ms: LOCK_BASE_MS,
        }
    }

    pub fn code(&self) -> u32 {
        self.code
    }

    /// 締め出し中なら残りの秒数
    pub fn locked(&self, now_ms: u32) -> Option<u32> {
        (self.locked_until != 0 && now_ms < self.locked_until).then(|| (self.locked_until - now_ms).div_ceil(1000))
    }

    /// 送られたコードを確かめる。締め出し中は正しいコードでも断る (総当たりを遅らせるため)
    pub fn check(&mut self, now_ms: u32, presented: Option<&str>) -> Result<(), AuthError> {
        if let Some(secs) = self.locked(now_ms) {
            return Err(AuthError::Locked { secs });
        }
        let Some(presented) = presented.map(str::trim).filter(|p| !p.is_empty()) else {
            return Err(AuthError::Missing);
        };
        if digits_equal(presented, self.code) {
            self.failures = 0;
            self.locked_until = 0;
            self.next_lock_ms = LOCK_BASE_MS;
            return Ok(());
        }
        self.failures = self.failures.saturating_add(1);
        if self.failures >= MAX_FAILURES {
            self.failures = 0;
            self.locked_until = now_ms.saturating_add(self.next_lock_ms).max(1);
            let secs = self.next_lock_ms / 1000;
            self.next_lock_ms = (self.next_lock_ms * 2).min(LOCK_MAX_MS);
            return Err(AuthError::Locked { secs });
        }
        Err(AuthError::Wrong {
            left: MAX_FAILURES - self.failures,
        })
    }
}

/// `presented` が `code` の 10 進 6 桁と同じか (桁ごとの比較を最後まで行う)
fn digits_equal(presented: &str, code: u32) -> bool {
    let p = presented.as_bytes();
    let mut expected = [0u8; 6];
    let mut c = code;
    for d in expected.iter_mut().rev() {
        *d = b'0' + (c % 10) as u8;
        c /= 10;
    }
    let mut diff = (p.len() != expected.len()) as u8;
    for (i, e) in expected.iter().enumerate() {
        diff |= p.get(i).copied().unwrap_or(0) ^ e;
    }
    diff == 0
}

/// `Host` がこの端末の IP アドレス (`a.b.c.d` または `a.b.c.d:80`) か
pub fn host_is_board(host: Option<&str>, ip: [u8; 4]) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.strip_suffix(":80").unwrap_or(host);
    let mut octets = host.split('.');
    for want in ip {
        match octets.next().and_then(|o| (!o.is_empty() && o.len() <= 3).then(|| o.parse::<u8>().ok()).flatten()) {
            Some(got) if got == want => {}
            _ => return false,
        }
    }
    octets.next().is_none()
}

/// 状態を変える要求の送り元の検査: `Origin` があれば `http://<Host>`、`Sec-Fetch-Site` があれば同じサイトから
pub fn same_origin(host: Option<&str>, origin: Option<&str>, fetch_site: Option<&str>) -> bool {
    if let Some(site) = fetch_site
        && !(site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none"))
    {
        return false;
    }
    match (origin, host) {
        (None, _) => true,
        (Some(origin), Some(host)) => origin.strip_prefix("http://") == Some(host.strip_suffix(":80").unwrap_or(host)),
        (Some(_), None) => false,
    }
}
