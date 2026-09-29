//! RP2350 bootrom の A/B パーティション・TBYB 関連 API の薄いラッパ
//!
//! 参照: RP2350 データシート
//! - 5.1.7 A/B versions, 5.1.16 Flash update boot, 5.1.17 Try before you buy
//! - 5.4.8.4 `explicit_buy`, 5.4.8.13 `flash_runtime_to_storage_addr`,
//!   5.4.8.15 `get_b_partition`, 5.4.8.16 `get_partition_table_info`,
//!   5.4.8.17 `get_sys_info` (BOOT_INFO), 5.4.8.24 `reboot`
//! - 5.9.4.2 Partition location, permissions, and flags
//!
//! 定数値は pico-sdk `boot/bootrom_constants.h` と一致させている。
//!
//! 注意: `pick_ab_partition` は TBYB の buy 待ち中に呼ぶと、explicit_buy が
//! 使う「他方スロット消去アドレス」を消してしまう (pico-sdk `rom_pick_ab_update_partition`
//! のコメント参照)。このモジュールでは代わりに BOOT_INFO と
//! `flash_runtime_to_storage_addr` から起動スロットを求める。

use core::ptr::addr_of_mut;

use embassy_rp::rom_data;

// ---- get_sys_info flags -------------------------------------------------
/// `get_sys_info` の BOOT_INFO フラグ (4 ワード)
pub const SYS_INFO_BOOT_INFO: u32 = 0x0040;

// ---- boot type (BOOT_INFO の bb バイト) ---------------------------------
pub const BOOT_TYPE_NORMAL: u8 = 0;
pub const BOOT_TYPE_BOOTSEL: u8 = 2;
pub const BOOT_TYPE_RAM_IMAGE: u8 = 3;
pub const BOOT_TYPE_FLASH_UPDATE: u8 = 4;
pub const BOOT_TYPE_PC_SP: u8 = 0xd;
/// chain_image() 経由で起動した場合に立つ
pub const BOOT_TYPE_CHAINED_FLAG: u8 = 0x80;

// ---- TBYB / update info (BOOT_INFO の tt バイト) -------------------------
/// TBYB イメージとして起動し、explicit_buy 待ち
pub const TBYB_FLAG_BUY_PENDING: u8 = 0x1;
/// OTP の rollback 版数が更新された (セキュアチップのみ)
pub const TBYB_FLAG_OTP_VERSION_APPLIED: u8 = 0x2;
/// 他方パーティションの先頭セクタが消去された (版数ダウングレード時)
pub const TBYB_FLAG_OTHER_ERASED: u8 = 0x4;

// ---- boot diagnostic (BOOT_INFO の 2 ワード目、pico-sdk bootrom_constants.h) ----
// 下位 16 bit が A (slot 0 / 診断対象パーティション)、上位 16 bit が B (slot 1 / その B 区画)。
pub const BOOT_DIAGNOSTIC_WINDOW_SEARCHED: u16 = 0x0001;
pub const BOOT_DIAGNOSTIC_INVALID_BLOCK_LOOP: u16 = 0x0002;
pub const BOOT_DIAGNOSTIC_VALID_BLOCK_LOOP: u16 = 0x0004;
pub const BOOT_DIAGNOSTIC_VALID_IMAGE_DEF: u16 = 0x0008;
pub const BOOT_DIAGNOSTIC_HAS_PARTITION_TABLE: u16 = 0x0010;
pub const BOOT_DIAGNOSTIC_CONSIDERED: u16 = 0x0020;
pub const BOOT_DIAGNOSTIC_CHOSEN: u16 = 0x0040;
pub const BOOT_DIAGNOSTIC_IMAGE_DEF_VERIFIED_OK: u16 = 0x1000;
pub const BOOT_DIAGNOSTIC_LOAD_MAP_ENTRIES_LOADED: u16 = 0x2000;
pub const BOOT_DIAGNOSTIC_IMAGE_LAUNCHED: u16 = 0x4000;
pub const BOOT_DIAGNOSTIC_IMAGE_CONDITION_FAILURE: u16 = 0x8000;

