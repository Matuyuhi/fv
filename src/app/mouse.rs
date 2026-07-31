use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::{App, Focus, Lane, Mode, Workspace};

impl App {
    /// マウス操作。Input/Finder 中はクリック位置の意味が入力欄と衝突するため無視する
    pub fn on_mouse(&mut self, mouse: MouseEvent) {
        // 幅の変更・タブクリックはレーン・フォーカスと直交する操作なので、編集中でも効くよう最初に見る
        if self.on_split_mouse(&mouse) {
            return;
        }
        // オーバーレイ (Finder 等に加え Mode::Confirm) が開いている間はクリック位置の意味が
        // 入力欄・確認ダイアログと衝突するため、タブクリックより先に弾く
        // (キー側で Ctrl+t をオーバーレイ判定の後ろに置いているのと同じ優先順位)
        if !matches!(self.mode, Mode::Normal) {
            return;
        }
        if self.on_tab_mouse(&mouse) {
            return;
        }
        // Viewer 以外のタブはツリー・ビューアの概念を持たない。issues/PR はそれぞれ専用ハンドラを持つ
        if !matches!(self.workspace, Workspace::Viewer) {
            match self.workspace {
                Workspace::Issues => self.on_issues_mouse(mouse),
                Workspace::PullRequests => self.on_pr_mouse(mouse),
                Workspace::Viewer => {}
            }
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
                    if matches!(self.lane, Lane::Log(_)) {
                        self.click_log_row(mouse.row);
                    } else {
                        self.click_tree_row(mouse.row);
                    }
                } else if self.viewer_area.contains(pos) {
                    self.focus = Focus::Viewer;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.tree_area.contains(pos) {
                    self.scroll_left_pane(-3);
                } else if self.viewer_area.contains(pos) {
                    self.scroll_right_pane(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.scroll_left_pane(3);
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

    // タブバーのクリック。ペイン境界のドラッグと同じくレーン・オーバーレイ判定より前で消費する
    // (タブ移動はレーンと直交する操作)。使えない間 (workspace_available が false) は
    // tab_areas が全て空 Rect のままなのでヒットせず自然に no-op になる
    fn on_tab_mouse(&mut self, mouse: &MouseEvent) -> bool {
        let MouseEventKind::Down(MouseButton::Left) = mouse.kind else {
            return false;
        };
        if !matches!(self.mode, Mode::Normal) {
            return false;
        }
        let pos = Position::new(mouse.column, mouse.row);
        let Some(index) = self.tab_areas.iter().position(|area| area.contains(pos)) else {
            return false;
        };
        self.pending_g = false;
        self.set_workspace(Workspace::from_index(index));
        true
    }

    // 左ペインの中身はレーンで変わる (VIEW/GIT はツリー、LOG はコミット一覧)。ホイールは
    // j/k のスクロールと同じ扱いで、LOG でも diff の自動追従はしない (move_selection 参照)
    fn scroll_left_pane(&mut self, delta: isize) {
        if matches!(self.lane, Lane::Log(_)) {
            let root = self.root.clone();
            if let Lane::Log(log) = &mut self.lane {
                log.move_selection(&root, delta);
            }
            return;
        }
        self.tree.move_selection(delta);
    }

    // 右ペインの中身はレーンで変わる (VIEW はファイル、GIT/LOG は diff)
    fn scroll_right_pane(&mut self, delta: isize) {
        match &mut self.lane {
            Lane::Git(git) => git.scroll_by(delta),
            Lane::Log(log) => log.scroll_by(delta),
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

    // issues タブ (#33) のマウス操作。左右ペインの領域判定は Viewer タブと同じ tree_area/
    // viewer_area を使い回す (draw_issues_workspace が同じ書き戻しパターンで埋める)
    fn on_issues_mouse(&mut self, mouse: MouseEvent) {
        self.pending_g = false;
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.tree_area.contains(pos) {
                    self.focus = Focus::Tree;
                    self.click_issue_row(mouse.row);
                } else if self.viewer_area.contains(pos) {
                    self.focus = Focus::Viewer;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.tree_area.contains(pos) {
                    self.issues.move_selection(-3);
                } else if self.viewer_area.contains(pos) {
                    self.issues.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.issues.move_selection(3);
                } else if self.viewer_area.contains(pos) {
                    self.issues.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    // pull requests タブ (#34) のマウス操作。issues (#33) と同じ tree_area/viewer_area を使い回す
    fn on_pr_mouse(&mut self, mouse: MouseEvent) {
        self.pending_g = false;
        let pos = Position::new(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.tree_area.contains(pos) {
                    self.focus = Focus::Tree;
                    self.click_pr_row(mouse.row);
                } else if self.viewer_area.contains(pos) {
                    self.focus = Focus::Viewer;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.tree_area.contains(pos) {
                    self.prs.move_selection(-3);
                } else if self.viewer_area.contains(pos) {
                    self.prs.scroll_by(-3);
                }
            }
            MouseEventKind::ScrollDown => {
                if self.tree_area.contains(pos) {
                    self.prs.move_selection(3);
                } else if self.viewer_area.contains(pos) {
                    self.prs.scroll_by(3);
                }
            }
            _ => {}
        }
    }

    // click_issue_row と同じ座標変換。クリックは Enter/l と同じ明示操作 (説明表示で開く)
    fn click_pr_row(&mut self, row: u16) {
        let row =
            row as isize - self.tree_area.y as isize - 1 + self.prs.list_state.offset() as isize;
        if row < 0 || row as usize >= self.prs.matches.len() {
            return;
        }
        self.prs.selected = row as usize;
        self.open_selected_pr();
    }

    // click_tree_row と同じ座標変換 (上枠1行 + list_state.offset())。クリックは Enter/l と同じ
    // 明示操作なので、j/k と違い自動追従 (詳細取得) を避ける理由がない
    fn click_issue_row(&mut self, row: u16) {
        let row =
            row as isize - self.tree_area.y as isize - 1 + self.issues.list_state.offset() as isize;
        if row < 0 || row as usize >= self.issues.matches.len() {
            return;
        }
        self.issues.selected = row as usize;
        self.open_selected_issue();
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

    // click_tree_row と同じ座標変換で、コミット一覧の行をクリックしたら即 diff を開く
    // (クリックは Enter/l と同じ明示操作なので、j/k と違い自動追従を避ける理由がない)
    fn click_log_row(&mut self, row: u16) {
        let root = self.root.clone();
        let area_y = self.tree_area.y;
        let Lane::Log(log) = &mut self.lane else {
            return;
        };
        let row = row as isize - area_y as isize - 1 + log.list_state.offset() as isize;
        if row < 0 || row as usize >= log.commits().len() {
            return;
        }
        log.selected = row as usize;
        log.list_state.select(Some(log.selected));
        log.open_selected(&root);
    }
}
