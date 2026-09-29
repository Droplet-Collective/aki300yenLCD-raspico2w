//! 書き込み先 A/B 区画の決定と、ストリーミング書き込み / 検証
//!
//! # アドレスの扱い (RP2350 データシート §5.4.8.10 / §5.4.8.11 / §2.2.2)
//!
//! - `embassy_rp::flash::Flash::blocking_erase / blocking_write` の `offset` は bootrom の
//!   `flash_range_erase(addr, …)` / `flash_range_program(addr, …)` にそのまま渡される
//!   (embassy-rp 0.9 `flash.rs` `ram_helpers::write_flash_inner`)。データシートはこれらの `addr` を
//!   「offset from start of flash」= **ストレージアドレス** と定義し、QMI アドレス変換 (ATRANS) は
//!   適用されない (「See flash_op() for a higher-level API which … can transparently apply a
//!   runtime-to-storage address translation」)。したがって他方区画の先頭オフセット
//!   (`Partition::start_offset()`) をそのまま渡せばよい。
//! - 一方 `Flash::blocking_read` は `0x1000_0000 + offset` (ATRANS が掛かる XIP 窓) を読むので、
//!   自区画以外は読めない (区画サイズ外はアドレスホールでバスフォールト)。読み戻しには
//!   **`0x1C00_0000` XIP_NOCACHE_NOALLOC_NOTRANSLATE** (§2.2.2 Table 10) にストレージオフセットを
//!   足したアドレスを CPU で直接読む。キャッシュもアドレス変換も通らないので、書いた直後でも
//!   フラッシュの実内容が見える。
//! - 書き込み中は割り込み禁止 (`critical_section`) かつ RAM 上のコードで bootrom を呼び、終了後に
//!   bootrom が BOOTRAM に保存した XIP セットアップ関数で元の XIP モードへ戻す (embassy-rp が行う)。
//!   ATRANS レジスタは触られないので、実行中のイメージはそのまま動き続ける。
//!
//! # 書き込み順序
//!
//! 1. `begin()`: 対象区画の先頭セクタ (IMAGE_DEF を含む) を消去して無効化する。以降どこで
//!    中断しても bootrom はこの区画を起動候補にしない。
//! 2. `push()`: 受信データを 4 kB ためて、セクタ 1 以降は消去 → 256 B ページ単位で書き込む。
//!    セクタ 0 の内容は RAM に取り置く。同時に SHA-256 を計算する。
//! 3. `finish()`: 端数を 0xFF で埋めて書き、受信サイズと SHA-256 を返す (呼び出し側が manifest と比較)。
//! 4. `commit_first_sector()`: 検証が通ったらセクタ 0 を書く。
//! 5. [`hash_storage`] で全域を読み戻し、manifest の SHA-256 と再比較する。

use embassy_rp::flash::{Blocking, ERASE_SIZE, Flash, PAGE_SIZE};
use embassy_rp::peripherals::FLASH;
use sha2::{Digest, Sha256};

use super::OtaError;
use crate::ab_boot::{self, FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S, Link, Partition, PartitionTable, XIP_BASE};

/// Pico 2 W のフラッシュ (W25Q32) 4 MB
pub const FLASH_SIZE: usize = 4 * 1024 * 1024;
/// 4 kB セクタ
pub const SECTOR_SIZE: usize = ERASE_SIZE;
/// XIP_NOCACHE_NOALLOC_NOTRANSLATE (§2.2.2)。ストレージオフセットをそのまま読める窓。
pub const XIP_NOTRANSLATE_BASE: u32 = 0x1C00_0000;

pub type OtaFlash = Flash<'static, FLASH, Blocking, FLASH_SIZE>;

/// 起動中の区画と、書き込み先の他方区画
#[derive(Clone, Copy, Debug)]
pub struct Slots {
    pub own: Partition,
    pub target: Partition,
}

/// 区画の A/B ラベル
pub fn slot_label(p: &Partition) -> &'static str {
    match p.link() {
        Link::BOf(_) => "B",
        _ => "A",
    }
}

