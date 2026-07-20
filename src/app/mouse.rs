use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::{App, Focus, Mode};

impl App {
    /// マウス操作。Input/Finder 中はクリック位置の意味が入力欄と衝突するため無視する
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        if let Mode::Edit(_) = self.mode {
            self.on_edit_mouse(mouse);
            return;
        }
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        // クリック/スクロールはどちらも文脈を切り替えうるので、キー入力の g 待ちと同様に破棄する
        self.pending_g = false;
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.tree_area.contains(pos) {
                    self.focus = Focus::Tree;
                    self.click_tree_row(mouse.row);
                } else if self.viewer_area.contains(pos) {
                    self.focus = Focus::Viewer;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.tree_area.contains(pos) {
                    self.tree.move_selection(-3);
                } else if self.viewer_area.contains(pos) {
                    self.viewer.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.tree.move_selection(3);
                } else if self.viewer_area.contains(pos) {
                    self.viewer.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    // 編集中: viewer ペイン内のクリックはカーソル移動、ホイールはスクロール。
    // ツリー側は編集のモーダル性を保つため反応させない
    fn on_edit_mouse(&mut self, mouse: MouseEvent) {
        let pos = Position::new(mouse.column, mouse.row);
        let area = self.viewer_area;
        let Mode::Edit(state) = &mut self.mode else {
            return;
        };
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if area.contains(pos) => {
                // 枠線 (上・左 1 セル) の内側だけをコンテンツ座標に変換する
                let (Some(row), Some(col)) = (
                    mouse.row.checked_sub(area.y + 1),
                    mouse.column.checked_sub(area.x + 1),
                ) else {
                    return;
                };
                state.click_at(row as usize, col as usize, &self.viewer.viewport);
            }
            MouseEventKind::ScrollUp => self.viewer.scroll_by(-3),
            MouseEventKind::ScrollDown => self.viewer.scroll_by(3),
            _ => {}
        }
    }

    // クリックされた画面行をツリーの selected に変換する。上枠1行分を引き、
    // ListState::offset() (直前フレームでのスクロールオフセット) を足して実際の行 index を求める。
    // 範囲外 (枠線や空行をクリックした場合) は選択を変えずフォーカス移動のみで終える
    fn click_tree_row(&mut self, row: u16) {
        let row =
            row as isize - self.tree_area.y as isize - 1 + self.tree.list_state.offset() as isize;
        if row < 0 || row as usize >= self.tree.visible.len() {
            return;
        }
        self.tree.selected = row as usize;
        if let Some(path) = self.tree.toggle_or_open() {
            self.viewer.open(&path, &self.root);
        }
    }
}
