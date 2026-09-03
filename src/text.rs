//! 閲覧 (viewer) と編集 (editor) が共有する桁計算の唯一の定義。
//! タブ幅・gutter 幅の解釈が場所によってズレると、検索ハイライト・カーソル・
//! クリック座標の桁対応 (CLAUDE.md の整合インバリアント) が全て壊れるため一箇所に集める。

use ratatui::text::Span;

/// タブ 1 文字の展開結果。normalize と display_col/char_col_at の換算は必ずこれ経由で揃える
pub const TAB_EXPANDED: &str = "    ";

/// 改行を落とし、端末で幅が不定になるタブをスペースに展開する
pub fn normalize(segment: &str) -> String {
    let trimmed = segment.trim_end_matches(['\n', '\r']);
    if trimmed.contains('\t') {
        trimmed.replace('\t', TAB_EXPANDED)
    } else {
        trimmed.to_string()
    }
}

/// 行番号 gutter の全体 char 幅 (行番号の桁数 + 末尾の区切り空白 1 文字)
pub fn gutter_width(line_count: usize) -> usize {
    line_count.max(1).to_string().len() + 1
}

/// バッファ char 座標 → 表示桁 (タブ = TAB_EXPANDED 幅)
pub fn display_col(line: &str, char_col: usize) -> usize {
    line.chars()
        .take(char_col)
        .map(|c| if c == '\t' { TAB_EXPANDED.len() } else { 1 })
        .sum()
}

/// 表示桁 → バッファ char 座標。タブの展開幅の途中はそのタブ自身に丸める
pub fn char_col_at(line: &str, display: usize) -> usize {
    let mut acc = 0;
    for (i, c) in line.chars().enumerate() {
        if acc >= display {
            return i;
        }
        acc += if c == '\t' { TAB_EXPANDED.len() } else { 1 };
        if acc > display {
            return i;
        }
    }
    line.chars().count()
}

/// 表示単位 (grapheme) が端末で占めるセル数。ratatui の描画 (LineTruncator) は
/// grapheme を単位に display width で桁を送り、幅を超えた時点でその行を打ち切るので、
/// 折返し位置も同じ尺度・同じ単位で決めないと「折返しの継ぎ目で文字が消える」。
/// unicode-width / unicode-segmentation を直接足さず ratatui の Span を通すのは、
/// 描画側とまったく同じ計算であることを保証するため (新規依存も増やさない)
pub fn cells(symbol: &str) -> usize {
    Span::raw(symbol).width()
}

/// 折返し位置の唯一の定義。normalize 済みの grapheme (タブは展開済み) を 1 つずつ
/// 食わせ、「それを置く前に折り返すか」を返す。描画 (text_pane::wrap_line)・視覚行数
/// (wrap_rows)・カーソル追従 (wrap_position)・クリック座標 (wrap_col_at) の 4 者が
/// この 1 つの規則を共有する。
/// **char ではなく grapheme を単位にする**のは、ZWJ 絵文字 (👩\u{200d}💻) のように
/// 「char ごとの幅の合計 (4) と実際の描画幅 (2) が食い違う」列があるため。char で
/// 数えると幅を過大に見積もって列が途中で切れ、絵文字が 2 つの視覚行に割れる
pub(crate) struct WrapCursor {
    width: usize,
    used: usize,
}

impl WrapCursor {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width: width.max(1),
            used: 0,
        }
    }

    /// symbol を今の視覚行に置けなければ true (= symbol は次の視覚行の先頭になる)。
    /// 折返し幅より広い grapheme は単独でも収まらないので、空の視覚行では必ず受け入れる
    /// (受け入れないと視覚行だけが無限に増える)
    pub(crate) fn push(&mut self, symbol: &str) -> bool {
        let cells = cells(symbol);
        if self.used > 0 && self.used + cells > self.width {
            self.used = cells;
            return true;
        }
        self.used += cells;
        false
    }
}

// normalize 後の grapheme 列を辿り、(その grapheme の先頭 char 座標, grapheme) を渡す。
// タブは空白 TAB_EXPANDED 個へ展開して渡すので、normalize() の String 確保なしに
// 生の行をそのまま食わせられる (折返しの計算はキー入力ごとに通る経路)。
// f が false を返したら打ち切る
fn walk(line: &str, mut f: impl FnMut(usize, &str) -> bool) {
    let mut col = 0usize;
    for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(line, true) {
        if grapheme == "\t" {
            for _ in 0..TAB_EXPANDED.chars().count() {
                if !f(col, " ") {
                    return;
                }
                col += 1;
            }
            continue;
        }
        if !f(col, grapheme) {
            return;
        }
        col += grapheme.chars().count();
    }
}

/// 論理行が占める視覚行数 (wrap 時)。空行も 1 行を占める。
/// 全角文字は折返し境界を跨げないため単純な割り算では出せない
pub fn wrap_rows(line: &str, width: usize) -> usize {
    let mut wrap = WrapCursor::new(width);
    let mut rows = 1;
    walk(line, |_, symbol| {
        if wrap.push(symbol) {
            rows += 1;
        }
        true
    });
    rows
}

