//! 閲覧 (viewer) と編集 (editor) が共有する桁計算の唯一の定義。
//! タブ幅・gutter 幅の解釈が場所によってズレると、検索ハイライト・カーソル・
//! クリック座標の桁対応 (CLAUDE.md の整合インバリアント) が全て壊れるため一箇所に集める。

use ratatui::text::Span;

/// タブ 1 文字の展開結果。normalize と display_col/char_col_at の換算は必ずこれ経由で揃える
pub const TAB_EXPANDED: &str = "    ";

/// 改行を落とし、端末で幅が不定になるタブをスペースに展開する
pub fn normalize(segment: &str) -> String {
    segment
        .trim_end_matches(['\n', '\r'])
        .replace('\t', TAB_EXPANDED)
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

/// 1 文字が端末で占めるセル数 (全角 = 2、結合文字 = 0)。ratatui の描画
/// (LineTruncator) は grapheme の display width で桁を送り、収まらない分をその行で
/// 打ち切るので、折返し位置も同じ尺度で決めないと「折返しの継ぎ目で文字が消える」。
/// unicode-width を直接足さず ratatui の Span::width を通すのは、描画側とまったく
/// 同じ計算であることを型で保証するため (新規依存も増やさない)
pub fn char_cells(c: char) -> usize {
    let mut buf = [0u8; 4];
    Span::raw(&*c.encode_utf8(&mut buf)).width()
}

/// 折返し位置の唯一の定義。normalize 済みの char (タブは展開済み) を 1 つずつ食わせ、
/// 「その char を置く前に折り返すか」を返す。描画 (text_pane::wrap_line)・視覚行数
/// (wrap_rows)・カーソル追従 (wrap_position)・クリック座標 (wrap_col_at) の 4 者が
/// この 1 つの規則を共有する
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

    /// c を今の視覚行に置けなければ true (= c は次の視覚行の先頭になる)。
    /// 折返し幅より広い char は単独でも収まらないので、空の視覚行では必ず受け入れる
    /// (受け入れないと視覚行だけが無限に増える)
    pub(crate) fn push(&mut self, c: char) -> bool {
        let cells = char_cells(c);
        if self.used > 0 && self.used + cells > self.width {
            self.used = cells;
            return true;
        }
        self.used += cells;
        false
    }
}

// タブを展開した char 列。normalize() と違い String を確保しないので、
// 折返しの計算 (毎フレーム・キー入力ごとに通る) から生の行をそのまま扱える
fn expanded(line: &str) -> impl Iterator<Item = char> + '_ {
    line.chars().flat_map(|c| {
        let (tab, single) = if c == '\t' {
            (TAB_EXPANDED, None)
        } else {
            ("", Some(c))
        };
        tab.chars().chain(single)
    })
}

/// 論理行が占める視覚行数 (wrap 時)。空行も 1 行を占める。
/// 全角文字は折返し境界を跨げないため単純な割り算では出せない
pub fn wrap_rows(line: &str, width: usize) -> usize {
    let mut wrap = WrapCursor::new(width);
    let mut rows = 1;
    for c in expanded(line) {
        if wrap.push(c) {
            rows += 1;
        }
    }
    rows
}

/// normalize 後の char 座標が何番目の視覚行のどこに来るか → (視覚行, 行内の char 座標)。
/// 行末より後ろ (カーソルが行末にある場合) は、直前の視覚行が埋まっていれば次の行の先頭を返す
pub fn wrap_position(line: &str, char_col: usize, width: usize) -> (usize, usize) {
    let mut wrap = WrapCursor::new(width);
    let mut row = 0usize;
    let mut row_start = 0usize;
    let mut i = 0usize;
    for c in expanded(line) {
        if wrap.push(c) {
            row += 1;
            row_start = i;
        }
        if i == char_col {
            return (row, i - row_start);
        }
        i += 1;
    }
    // 行末より後ろ: カーソル自体が 1 セルを要求する
    if wrap.push(' ') {
        row += 1;
        row_start = i;
    }
    (row, char_col.saturating_sub(row_start))
}

/// 視覚行 row が含む normalize 後 char 座標の範囲 [start, end)。
/// row が視覚行数を超える場合は最終視覚行に丸める
fn wrap_row_range(line: &str, row: usize, width: usize) -> (usize, usize) {
    let mut wrap = WrapCursor::new(width);
    let mut r = 0usize;
    let mut start = 0usize;
    let mut i = 0usize;
    for c in expanded(line) {
        if wrap.push(c) {
            if r == row {
                return (start, i);
            }
            r += 1;
            start = i;
        }
        i += 1;
    }
    (start, i)
}

/// クリック座標 (視覚行 row・行内の表示セル cell) → normalize 後の char 座標。
/// 全角文字の 2 セル目を指した場合はその文字自身に丸める
pub fn wrap_col_at(line: &str, row: usize, cell: usize, width: usize) -> usize {
    let (start, end) = wrap_row_range(line, row, width);
    let mut used = 0usize;
    for (i, c) in expanded(line).enumerate().take(end).skip(start) {
        let cells = char_cells(c).max(1);
        if cell < used + cells {
            return i;
        }
        used += cells;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::{char_cells, wrap_col_at, wrap_position, wrap_rows};

    #[test]
    fn char_cells_counts_east_asian_width() {
        assert_eq!(char_cells('a'), 1);
        assert_eq!(char_cells('あ'), 2);
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
}
