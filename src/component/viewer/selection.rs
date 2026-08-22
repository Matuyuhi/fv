//! VIEW レーンの範囲選択。閲覧はカーソルを持たない (編集と違い EditState が無い) ので、
//! 選択そのものが「今どこを指しているか」を持つ唯一の状態になる。
//!
//! 座標は plain (タブ展開済み) の char インデックス。描画桁と 1:1 対応する
//! (CLAUDE.md の桁インバリアント) ので、検索マッチと同じやり方でそのまま背景色を重ねられる。
//! 一方コピーする中身は raw から取り出す — plain のままだとタブが空白 4 個に化けて
//! 貼り付け先のインデントが壊れるため、text::char_col_at で桁を raw の char 座標へ戻す。

use crate::text;

/// plain 座標の 1 点。フィールド順のまま lexicographic に比較できるので、
/// anchor/head の前後判定はそのまま `<` で足りる
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Point {
    pub line: usize,
    pub col: usize,
}

pub struct Selection {
    /// 掴んだ側 (押した位置 / v を押した行)。伸ばしても動かない
    anchor: Point,
    /// 伸ばしている側。ドラッグと j/k はこちらだけを動かす
    head: Point,
    /// 行単位選択。キーボード (v) だけがこれを作り、マウスのドラッグは常に char 単位。
    /// 「今の選択を j/k で伸縮してよいか」の判定もこのフラグを兼ねる
    pub linewise: bool,
    /// マウスのボタンを押している間だけ true。ボタン状態を報告しない端末では Drag が
    /// Moved で届くため、Moved を伸縮として扱ってよいかの判定に使う (on_split_mouse と同じ手)
    pub dragging: bool,
}

impl Selection {
    pub fn new(at: Point, linewise: bool, dragging: bool) -> Self {
        Self {
            anchor: at,
            head: at,
            linewise,
            dragging,
        }
    }

    pub fn set_head(&mut self, at: Point) {
        self.head = at;
    }

    pub fn head_line(&self) -> usize {
        self.head.line
    }

    /// 選択が空 (掴んだだけでまだ伸ばしていない char 単位選択) か。
    /// 行単位選択は 1 行を指すので空にはならない
    pub fn is_empty(&self) -> bool {
        !self.linewise && self.anchor == self.head
    }

    /// 選択がまたぐ論理行数 (ステータスバー表示用)
    pub fn line_count(&self) -> usize {
        let (start, end) = self.range();
        end.line - start.line + 1
    }

    // 前後を正規化した (始点, 終点)。終点は exclusive
    fn range(&self) -> (Point, Point) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// 指定の論理行のうち選択に入っている桁範囲 [start, end)。行末まで (改行を含む) の
    /// 場合は end = usize::MAX を返す — 行の実際の長さを知らなくても表現できるようにするため
    pub fn columns_at(&self, line: usize) -> Option<(usize, usize)> {
        let (start, end) = self.range();
        if line < start.line || line > end.line {
            return None;
        }
        if self.linewise {
            return Some((0, usize::MAX));
        }
        let from = if line == start.line { start.col } else { 0 };
        let to = if line == end.line {
            end.col
        } else {
            usize::MAX
        };
        (from < to).then_some((from, to))
    }

    /// 選択範囲のテキスト。raw (タブ未展開) から取り出すので、貼り付け先で
    /// 元のインデントがそのまま再現される
    pub fn text(&self, raw: &[String]) -> String {
        if raw.is_empty() {
            return String::new();
        }
        let last = raw.len() - 1;
        let (start, end) = self.range();
        let (first_line, last_line) = (start.line.min(last), end.line.min(last));
        if self.linewise {
            // 行単位は行末の改行まで含める (行ごと貼り付けられるようにする)
            let mut out = String::new();
            for line in &raw[first_line..=last_line] {
                out.push_str(line);
                out.push('\n');
            }
            return out;
        }
        let mut out = String::new();
        for (i, line) in raw[first_line..=last_line].iter().enumerate() {
            let i = first_line + i;
            let from = if i == start.line {
                text::char_col_at(line, start.col)
            } else {
                0
            };
            let to = if i == end.line {
                text::char_col_at(line, end.col)
            } else {
                line.chars().count()
            };
            out.extend(line.chars().skip(from).take(to.saturating_sub(from)));
            if i < last_line {
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{Point, Selection};

    fn raw() -> Vec<String> {
        vec![
            "fn main() {".to_string(),
            "\tprintln!(\"hi\");".to_string(),
            "}".to_string(),
        ]
    }

    fn at(line: usize, col: usize) -> Point {
        Point { line, col }
    }

    #[test]
    fn charwise_single_line() {
        let mut sel = Selection::new(at(0, 3), false, true);
        sel.set_head(at(0, 7));
        assert_eq!(sel.text(&raw()), "main");
        assert_eq!(sel.columns_at(0), Some((3, 7)));
        assert_eq!(sel.columns_at(1), None);
    }

    #[test]
    fn charwise_backwards_drag_is_normalized() {
        let mut sel = Selection::new(at(0, 7), false, true);
        sel.set_head(at(0, 3));
        assert_eq!(sel.text(&raw()), "main");
    }

    #[test]
    fn charwise_multiline_keeps_tabs() {
        let mut sel = Selection::new(at(0, 10), false, true);
        // plain 上の桁 4 はタブ展開後なので、raw では tab の直後 (char 1) に戻る
        sel.set_head(at(1, 4 + 7));
        assert_eq!(sel.text(&raw()), "{\n\tprintln");
    }

    #[test]
    fn linewise_includes_trailing_newline() {
        let mut sel = Selection::new(at(1, 0), true, false);
        sel.set_head(at(2, 0));
        assert_eq!(sel.text(&raw()), "\tprintln!(\"hi\");\n}\n");
        assert_eq!(sel.columns_at(1), Some((0, usize::MAX)));
        assert_eq!(sel.line_count(), 2);
    }

    #[test]
    fn empty_charwise_selection_paints_nothing() {
        let sel = Selection::new(at(1, 2), false, true);
        assert!(sel.is_empty());
        assert_eq!(sel.columns_at(1), None);
    }
}
