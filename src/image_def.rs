//! RP2350 bootrom 用 IMAGE_DEF と picotool 用 binary_info
//!
//! embassy-rp は既定で版数なしの IMAGE_DEF を `.start_block` に置くが、
//! A/B パーティション + OTA では bootrom が「どちらのスロットが新しいか」を
//! IMAGE_DEF の VERSION 項目 (RP2350 データシート 5.9.2.1) で判断するため、
//! Cargo.toml の `version` から生成した VERSION 項目付き IMAGE_DEF を各 bin に
//! 埋め込む。embassy-rp 側は `imagedef-none` feature で無効化している。
//!
//! 各 bin は `use` 群の直後に 1 行書くだけでよい:
//!
//! ```ignore
//! pico2w_300yen_lcd::firmware_image_def!();
//! ```
//!
//! 版数の対応 (16 bit × 2 しか無いので patch を minor に畳み込む):
//!
//! | Cargo.toml version | IMAGE_DEF major.minor | picotool info の表示 |
//! |--------------------|-----------------------|----------------------|
//! | 0.1.0              | 0.100                 | `version: 0.100`     |
//! | 0.1.1              | 0.101                 | `version: 0.101`     |
//! | 0.2.0              | 0.200                 | `version: 0.200`     |
//! | 1.0.0              | 1.0                   | `version: 1.0`       |
//!
//! bootrom は (rollback, major, minor) を辞書順比較する (5.1.6) ので、
//! patch < 100 の範囲で Cargo の semver 順と一致する。

use embassy_rp::block::{
    Block, IMAGE_TYPE_EXE, IMAGE_TYPE_EXE_CHIP_RP2350, IMAGE_TYPE_EXE_CPU_ARM,
    IMAGE_TYPE_EXE_TYPE_SECURITY_S, IMAGE_TYPE_TBYB, ITEM_1BS_IMAGE_TYPE, ITEM_1BS_VERSION,
    item_generic_1bs,
};

/// picotool 用 binary_info (rp-binary-info クレート) の再エクスポート。
/// `firmware_image_def!` マクロから `$crate::image_def::binary_info::...` で参照する。
pub use embassy_rp::binary_info;

/// Cargo.toml の `version` そのまま (LCD 表示・ログ用)
pub const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Cargo の major / minor / patch
pub const VERSION_MAJOR: u16 = parse_u16(env!("CARGO_PKG_VERSION_MAJOR"));
pub const VERSION_MINOR: u16 = parse_u16(env!("CARGO_PKG_VERSION_MINOR"));
pub const VERSION_PATCH: u16 = parse_u16(env!("CARGO_PKG_VERSION_PATCH"));

/// IMAGE_DEF に載せる major
pub const IMAGE_DEF_MAJOR: u16 = VERSION_MAJOR;
/// IMAGE_DEF に載せる minor (= Cargo minor × 100 + patch)
pub const IMAGE_DEF_MINOR: u16 = VERSION_MINOR * 100 + VERSION_PATCH;

/// VERSION 項目の 2 ワード目 (上位 16 bit = major, 下位 16 bit = minor)
pub const IMAGE_DEF_VERSION_WORD: u32 = ((IMAGE_DEF_MAJOR as u32) << 16) | IMAGE_DEF_MINOR as u32;

/// `tbyb` feature 有効時は IMAGE_TYPE に TBYB フラグ (0x8000) を立てる
pub const TBYB: bool = cfg!(feature = "tbyb");

/// IMAGE_TYPE のフラグ値 (EXE, Secure, Arm, RP2350 [, TBYB])
pub const IMAGE_TYPE_FLAGS: u16 = {
    let base =
        IMAGE_TYPE_EXE | IMAGE_TYPE_EXE_TYPE_SECURITY_S | IMAGE_TYPE_EXE_CPU_ARM | IMAGE_TYPE_EXE_CHIP_RP2350;
    if TBYB { base | IMAGE_TYPE_TBYB } else { base }
};

/// 本クレートが生成する IMAGE_DEF の型。項目は
/// `[IMAGE_TYPE(1 word), VERSION ヘッダ, VERSION 値]` の 3 ワード。
pub type FirmwareImageDef = Block<3>;

/// VERSION 項目付き IMAGE_DEF を作る (const)。
///
/// データシート 5.9.2.1: word0 = `0x48 | size(2) << 8 | pad << 16 | num_otp_rows(0) << 24`、
/// word1 = major << 16 | minor。rollback 版数は非セキュアチップでは無視されるため付けない。
pub const fn firmware_image_def() -> FirmwareImageDef {
    Block::new([
        item_generic_1bs(IMAGE_TYPE_FLAGS, 1, ITEM_1BS_IMAGE_TYPE),
        item_generic_1bs(0, 2, ITEM_1BS_VERSION),
        IMAGE_DEF_VERSION_WORD,
    ])
}

/// 10 進文字列を u16 にする (const 版。Cargo が渡す版数専用)
const fn parse_u16(s: &str) -> u16 {
    let bytes = s.as_bytes();
    let mut value: u16 = 0;
    let mut i = 0;
    while i < bytes.len() {
        let digit = bytes[i];
        assert!(digit.is_ascii_digit(), "CARGO_PKG_VERSION_* must be decimal");
        value = value * 10 + (digit - b'0') as u16;
        i += 1;
    }
    value
}

/// bin の先頭 (`use` 群の直後など) で 1 回呼び、IMAGE_DEF と binary_info を埋め込む。
///
/// * `.start_block` — bootrom が最初の 4 kB を走査して見つける IMAGE_DEF
///   (memory.x の `.start_block` セクション。`KEEP` されている)
/// * `.bi_entries` — `picotool info` が表示する名前・版数・ビルド種別
///   (ヘッダ `PICOTOOL_HEADER` は rp-binary-info が `.boot_info` に置く)
///
/// bin クレート側に static を生やすので、リンカがライブラリの
/// オブジェクトを捨てて IMAGE_DEF が消える事故が起きない。
#[macro_export]
macro_rules! firmware_image_def {
    () => {
        #[unsafe(link_section = ".start_block")]
        #[used]
        pub static IMAGE_DEF: $crate::image_def::FirmwareImageDef =
            $crate::image_def::firmware_image_def();

        #[unsafe(link_section = ".bi_entries")]
        #[used]
        pub static PICOTOOL_ENTRIES: [$crate::image_def::binary_info::EntryAddr; 3] = [
            $crate::image_def::binary_info::rp_cargo_bin_name!(),
            $crate::image_def::binary_info::rp_cargo_version!(),
            $crate::image_def::binary_info::rp_program_build_attribute!(),
        ];
    };
}
