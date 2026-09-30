//! future の poll をインライン展開させない包み (0.4.2〜、スタック対策)
//!
//! async の poll は呼び出し先の poll までまとめてインライン展開されやすく、1 つのタスクの中で大きな future
//! (OTA 確認 = TLS、CYW43 の起動、接続管理) を何か所も待つと、それぞれの局所変数が 1 つの巨大なスタック
//! フレームに並ぶ (LLVM が別々の分岐の領域を重ねない)。0.4.2 の取得タスク (通常の取得 + 回復モード) では
//! 30 kB になった。[`noinline`] で包んだ future は別の関数で poll されるので、フレームは深さ方向に積まれる
//! (同時に使うのは 1 つだけなので、最深経路は最大のものだけになる)。

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

pub struct NoInline<F>(F);

pub fn noinline<F: Future>(f: F) -> NoInline<F> {
    NoInline(f)
}

impl<F: Future> Future for NoInline<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        // Safety: 中身を動かさない (Pin の射影だけ)
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}
