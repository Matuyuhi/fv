/// テキストペインの「見え方」の状態。閲覧と編集で同じ実体を共有し、
/// モード遷移でスクロール位置が飛ばないようにする。
/// 「wrap 中は hscroll = 0」のインバリアントはこの型のメソッドが守る
/// (フィールドを直接書く側はインバリアントを壊さない責任を持つ)
/// Clone だけを derive し Copy は付けない。side-by-side が「wrap は独自に事前分割するので
/// TextPane には wrap=false で渡したい」という一時コピーを作るが、Copy にすると暗黙のコピーで
/// 「同じ実体を共有する」前提を破っても呼び出し側から見えなくなる。明示的な .clone() を
/// 残すことで、コピーを作っている箇所がレビューで目に付くようにする
#[derive(Clone)]
pub struct Viewport {
    pub scroll: usize,
    /// wrap off 時のみ有効な水平スクロール量 (char 単位)
    pub hscroll: usize,
    /// ファイルを跨いで維持する折返し設定
    pub wrap: bool,
    /// 描画時に ui 側が実測値を書き戻す (罫線を除いた内側)
    pub height: usize,
    pub width: usize,
}

impl Viewport {
    pub fn new(wrap: bool) -> Self {
        Self {
            scroll: 0,
            hscroll: 0,
            wrap,
            height: 0,
            width: 0,
        }
    }

    pub fn scroll_by(&mut self, delta: isize, last_line: usize) {
        self.scroll = (self.scroll as isize + delta).clamp(0, last_line as isize) as usize;
    }

    /// 水平スクロール。wrap 中は no-op (呼び出し側の条件分岐と二重に守る)
    pub fn hscroll_by(&mut self, delta: isize, max: usize) {
        if self.wrap {
            return;
        }
        self.hscroll = (self.hscroll as isize + delta).clamp(0, max as isize) as usize;
    }

    /// 折返しトグル。有効化した瞬間は水平スクロール位置の意味が失われるので 0 に戻す
    pub fn toggle_wrap(&mut self) {
        self.wrap = !self.wrap;
        if self.wrap {
            self.hscroll = 0;
        }
    }

    /// 指定行が viewport の縦範囲に収まるようスクロールする (non-wrap の視覚行 = 論理行前提)
    pub fn ensure_row_visible(&mut self, line: usize) {
        let height = self.height.max(1);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + height {
            self.scroll = line + 1 - height;
        }
    }

    /// 指定の表示桁が横範囲 (content_width 桁) に収まるよう hscroll を動かす
    pub fn ensure_col_visible(&mut self, display: usize, content_width: usize) {
        let width = content_width.max(1);
        if display < self.hscroll {
            self.hscroll = display;
        } else if display >= self.hscroll + width {
            self.hscroll = display + 1 - width;
        }
    }

    /// 画面内座標 (ペイン内側の row/col) → (論理行, 表示桁)。wrap 中は描画 (text_pane) と
    /// 同じ数え方 (text::wrap_rows) で視覚行を辿る。描画・カーソル追従・クリック座標の
    /// 3 者が同じ折返し計算を共有するための入口 (CLAUDE.md の桁インバリアント) なので、
    /// クリック位置を解釈する側 (編集のカーソル移動・閲覧の範囲選択) はここだけを通す。
    /// display_len は「その論理行が占める表示桁数」を返すクロージャ
    pub fn locate(
        &self,
        row: usize,
        col: usize,
        gutter_width: usize,
        line_count: usize,
        display_len: impl Fn(usize) -> usize,
    ) -> (usize, usize) {
        let last = line_count.saturating_sub(1);
        let content_col = col.saturating_sub(gutter_width);
        if !self.wrap {
            return ((self.scroll + row).min(last), self.hscroll + content_col);
        }
        let width = self.width.saturating_sub(gutter_width).max(1);
        let mut line = self.scroll.min(last);
        let mut remaining = row;
        loop {
            let rows = crate::text::wrap_rows(display_len(line), width);
            if remaining < rows || line >= last {
                remaining = remaining.min(rows - 1);
                break;
            }
            remaining -= rows;
            line += 1;
        }
        (line, remaining * width + content_col)
    }

    /// 指定行が viewport の中央付近に来るようスクロールする (検索ジャンプ・:N 用)
    pub fn center_on(&mut self, line: usize, last_line: usize) {
        let half = self.height / 2;
        self.scroll = line.saturating_sub(half).min(last_line);
    }
}

#[cfg(test)]
mod tests {
    use super::Viewport;

    fn viewport(wrap: bool, scroll: usize, hscroll: usize, width: usize) -> Viewport {
        let mut vp = Viewport::new(wrap);
        vp.scroll = scroll;
        vp.hscroll = hscroll;
        vp.width = width;
        vp.height = 10;
        vp
    }

    #[test]
    fn locate_without_wrap_is_scroll_plus_row() {
        let vp = viewport(false, 5, 3, 40);
        // gutter 2 桁のぶんだけコンテンツ桁が右にずれ、hscroll が足し戻される
        assert_eq!(vp.locate(2, 9, 2, 100, |_| 80), (7, 3 + 7));
        // gutter の上をクリックしたらコンテンツ桁 0 に丸める
        assert_eq!(vp.locate(0, 1, 2, 100, |_| 80), (5, 3));
    }

    #[test]
    fn locate_without_wrap_clamps_past_the_last_line() {
        let vp = viewport(false, 5, 0, 40);
        assert_eq!(vp.locate(50, 2, 2, 8, |_| 10).0, 7);
    }

    #[test]
    fn locate_with_wrap_walks_visual_rows() {
        // 幅 40・gutter 2 → 折返し幅 38。行 0 は 3 視覚行、以降は 1 視覚行ずつ
        let vp = viewport(true, 0, 0, 40);
        let len = |line: usize| if line == 0 { 100 } else { 10 };
        assert_eq!(vp.locate(0, 2, 2, 5, len), (0, 0));
        // 行 0 の 2 段目の先頭
        assert_eq!(vp.locate(1, 2, 2, 5, len), (0, 38));
        // 行 0 の 3 段目 + 5 桁
        assert_eq!(vp.locate(2, 7, 2, 5, len), (0, 76 + 5));
        // 行 0 を跨いだ次の視覚行が論理行 1
        assert_eq!(vp.locate(3, 2, 2, 5, len), (1, 0));
        assert_eq!(vp.locate(4, 4, 2, 5, len), (2, 2));
    }

    #[test]
    fn locate_with_wrap_clamps_to_the_last_visual_row() {
        let vp = viewport(true, 0, 0, 40);
        // 最終行より下をクリックしても、その行の最後の視覚行に留める
        assert_eq!(vp.locate(9, 2, 2, 2, |_| 10), (1, 0));
    }
}
