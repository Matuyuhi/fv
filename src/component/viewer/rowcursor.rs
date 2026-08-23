//! 行カーソル (ペインが「今どの行を対象にしているか」) の追従計算。
//!
//! GIT/LOG/PR の diff と VIEW・EDIT のどのペインにも「フォーカスしている 1 行」があり、
//! スクロールとの噛み合わせ方は全て同じ (上へはみ出したら scroll をカーソルまで下げ、
//! 下へはみ出したら「カーソルが最下段に来る scroll」まで上げる)。折返し中は視覚行数を
//! 数えないとこれが出せないので、その計算を `text::WrapCursor` と同じ規則で 1 箇所に置く
//! (折返し規則の定義を増やさない、という CLAUDE.md の方針の一環)。
//!
//! **カーソルの実体は各コンポーネントが持ち、ここには置かない**。ペインごとに「何行あるか」
//! (`line_count`) と「1 論理行が何視覚行を占めるか」(`rows_at`) の求め方が違う — diff ペインは
//! `Line` の span を連結し、VIEW は `TextDoc::plain` をそのまま引く — ため、状態ではなく
//! 計算だけを共有する形にしてある。
//!
//! 全て**純関数**にしてあるのは借用のため。呼び出し側は `&mut self.viewport` と
//! `self.lines()` (self 全体の不変借用) を同時には取れないので、「新しい値を計算して返す →
//! 代入する」の 2 段に分ける必要がある。

use super::Viewport;

/// カーソルが画面に収まるための scroll。`wrapped` は「視覚行 ≠ 論理行」かどうかで、
/// 非 wrap と side-by-side (描画側が事前に行数を揃える) はどちらも false になる。
/// `rows_at(i)` は論理行 i が占める視覚行数
pub(crate) fn scroll_for(
    vp: &Viewport,
    cursor: usize,
    line_count: usize,
    wrapped: bool,
    rows_at: impl Fn(usize) -> usize,
) -> usize {
    if vp.scroll > cursor {
        return cursor;
    }
    let min = min_scroll(vp, cursor, line_count, wrapped, rows_at);
    vp.scroll.max(min)
}

/// スクロール側を動かした (ホイール等) 後に、画面内へ引き戻したカーソル位置。
/// 置き去りにすると「対象が画面外に居るまま実行キーを押せる」状態になるため
pub(crate) fn clamp_cursor(
    vp: &Viewport,
    cursor: usize,
    line_count: usize,
    wrapped: bool,
    rows_at: impl Fn(usize) -> usize,
) -> usize {
    let last = line_count.saturating_sub(1);
    let mut cursor = cursor.min(last).max(vp.scroll.min(last));
    // 下端は視覚行数に依存するので、収まるまで 1 行ずつ引き上げる (高々 height 回)
    while cursor > vp.scroll && min_scroll(vp, cursor, line_count, wrapped, &rows_at) > vp.scroll {
        cursor -= 1;
    }
    cursor
}

/// ペイン内の row (枠線を除いた 0 起点) → 論理行
pub(crate) fn line_at_row(
    vp: &Viewport,
    row: usize,
    line_count: usize,
    wrapped: bool,
    rows_at: impl Fn(usize) -> usize,
) -> usize {
    let last = line_count.saturating_sub(1);
    if !wrapped {
        return (vp.scroll + row).min(last);
    }
    let mut line = vp.scroll.min(last);
    let mut remaining = row;
    while line < last {
        let rows = rows_at(line);
        if remaining < rows {
            break;
        }
        remaining -= rows;
        line += 1;
    }
    line
}

// カーソルを最下段に置いたときの scroll。非 wrap は視覚行 = 論理行なので単純な引き算、
// wrap 中はカーソルから上へ height 視覚行ぶん遡る (O(画面行数))
fn min_scroll(
    vp: &Viewport,
    cursor: usize,
    line_count: usize,
    wrapped: bool,
    rows_at: impl Fn(usize) -> usize,
) -> usize {
    let height = vp.height.max(1);
    if !wrapped {
        return cursor.saturating_sub(height - 1);
    }
    if line_count == 0 {
        return 0;
    }
    let mut top = cursor.min(line_count - 1);
    let mut used = 0usize;
    loop {
        used += rows_at(top);
        // カーソル行だけで画面を超える場合も、先頭はカーソル行に置くしかない
        if used > height && top < cursor {
            return top + 1;
        }
        if top == 0 {
            return 0;
        }
        top -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_cursor, line_at_row, scroll_for};
    use crate::component::viewer::Viewport;

    fn viewport(wrap: bool, scroll: usize) -> Viewport {
        let mut vp = Viewport::new(wrap);
        vp.scroll = scroll;
        vp.height = 10;
        vp.width = 40;
        vp
    }

    // 行 0 だけ 3 視覚行、以降は 1 視覚行
    fn rows(line: usize) -> usize {
        if line == 0 { 3 } else { 1 }
    }

    #[test]
    fn a_cursor_above_the_window_pulls_the_scroll_up_to_it() {
        let vp = viewport(false, 20);
        assert_eq!(scroll_for(&vp, 5, 100, false, rows), 5);
    }

    #[test]
    fn a_cursor_below_the_window_scrolls_just_far_enough() {
        let vp = viewport(false, 0);
        // height 10 → カーソル 12 が最下段に来る scroll は 3
        assert_eq!(scroll_for(&vp, 12, 100, false, rows), 3);
        // 画面内なら動かさない
        assert_eq!(scroll_for(&vp, 9, 100, false, rows), 0);
    }

    // 折返し中は「論理行いくつ分が画面に入るか」が行ごとに違うので、
    // scroll - cursor の引き算では出せない
    #[test]
    fn wrapping_counts_visual_rows_when_scrolling_to_the_cursor() {
        let vp = viewport(true, 0);
        // 行 0 が 3 視覚行を食うので、10 視覚行に収まるのは行 0..=7 まで。
        // カーソル 8 を出すには行 0 を追い出して scroll = 1 が要る
        assert_eq!(scroll_for(&vp, 8, 100, true, rows), 1);
        assert_eq!(scroll_for(&vp, 7, 100, true, rows), 0);
    }

    #[test]
    fn scrolling_drags_the_cursor_back_into_the_window() {
        let vp = viewport(false, 30);
        // 画面より上に取り残されたら上端へ
        assert_eq!(clamp_cursor(&vp, 5, 100, false, rows), 30);
        // 画面より下に取り残されたら下端へ
        assert_eq!(clamp_cursor(&vp, 90, 100, false, rows), 39);
        // 画面内ならそのまま
        assert_eq!(clamp_cursor(&vp, 33, 100, false, rows), 33);
    }

    #[test]
    fn clicking_a_row_walks_visual_rows_when_wrapped() {
        let vp = viewport(true, 0);
        // 行 0 が 3 視覚行を占めるので、row 0..2 は全て行 0
        assert_eq!(line_at_row(&vp, 0, 100, true, rows), 0);
        assert_eq!(line_at_row(&vp, 2, 100, true, rows), 0);
        assert_eq!(line_at_row(&vp, 3, 100, true, rows), 1);
        // 非 wrap は素直に scroll + row
        assert_eq!(line_at_row(&vp, 3, 100, false, rows), 3);
        // 最終行より下をクリックしても最終行に留める
        assert_eq!(line_at_row(&vp, 50, 4, true, rows), 3);
    }
}
