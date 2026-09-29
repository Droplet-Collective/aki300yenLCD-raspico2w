//! グローバルアロケータ (embedded-alloc)
//!
//! `reqwless` → `embedded-tls` の `rsa` feature が `alloc` を要求する (`rsa` / `num-bigint-dig`)。
//! 本クレートをリンクする全 bin に `alloc` が入るので、`#[global_allocator]` はライブラリ側に
//! 1 つ置く。ヒープ領域は使う bin (`wifi_ota`) が [`init`] で渡す。他の bin は init しないので
//! RAM を消費しない (その状態で確保が起きれば `alloc` のエラーハンドラ = panic になる)。
//!
//! `TlsVerify::None` (証明書検証なし) では RSA 検証コードは呼ばれず、実際にはヒープは使われない
//! 見込み。ヒープはあくまで rsa crate をリンクするための保険。

use core::mem::MaybeUninit;

use embedded_alloc::LlffHeap;

#[global_allocator]
static HEAP: LlffHeap = LlffHeap::empty();

/// ヒープ領域を登録する。1 回だけ呼ぶ。
///
/// # Safety
/// `memory` は他から参照されず、プログラムの寿命の間有効でなければならない (`static mut` を渡す)。
pub unsafe fn init(memory: &'static mut [MaybeUninit<u8>]) {
    // Safety: 呼び出し側の契約どおり領域は専有かつ 'static。
    unsafe { HEAP.init(memory.as_mut_ptr() as usize, memory.len()) };
}

/// 使用中のヒープ量 (診断用)
pub fn used() -> usize {
    HEAP.used()
}

/// 空きヒープ量 (診断用)
pub fn free() -> usize {
    HEAP.free()
}