/// 診断ワードの片側 (16 bit) を一語 (LCD 用に 9 字以内) に要約する。bootrom がその区画をどこまで進めたかの目安。
pub fn diagnostic_summary(half: u16) -> &'static str {
    if half & BOOT_DIAGNOSTIC_IMAGE_LAUNCHED != 0 {
        "launched"
    } else if half & BOOT_DIAGNOSTIC_IMAGE_CONDITION_FAILURE != 0 {
        // 検証は通ったが起動条件で落ちた (TBYB だが FLASH_UPDATE の対象でない、等)
        "cond-fail"
    } else if half & BOOT_DIAGNOSTIC_CHOSEN != 0 {
        "chosen"
    } else if half & BOOT_DIAGNOSTIC_CONSIDERED != 0 {
        "consid"
    } else if half & BOOT_DIAGNOSTIC_VALID_IMAGE_DEF != 0 {
        "imgdef"
    } else if half & BOOT_DIAGNOSTIC_INVALID_BLOCK_LOOP != 0 {
        "badloop"
    } else if half & BOOT_DIAGNOSTIC_WINDOW_SEARCHED != 0 {
        "searched"
    } else {
        "-"
    }
}

// ---- get_partition_table_info flags -------------------------------------
pub const PT_INFO_PT_INFO: u32 = 0x0001;
pub const PT_INFO_PARTITION_LOCATION_AND_FLAGS: u32 = 0x0010;
pub const PT_INFO_PARTITION_NAME: u32 = 0x0080;
pub const PT_INFO_SINGLE_PARTITION: u32 = 0x8000;

// ---- 5.9.4.2 permissions_and_location / permissions_and_flags ----------
pub const LOCATION_FIRST_SECTOR_BITS: u32 = 0x0000_1fff;
pub const LOCATION_LAST_SECTOR_BITS: u32 = 0x03ff_e000;
pub const PERMISSION_S_R: u32 = 0x0400_0000;
pub const PERMISSION_S_W: u32 = 0x0800_0000;
pub const PERMISSION_NS_R: u32 = 0x1000_0000;
pub const PERMISSION_NS_W: u32 = 0x2000_0000;
pub const PERMISSION_NSBOOT_R: u32 = 0x4000_0000;
pub const PERMISSION_NSBOOT_W: u32 = 0x8000_0000;
pub const FLAGS_HAS_ID: u32 = 0x0000_0001;
pub const FLAGS_LINK_TYPE_BITS: u32 = 0x0000_0006;
pub const FLAGS_LINK_VALUE_BITS: u32 = 0x0000_0078;
pub const FLAGS_NOT_BOOTABLE_ARM: u32 = 0x0000_0200;
pub const FLAGS_NOT_BOOTABLE_RISCV: u32 = 0x0000_0400;
pub const FLAGS_HAS_NAME: u32 = 0x0000_1000;
pub const FLAGS_ACCEPTS_FAMILY_DATA: u32 = 0x0001_0000;
pub const FLAGS_ACCEPTS_FAMILY_RP2350_ARM_S: u32 = 0x0002_0000;

// ---- reboot flags (5.4.8.24) --------------------------------------------
pub const REBOOT_TYPE_NORMAL: u32 = 0x0000;
pub const REBOOT_TYPE_BOOTSEL: u32 = 0x0002;
pub const REBOOT_TYPE_FLASH_UPDATE: u32 = 0x0004;
pub const REBOOT_NO_RETURN_ON_SUCCESS: u32 = 0x0100;

/// フラッシュセクタ (bootrom のパーティション単位)
pub const SECTOR_SIZE: u32 = 4096;
/// XIP ランタイム窓の先頭 (リンク時のアドレス)
pub const XIP_BASE: u32 = 0x1000_0000;

/// bootrom の負のエラーコード (pico-sdk `bootrom_constants.h`)
pub fn error_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        -4 => "NOT_PERMITTED",
        -5 => "INVALID_ARG",
        -9 => "INSUFFICIENT_RESOURCES",
        -10 => "INVALID_ADDRESS",
        -11 => "BAD_ALIGNMENT",
        -12 => "INVALID_STATE",
        -13 => "BUFFER_TOO_SMALL",
        -14 => "PRECONDITION_NOT_MET",
        -15 => "MODIFIED_DATA",
        -16 => "INVALID_DATA",
        -17 => "NOT_FOUND",
        -18 => "UNSUPPORTED_MODIFICATION",
        -19 => "LOCK_REQUIRED",
        _ => "UNKNOWN",
    }
}