/// BOOT_INFO ではなく `flash_runtime_to_storage_addr(0x10000000)` で自区画を求め、
/// リンク (§5.9.4.2 link type) から他方区画を決める。
pub fn find_slots() -> Result<Slots, OtaError> {
    let table = PartitionTable::read().map_err(|_| OtaError::NoPartitionTable)?;
    if !table.present {
        return Err(OtaError::NoPartitionTable);
    }
    let own_offset = ab_boot::runtime_to_storage_offset(XIP_BASE).map_err(|_| OtaError::NoTarget)?;
    let own = *table.find_by_offset(own_offset).ok_or(OtaError::NoTarget)?;
    let target = match own.link() {
        // 自分が B なら対応する A
        Link::BOf(a) => table.get(a).copied(),
        // 自分が A なら「A = 自分」とリンクされた B
        _ => table.partitions().find(|p| p.link() == Link::BOf(own.index)).copied(),
    }
    .ok_or(OtaError::NoTarget)?;
    if target.index == own.index || !target.has_flag(FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S) {
        return Err(OtaError::NoTarget);
    }
    Ok(Slots { own, target })
}

/// ストレージオフセット `offset` から `buf.len()` バイトを、ATRANS を通さない窓で読む。
pub fn read_storage(offset: u32, buf: &mut [u8]) {
    let src = (XIP_NOTRANSLATE_BASE + offset) as *const u8;
    // Safety: 0x1C00_0000 + 0..4 MB はフラッシュのミラーで、読み出しは常に安全。
    // 呼び出し側は FLASH_SIZE 内のオフセットしか渡さない。
    unsafe { core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), buf.len()) };
}

/// `[offset, offset + len)` の SHA-256 を読み戻しで計算する
pub fn hash_storage(offset: u32, len: u32) -> [u8; 32] {
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 1024];
    let mut pos = 0u32;
    while pos < len {
        let n = (len - pos).min(chunk.len() as u32) as usize;
        read_storage(offset + pos, &mut chunk[..n]);
        hasher.update(&chunk[..n]);
        pos += n as u32;
    }
    hasher.finalize().into()
}

/// `SlotWriter` が使う 4 kB × 2 の作業バッファ。bin 側で `static mut` (BSS、ゼロ初期化) に置き、
/// 大きな future がスタック経由でコピーされるのを避ける。DMA は触らない。
pub struct SectorBuffers {
    /// 受信中のセクタ
    pub buf: [u8; SECTOR_SIZE],
    /// 先頭セクタ (IMAGE_DEF) の取り置き
    pub first: [u8; SECTOR_SIZE],
}

impl SectorBuffers {
    pub const fn new() -> Self {
        Self {
            buf: [0; SECTOR_SIZE],
            first: [0; SECTOR_SIZE],
        }
    }
}

impl Default for SectorBuffers {
    fn default() -> Self {
        Self::new()
    }
}

/// 他方区画へのストリーミング書き込み
pub struct SlotWriter<'f> {
    flash: &'f mut OtaFlash,
    bufs: &'f mut SectorBuffers,
    /// 対象区画の先頭 (ストレージオフセット)
    base: u32,
    /// manifest が告げるイメージサイズ (これを超えて受信したらエラー)
    expected: u32,
    received: u32,
    /// 次に確定するセクタ番号 (0 = 先頭セクタ、RAM に取り置く)
    next_sector: u32,
    sectors_written: u32,
    buf_len: usize,
    first_len: usize,
    hasher: Sha256,
}

impl<'f> SlotWriter<'f> {
    /// `expected` は manifest の size。0 または区画サイズ超過は `BadSize`。
    pub fn new(
        flash: &'f mut OtaFlash,
        bufs: &'f mut SectorBuffers,
        target: &Partition,
        expected: u32,
    ) -> Result<Self, OtaError> {
        if expected == 0 || expected > target.size() {
            return Err(OtaError::BadSize);
        }
        Ok(Self {
            flash,
            bufs,
            base: target.start_offset(),
            expected,
            received: 0,
            next_sector: 0,
            sectors_written: 0,
            buf_len: 0,
            first_len: 0,
            hasher: Sha256::new(),
        })
    }

