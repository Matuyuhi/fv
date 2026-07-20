/// テキストペインの「見え方」の状態。閲覧と編集で同じ実体を共有し、
/// モード遷移でスクロール位置が飛ばないようにする。
/// 「wrap 中は hscroll = 0」のインバリアントはこの型のメソッドが守る
/// (フィールドを直接書く側はインバリアントを壊さない責任を持つ)
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

    /// 指定行が viewport の中央付近に来るようスクロールする (検索ジャンプ・:N 用)
    pub fn center_on(&mut self, line: usize, last_line: usize) {
        let half = self.height / 2;
        self.scroll = line.saturating_sub(half).min(last_line);
    }
}