/// `get_sys_info(BOOT_INFO)` の内容 (pico-sdk `boot_info_t` と同じ並び)
#[derive(Clone, Copy, Debug, defmt::Format)]
pub struct BootInfo {
    /// 診断情報の対象 "パーティション" (0..15、-1 none、-2 slot0、-3 slot1、-4 image)
    pub diagnostic_partition: i8,
    /// 直近の起動種別 (`BOOT_TYPE_*`)。chain_image 経由なら 0x80 が立つ
    pub boot_type: u8,
    /// 起動したパーティション番号。パーティションテーブル無し等では -1
    pub partition: i8,
    /// `TBYB_FLAG_*`
    pub tbyb_and_update_info: u8,
    /// 起動診断ワード (下位 16 bit = slot0 / A、上位 16 bit = slot1 / B)
    pub boot_diagnostic: u32,
    /// reboot() の p0 / p1 (FLASH_UPDATE なら p0 = 更新した領域の先頭)
    pub reboot_params: [u32; 2],
}

impl BootInfo {
    /// bootrom から読む。パーティションテーブル無しでも成功する。
    pub fn read() -> Option<Self> {
        let mut buf = [0u32; 5];
        // Safety: buf は 5 ワードで、BOOT_INFO は 1 + 4 ワードを返す。
        let n = unsafe { rom_data::get_sys_info(buf.as_mut_ptr(), buf.len(), SYS_INFO_BOOT_INFO) };
        if n != 5 || buf[0] != SYS_INFO_BOOT_INFO {
            return None;
        }
        let word = buf[1];
        Some(Self {
            diagnostic_partition: (word & 0xff) as u8 as i8,
            boot_type: ((word >> 8) & 0xff) as u8,
            partition: ((word >> 16) & 0xff) as u8 as i8,
            tbyb_and_update_info: ((word >> 24) & 0xff) as u8,
            boot_diagnostic: buf[2],
            reboot_params: [buf[3], buf[4]],
        })
    }

    /// TBYB イメージとして起動し、まだ explicit_buy していない
    pub fn buy_pending(&self) -> bool {
        self.tbyb_and_update_info & TBYB_FLAG_BUY_PENDING != 0
    }

    /// 診断ワードの A 側 (下位 16 bit) と B 側 (上位 16 bit)
    pub fn diagnostic_halves(&self) -> (u16, u16) {
        ((self.boot_diagnostic & 0xffff) as u16, (self.boot_diagnostic >> 16) as u16)
    }

    /// 起動種別の名前 (chain フラグは除く)
    pub fn boot_type_name(&self) -> &'static str {
        match self.boot_type & !BOOT_TYPE_CHAINED_FLAG {
            BOOT_TYPE_NORMAL => "NORMAL",
            BOOT_TYPE_BOOTSEL => "BOOTSEL",
            BOOT_TYPE_RAM_IMAGE => "RAM_IMAGE",
            BOOT_TYPE_FLASH_UPDATE => "FLASH_UPDATE",
            BOOT_TYPE_PC_SP => "PC_SP",
            _ => "?",
        }
    }
}

/// パーティションの A/B / owner リンク
#[derive(Clone, Copy, Debug, PartialEq, Eq, defmt::Format)]
pub enum Link {
    None,
    /// この区画は B。値は対応する A の番号
    BOf(u8),
    /// この区画は A で、値は owner の番号
    OwnedBy(u8),
}

/// パーティション 1 件 (`PT_INFO_PARTITION_LOCATION_AND_FLAGS` + `PT_INFO_PARTITION_NAME`)
#[derive(Clone, Copy, Debug, defmt::Format)]
pub struct Partition {
    pub index: u8,
    pub permissions_and_location: u32,
    pub permissions_and_flags: u32,
    name: [u8; 32],
    name_len: u8,
}

impl Partition {
    pub fn first_sector(&self) -> u32 {
        self.permissions_and_location & LOCATION_FIRST_SECTOR_BITS
    }

    pub fn last_sector(&self) -> u32 {
        (self.permissions_and_location & LOCATION_LAST_SECTOR_BITS) >> 13
    }