    pub fn received(&self) -> u32 {
        self.received
    }

    pub fn expected(&self) -> u32 {
        self.expected
    }

    pub fn sectors_written(&self) -> u32 {
        self.sectors_written
    }

    /// 対象区画の先頭セクタを消去して無効化する
    pub fn begin(&mut self) -> Result<(), OtaError> {
        self.erase_sector(0)
    }

    /// 受信データを追加する。4 kB たまるごとにセクタを書く。
    pub fn push(&mut self, mut data: &[u8]) -> Result<(), OtaError> {
        if self.received as usize + data.len() > self.expected as usize {
            return Err(OtaError::SizeMismatch);
        }
        self.hasher.update(data);
        self.received += data.len() as u32;
        while !data.is_empty() {
            let room = SECTOR_SIZE - self.buf_len;
            let n = room.min(data.len());
            self.bufs.buf[self.buf_len..self.buf_len + n].copy_from_slice(&data[..n]);
            self.buf_len += n;
            data = &data[n..];
            if self.buf_len == SECTOR_SIZE {
                self.flush_sector()?;
            }
        }
        Ok(())
    }

    /// 端数を書き、受信した全データの SHA-256 と受信サイズを返す。
    pub fn finish(&mut self) -> Result<([u8; 32], u32), OtaError> {
        if self.buf_len > 0 {
            self.flush_sector()?;
        }
        let digest: [u8; 32] = core::mem::replace(&mut self.hasher, Sha256::new()).finalize().into();
        Ok((digest, self.received))
    }

    /// 検証が済んだ後に先頭セクタ (IMAGE_DEF) を書き、区画を有効にする。
    pub fn commit_first_sector(&mut self) -> Result<(), OtaError> {
        if self.first_len == 0 {
            return Err(OtaError::SizeMismatch);
        }
        // begin() で消去済みだが、念のためもう一度消してから書く
        self.erase_sector(0)?;
        let len = self.first_len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        self.bufs.first[self.first_len..len].fill(0xff);
        self.flash
            .blocking_write(self.base, &self.bufs.first[..len])
            .map_err(|_| OtaError::Flash)?;
        self.sectors_written += 1;
        Ok(())
    }

    /// 失敗時: 先頭セクタを消して区画を無効のままにする
    pub fn invalidate(&mut self) -> Result<(), OtaError> {
        self.erase_sector(0)
    }

    fn erase_sector(&mut self, sector: u32) -> Result<(), OtaError> {
        let from = self.base + sector * SECTOR_SIZE as u32;
        self.flash
            .blocking_erase(from, from + SECTOR_SIZE as u32)
            .map_err(|_| OtaError::Flash)
    }

    /// `buf` の内容をセクタ `next_sector` へ (セクタ 0 は RAM に取り置く)
    fn flush_sector(&mut self) -> Result<(), OtaError> {
        let sector = self.next_sector;
        if sector == 0 {
            let (first, buf) = (&mut self.bufs.first, &self.bufs.buf);
            first[..self.buf_len].copy_from_slice(&buf[..self.buf_len]);
            self.first_len = self.buf_len;
        } else {
            self.erase_sector(sector)?;
            let len = self.buf_len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
            self.bufs.buf[self.buf_len..len].fill(0xff);
            let offset = self.base + sector * SECTOR_SIZE as u32;
            // buf は RAM (BSS) 上なので embassy は 1 回の in_ram 呼び出しで最大 16 ページをまとめて書く。
            self.flash
                .blocking_write(offset, &self.bufs.buf[..len])
                .map_err(|_| OtaError::Flash)?;
            self.sectors_written += 1;
        }
        self.next_sector += 1;
        self.buf_len = 0;
        Ok(())
    }
}