/// normalize 後の char 座標が何番目の視覚行のどこに来るか → (視覚行, 行内の char 座標)。
/// 行末より後ろ (カーソルが行末にある場合) は、直前の視覚行が埋まっていれば次の行の先頭を返す
pub fn wrap_position(line: &str, char_col: usize, width: usize) -> (usize, usize) {
    let mut wrap = WrapCursor::new(width);
    let mut row = 0usize;
    let mut row_start = 0usize;
    let mut scanned = 0usize;
    let mut found: Option<(usize, usize)> = None;
    walk(line, |col, symbol| {
        if wrap.push(symbol) {
            row += 1;
            row_start = col;
        }
        scanned = col + symbol.chars().count();
        // 複数 char からなる grapheme の途中を指した場合はその先頭に丸める
        if char_col < scanned {
            found = Some((row, col - row_start));
            return false;
        }
        true
    });
    if let Some(hit) = found {
        return hit;
    }
    // 行末より後ろ: カーソル自体が 1 セルを要求する
    if wrap.push(" ") {
        row += 1;
        row_start = scanned;
    }
    (row, char_col.saturating_sub(row_start))
}

/// 視覚行 row が含む normalize 後 char 座標の範囲 [start, end)。
/// row が視覚行数を超える場合は最終視覚行に丸める
fn wrap_row_range(line: &str, row: usize, width: usize) -> (usize, usize) {
    let mut wrap = WrapCursor::new(width);
    let mut current = 0usize;
    let mut start = 0usize;
    let mut scanned = 0usize;
    let mut done: Option<(usize, usize)> = None;
    walk(line, |col, symbol| {
        if wrap.push(symbol) {
            if current == row {
                done = Some((start, col));
                return false;
            }
            current += 1;
            start = col;
        }
        scanned = col + symbol.chars().count();
        true
    });
    done.unwrap_or((start, scanned))
}

/// クリック座標 (視覚行 row・行内の表示セル cell) → normalize 後の char 座標。
/// 全角の 2 セル目を指した場合はその文字自身に丸める。幅 0 の grapheme は
/// WrapCursor と同じくセルを消費しない (消費させると結合文字の直後の文字を指せなくなる)
pub fn wrap_col_at(line: &str, row: usize, cell: usize, width: usize) -> usize {
    let (start, end) = wrap_row_range(line, row, width);
    let mut used = 0usize;
    let mut hit: Option<usize> = None;
    walk(line, |col, symbol| {
        if col < start {
            return true;
        }
        if col >= end {
            return false;
        }
        let cells = cells(symbol);
        if cell < used + cells {
            hit = Some(col);
            return false;
        }
        used += cells;
        true
    });
    hit.unwrap_or(end)
}

#[cfg(test)]
mod tests {
    use super::{cells, wrap_col_at, wrap_position, wrap_rows};

    #[test]
    fn cells_counts_the_rendered_width_of_a_grapheme() {
        assert_eq!(cells("a"), 1);
        assert_eq!(cells("あ"), 2);
        // ZWJ 絵文字は char ごとの合計 (2 + 0 + 2) ではなく 1 つ 2 セルとして描かれる
        assert_eq!(cells("👩\u{200d}💻"), 2);
    }

    #[test]
    fn wrap_rows_counts_cells_not_chars() {
        assert_eq!(wrap_rows("", 4), 1);
        assert_eq!(wrap_rows("abcd", 4), 1);
        assert_eq!(wrap_rows("abcde", 4), 2);
        // 全角 2 文字で 4 セル埋まる
        assert_eq!(wrap_rows("ああ", 4), 1);
        assert_eq!(wrap_rows("あああ", 4), 2);
        // 折返し幅が奇数だと全角が境界を跨げず 1 セル余る (割り算では出せない)
        assert_eq!(wrap_rows("あああ", 3), 3);
        // タブは展開して数える
        assert_eq!(wrap_rows("\tab", 4), 2);
    }

    #[test]
    fn wrap_rows_keeps_a_zwj_sequence_on_one_row() {
        // char で数えると 4 セル扱いになり 3 セル目で割れてしまう
        assert_eq!(wrap_rows("👩\u{200d}💻ab", 4), 1);
        assert_eq!(wrap_rows("👩\u{200d}💻abc", 4), 2);
    }

    #[test]
    fn wrap_position_follows_the_same_boundaries() {
        assert_eq!(wrap_position("abcde", 0, 4), (0, 0));
        assert_eq!(wrap_position("abcde", 4, 4), (1, 0));
        // 行がちょうど埋まっている位置のカーソルは次の視覚行の先頭へ
        assert_eq!(wrap_position("abcd", 4, 4), (1, 0));
        // 全角 1 文字 (2 セル) で幅 3 を使い切れないので 2 文字目は次の行
        assert_eq!(wrap_position("あああ", 1, 3), (1, 0));
        assert_eq!(wrap_position("あああ", 2, 3), (2, 0));
    }

    #[test]
    fn wrap_col_at_maps_cells_back_to_chars() {
        assert_eq!(wrap_col_at("abcde", 0, 2, 4), 2);
        assert_eq!(wrap_col_at("abcde", 1, 0, 4), 4);
        // 全角の 2 セル目を指してもその文字自身に丸める
        assert_eq!(wrap_col_at("ああああ", 0, 3, 4), 1);
        assert_eq!(wrap_col_at("ああああ", 1, 0, 4), 2);
        // 行末より右は行末へ
        assert_eq!(wrap_col_at("ab", 0, 9, 4), 2);
    }

    #[test]
    fn wrap_col_at_does_not_give_a_cell_to_zero_width_marks() {
        // "a" + 結合アクセント + "b"。アクセントは 0 セルなので、セル 1 は "b"
        assert_eq!(wrap_col_at("a\u{301}b", 0, 0, 8), 0);
        assert_eq!(wrap_col_at("a\u{301}b", 0, 1, 8), 2);
        // ZWJ 絵文字 (2 セル) の次はセル 2
        assert_eq!(wrap_col_at("👩\u{200d}💻b", 0, 2, 8), 3);
    }
}