    /// フラッシュ内オフセット (ストレージアドレス、0 起点)
    pub fn start_offset(&self) -> u32 {
        self.first_sector() * SECTOR_SIZE
    }

    /// 末尾の次のオフセット
    pub fn end_offset(&self) -> u32 {
        (self.last_sector() + 1) * SECTOR_SIZE
    }

    pub fn size(&self) -> u32 {
        self.end_offset() - self.start_offset()
    }

    pub fn link(&self) -> Link {
        let value = ((self.permissions_and_flags & FLAGS_LINK_VALUE_BITS) >> 3) as u8;
        match (self.permissions_and_flags & FLAGS_LINK_TYPE_BITS) >> 1 {
            1 => Link::BOf(value),
            2 => Link::OwnedBy(value),
            _ => Link::None,
        }
    }

    pub fn has_flag(&self, mask: u32) -> bool {
        self.permissions_and_flags & mask != 0
    }

    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }

    /// オフセットがこのパーティション内か
    pub fn contains_offset(&self, offset: u32) -> bool {
        offset >= self.start_offset() && offset < self.end_offset()
    }
}

/// パーティションテーブル全体
#[derive(Clone, Copy, Debug)]
pub struct PartitionTable {
    pub present: bool,
    pub unpartitioned_permissions_and_location: u32,
    pub unpartitioned_permissions_and_flags: u32,
    count: u8,
    partitions: [Option<Partition>; 16],
}

impl PartitionTable {
    /// bootrom の常駐パーティションテーブルを読む。
    ///
    /// エラー: `PRECONDITION_NOT_MET(-14)` は未ロード (watchdog/RAM 起動時など)、
    /// `INVALID_STATE(-12)` はロード後にフラッシュ上のテーブルが書き換わった場合。
    pub fn read() -> Result<Self, i32> {
        // 1 (flags) + 3 (PT_INFO) + 16 × (2 + 最大 9 ワードの名前) = 180 ワード
        let mut buf = [0u32; 192];
        let flags = PT_INFO_PT_INFO | PT_INFO_PARTITION_LOCATION_AND_FLAGS | PT_INFO_PARTITION_NAME;
        // Safety: buf の長さを渡しているので bootrom は範囲外に書かない。
        let n = unsafe { rom_data::get_partition_table_info(buf.as_mut_ptr(), buf.len(), flags) };
        if n < 0 {
            return Err(n);
        }
        let words = &buf[..n as usize];
        let supported = words.first().copied().unwrap_or(0);
        if supported & PT_INFO_PT_INFO == 0 || words.len() < 4 {
            return Err(-16);
        }
        let mut table = Self {
            present: words[1] & 0x100 != 0,
            unpartitioned_permissions_and_location: words[2],
            unpartitioned_permissions_and_flags: words[3],
            count: (words[1] & 0xff) as u8,
            partitions: [None; 16],
        };
        let mut pos = 4;
        for index in 0..table.count.min(16) {
            if supported & PT_INFO_PARTITION_LOCATION_AND_FLAGS == 0 || pos + 2 > words.len() {
                break;
            }
            let mut partition = Partition {
                index,
                permissions_and_location: words[pos],
                permissions_and_flags: words[pos + 1],
                name: [0; 32],
                name_len: 0,
            };
            pos += 2;
            if supported & PT_INFO_PARTITION_NAME != 0 && partition.has_flag(FLAGS_HAS_NAME) {
                if pos >= words.len() {
                    break;
                }
                // Byte 0: 7 bit の長さ、Byte 1..: 文字、ワード境界までパディング
                let len = (words[pos] & 0x7f) as usize;
                let total_words = (1 + len).div_ceil(4);
                let mut written = 0;
                for i in 0..len.min(32) {
                    let byte_index = 1 + i;
                    let word = words.get(pos + byte_index / 4).copied().unwrap_or(0);
                    partition.name[i] = ((word >> ((byte_index % 4) * 8)) & 0xff) as u8;
                    written += 1;
                }
                partition.name_len = written as u8;
                pos += total_words;
            }
            table.partitions[index as usize] = Some(partition);
        }
        Ok(table)
    }

    pub fn count(&self) -> usize {
        self.count as usize
    }

    pub fn partitions(&self) -> impl Iterator<Item = &Partition> {
        self.partitions.iter().flatten()
    }

