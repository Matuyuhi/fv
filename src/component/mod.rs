//! 画面上の 1 つの部品 = 1 フォルダ。状態 (mod.rs 以下) と、その状態だけを受け取って
//! 描く View (view.rs) を同じ場所に置く。View が App 全体を受け取らないのは
//! CLAUDE.md「描画の依存範囲」の通りで、ここに置ける条件そのものでもある
//! (App を参照する画面は component ではなく shell/ に置く)。

pub mod branch;
pub mod editor;
pub mod finder;
pub mod gitlane;
pub mod issues;
pub mod log;
pub mod prs;
pub mod remotelist;
pub mod tree;
pub mod viewer;
