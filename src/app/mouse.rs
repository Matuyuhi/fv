use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::{App, Focus, Lane, Mode};

impl App {
    /// マウス操作。Input/Finder 中はクリック位置の意味が入力欄と衝突するため無視する
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        // 幅の変更はレーン・フォーカスと直交する操作なので、編集中でも効くよう最初に見る
        if self.on_split_mouse(&mouse) {
            return;
        }
        // オーバーレイ (Finder 等に加え Mode::Confirm) が開いている間はクリック位置の意味が
        // 入力欄・確認ダイアログと衝突するため、Edit レーンのカーソル移動より先に弾く
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        if let Lane::Edit(_) = self.lane {
            self.on_edit_mouse(mouse);
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
                    self.scroll_right_pane(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.tree.move_selection(3);
                } else if self.viewer_area.contains(pos) {
                    self.scroll_right_pane(3);
                }
            }
            _ => {}
        }
    }

    // ペイン境界のドラッグ。消費したら true を返し、通常のクリック処理には渡さない。
    // 掴んだ後は境界の外に出ても追従させる (下限幅までは張り付き、戻せば再び追従する)
    fn on_split_mouse(&mut self, mouse: &MouseEvent) -> bool {
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left)
                if matches!(self.mode, Mode::Normal) && self.splitter_area.contains(pos) =>
            {
                self.pending_g = false;
                self.dragging_split = Some(mouse.column.saturating_sub(self.splitter_area.x));
                true
            }
            // ボタン状態を報告しない端末では Drag でなく Moved で届くため両方受ける
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved => {
                match self.dragging_split {
                    Some(grab) => {
                        self.set_split_at(mouse.column, grab);
                        true
                    }
                    None => false,
                }
            }
            MouseEventKind::Up(MouseButton::Left) if self.dragging_split.is_some() => {
                self.dragging_split = None;
                // ドラッグ中は毎フレーム書き込まないよう、離した時だけ永続化する
                self.persist_config();
                true
            }
            _ => false,
        }
    }

    // 右ペインの中身はレーンで変わる (VIEW はファイル、GIT は diff)
    fn scroll_right_pane(&mut self, delta: isize) {
        match &mut self.lane {
            Lane::Git(git) => git.scroll_by(delta),
            _ => self.viewer.scroll_by(delta),
        }
    }

    // 編集中: viewer ペイン内のクリックはカーソル移動、ホイールはスクロール。
    // ツリー側は編集のモーダル性を保つため反応させない
    fn on_edit_mouse(&mut self, mouse: MouseEvent) {
        let pos = Position::new(mouse.column, mouse.row);
        let area = self.viewer_area;
        let Lane::Edit(state) = &mut self.lane else {
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
            self.open_selected(&path);
        }
    }
}