    pub fn get(&self, index: u8) -> Option<&Partition> {
        self.partitions.get(index as usize).and_then(|p| p.as_ref())
    }

    /// ストレージオフセットを含むパーティション
    pub fn find_by_offset(&self, offset: u32) -> Option<&Partition> {
        self.partitions().find(|p| p.contains_offset(offset))
    }
}

/// XIP ランタイムアドレス (0x1000_0000 起点) を QMI アドレス変換後の
/// ストレージアドレスへ変換する。戻り値はフラッシュ先頭からのオフセット。
///
/// パーティションから起動していれば `runtime_to_storage_offset(XIP_BASE)` が
/// そのパーティションの先頭オフセットになる。
pub fn runtime_to_storage_offset(runtime_addr: u32) -> Result<u32, i32> {
    // Safety: 引数のみで副作用なし。
    let r = unsafe { rom_data::flash_runtime_to_storage_addr(runtime_addr) };
    if r < 0 {
        Err(r)
    } else {
        // bootrom は 0x1000_0000 を含む形で返す実装と含まない実装の両方が
        // ありうるので、どちらでもオフセットに正規化する。
        Ok((r as u32).wrapping_sub(XIP_BASE) & 0x0fff_ffff)
    }
}

/// パーティション A に対応する B の番号
pub fn b_partition_of(partition_a: u8) -> Result<u8, i32> {
    // Safety: 引数のみ。
    let r = unsafe { rom_data::get_b_partition(partition_a as u32) };
    if r < 0 { Err(r) } else { Ok(r as u8) }
}

/// explicit_buy 用の 4 kB ワークエリア (ワード境界)
static mut BUY_WORKAREA: [u32; 1024] = [0; 1024];

/// TBYB で起動した現イメージを「購入」する (5.1.17 / 5.4.8.4)。
///
/// bootrom は IMAGE_DEF を含むセクタを消去・再書き込みして TBYB フラグを
/// 落とし、版数ダウングレードなら他方パーティションの先頭セクタも消す。
/// フラッシュ操作中は XIP が止まるので、割り込みを禁止して呼ぶ
/// (割り込みハンドラはフラッシュ上にある)。LCD の DMA は RAM しか読まない
/// ので継続して良い。core1 は本プロジェクトでは使っていない。
///
/// 戻り値: `Ok(())` または bootrom のエラーコード。
pub fn explicit_buy() -> Result<(), i32> {
    let r = critical_section::with(|_| {
        // Safety: BUY_WORKAREA は 4 kB・ワード境界で、他から参照されない。
        unsafe { rom_data::explicit_buy(addr_of_mut!(BUY_WORKAREA) as *mut u8, 4096) }
    });
    // 自イメージの先頭セクタが書き換わったので XIP キャッシュを捨てる。
    // Safety: ROM 関数。副作用はキャッシュのフラッシュのみ。
    unsafe { rom_data::flash_flush_cache() };
    if r == 0 { Ok(()) } else { Err(r) }
}

/// 更新した領域 (ストレージオフセット) を優先して起動する FLASH_UPDATE 再起動。
/// OTA で他方スロットへ書き終えた後に呼ぶ。`delay_ms` 後にリセットし、戻らない。
///
/// p0 には bootrom がパーティション先頭と比較する「更新した領域の先頭」を渡す。
/// picotool と同じく XIP_BASE を足したフラッシュアドレスで渡す (実機未確認)。
pub fn reboot_flash_update(updated_region_offset: u32, delay_ms: u32) -> ! {
    let _ = rom_data::reboot(
        REBOOT_TYPE_FLASH_UPDATE | REBOOT_NO_RETURN_ON_SUCCESS,
        delay_ms,
        XIP_BASE + updated_region_offset,
        0,
    );
    loop {
        cortex_m::asm::wfe();
    }
}

/// 通常再起動 (診断パーティション p0 を指定)
pub fn reboot_normal(diagnostic_partition: u32, delay_ms: u32) -> ! {
    let _ = rom_data::reboot(
        REBOOT_TYPE_NORMAL | REBOOT_NO_RETURN_ON_SUCCESS,
        delay_ms,
        diagnostic_partition,
        0,
    );
    loop {
        cortex_m::asm::wfe();
    }
}
