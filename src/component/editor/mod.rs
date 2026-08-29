mod buffer;
// word-level diff (#29) が LCS 実装を再利用するため gitlane からも見える必要がある
pub(crate) mod diff;
pub mod view;
mod word;

pub use buffer::EditBuffer;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::component::viewer::{HighlightCache, Touched, Viewer, Viewport};
use crate::git;
use crate::text;

/// これを超えるファイルは編集対象にしない (メモリ・再ハイライトの両面で現実的でないため)
const MAX_EDIT_BYTES: u64 = 10 * 1024 * 1024;

pub enum EditOutcome {
    Continue,
    Exit,
}

/// インライン編集の状態。閲覧側とは「Viewport (スクロール共有) だけを借りる」関係に留め、
/// Viewer の cache・履歴・検索には触らない。保存 (save) だけは cache の即時更新のため
/// Viewer::reload を呼ぶ。ハイライトは描画時に可視範囲だけ組み立てるので、編集操作の
/// 経路 (handle_key/paste) は Highlighter を一切借りない
pub struct EditState {
    pub path: PathBuf,
    pub buffer: EditBuffer,
    /// (line, col)。バッファの生テキスト上の char 座標 (タブは 1 char)
    pub cursor: (usize, usize),
    // 上下移動で維持する目標列。短い行を跨いでも元の列に戻れるようにする (vim 相当)
    desired_col: usize,
    /// 可視範囲の描画キャッシュ。編集は「変更行以降を無効化する」だけで、
    /// 実際に組み直すのは次の描画で画面に映る行だけ
    pub render: HighlightCache,
    /// 保存エラー・discard 確認などステータスバーに出す一時メッセージ
    pub notice: Option<String>,
    confirm_discard: bool,
    /// 直近の操作で保存が成功したか。App は EditState から借りられない (依存範囲の制約) ため、
    /// 「保存で差分が生まれた/消えた」を App へ伝える take フラグとして持つ
    saved: bool,
    // ライブ diff の比較元 (編集開始時の HEAD / index 版)。repo 外・untracked は None
    baseline: Option<Vec<String>>,
    /// baseline と現在のバッファが「どこまで共通か」。1 打鍵ごとに文書全体を舐め直さない
    /// ための持ち越しで、触った行から次の下限を O(1) で絞る (component/editor/diff.rs)
    trim: diff::CommonTrim,
    /// 未保存バッファ vs baseline の変更行 (1-origin)。viewer の changed_lines と同じ描画に使う
    pub changed_lines: Option<HashSet<usize>>,
}

impl EditState {
    /// 編集セッションを開始する。非 UTF-8・巨大ファイル・読込失敗は None (呼び出し側で no-op)
    pub fn open(path: &Path, start_line: usize, root: &Path) -> Option<Self> {
        let size = fs::metadata(path).ok()?.len();
        if size > MAX_EDIT_BYTES {
            return None;
        }
        let buffer = EditBuffer::load(path).ok()?;
        let cursor_line = start_line.min(buffer.line_count() - 1);
        let mut render = HighlightCache::new();
        render.reset(path, false);
        let mut state = Self {
            path: path.to_path_buf(),
            buffer,
            cursor: (cursor_line, 0),
            desired_col: 0,
            render,
            notice: None,
            confirm_discard: false,
            saved: false,
            baseline: git::baseline_lines(root, path),
            trim: diff::CommonTrim::default(),
            changed_lines: None,
        };
        state.refresh_changed_lines(None);
        Some(state)
    }

    /// 直近の handle_key で保存が成功したかを取り出す (取ると false に戻る)。
    /// 保存で作られた差分を FS 監視のイベント待ちにせず App 側から git status へ反映させる
    pub fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    /// 行番号 gutter の char 幅 (末尾空白込み)。行数だけで決まるので状態として持たない
    pub fn gutter_width(&self) -> usize {
        text::gutter_width(self.buffer.line_count())
    }

    pub fn handle_key(&mut self, key: KeyEvent, viewer: &mut Viewer) -> EditOutcome {
        let mods = key.modifiers;
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        // SUPER (mac の Cmd) は kitty keyboard protocol 対応端末でのみ届く (main.rs で opt-in)
        let cmd = mods.contains(KeyModifiers::SUPER);
        let shift = mods.contains(KeyModifiers::SHIFT);
        // 修飾付き文字は端末により大文字 (Shift 畳み込み済み) で届くことがあるため小文字に揃える。
        // ALT も畳むのは、Option を Meta として送る端末の ESC b / ESC f を取りこぼさないため
        let code = match key.code {
            KeyCode::Char(c) if ctrl || cmd || mods.contains(KeyModifiers::ALT) => {
                KeyCode::Char(c.to_ascii_lowercase())
            }
            other => other,
        };
        // discard 確認は Esc の連続でだけ成立させる。他のキーを挟んだら仕切り直し
        let confirming = std::mem::take(&mut self.confirm_discard);
        self.notice = None;
        if confirming {
            match code {
                KeyCode::Esc => return EditOutcome::Exit,
                // Ctrl+s が端末に奪われる環境向けの逃げ道。保存できたらそのまま閲覧へ戻る
                KeyCode::Char('s') if !ctrl && !cmd => {
                    self.save(viewer);
                    return if self.buffer.dirty() {
                        EditOutcome::Continue
                    } else {
                        EditOutcome::Exit
                    };
                }
                // それ以外のキーは確認を解除した上で通常どおり処理する
                _ => {}
            }
        }
        // 端末により word 移動は Ctrl+矢印 / Alt+矢印 (mac の Option) のどちらでも届くため両方受ける
        let alt = mods.contains(KeyModifiers::ALT);
        let word = ctrl || alt;
        // 保存だけは cache 再読込のため Viewer 全体が要る。先に処理して抜けることで、
        // 以降の操作は viewport しか借りないことを型で保証する
        if code == KeyCode::Char('s') && (ctrl || cmd) {
            self.save(viewer);
            return EditOutcome::Continue;
        }
        let vp = &mut viewer.viewport;
        match code {
            KeyCode::Esc => {
                if self.buffer.dirty() {
                    self.confirm_discard = true;
                    self.notice = Some(
                        "unsaved changes — Esc: discard / s: save & exit / Ctrl+s: save"
                            .to_string(),
                    );
                    return EditOutcome::Continue;
                }
                return EditOutcome::Exit;
            }
            // mac 慣習の Cmd+Shift+z も redo に割り当てる
            KeyCode::Char('z') if (ctrl || cmd) && shift => {
                if let Some(cursor) = self.buffer.redo() {
                    self.cursor = cursor;
                    self.after_edit(vp);
                }
            }
            KeyCode::Char('z') if ctrl || cmd => {
                if let Some(cursor) = self.buffer.undo() {
                    self.cursor = cursor;
                    self.after_edit(vp);
                }
            }
            KeyCode::Char('y') if ctrl || cmd => {
                if let Some(cursor) = self.buffer.redo() {
                    self.cursor = cursor;
                    self.after_edit(vp);
                }
            }
            KeyCode::Char('k') if ctrl => self.delete_line(vp),
            // Option を Meta として送る端末 (Terminal.app 等) では Option+←/→ が
            // ESC b / ESC f として届くので、単語移動の別名として受ける
            KeyCode::Char('b') if alt && !ctrl && !cmd => self.word_left(vp),
            KeyCode::Char('f') if alt && !ctrl && !cmd => self.word_right(vp),
            // readline 慣習の行編集。Ctrl+矢印が端末に奪われる環境の逃げ道でもある
            KeyCode::Char('a') if ctrl => self.move_home(vp),
            KeyCode::Char('e') if ctrl => {
                self.move_to((self.cursor.0, self.buffer.line_len(self.cursor.0)), vp)
            }
            KeyCode::Char('w') if ctrl => self.delete_word_left(vp),
            KeyCode::Char('u') if ctrl => self.delete_to_line_start(vp),
            KeyCode::Enter => {
                self.cursor = self.buffer.insert_block(self.cursor, "\n");
                self.after_edit(vp);
            }
            // Cmd+Backspace は mac 慣習で行頭まで、Option/Ctrl+Backspace は 1 単語ぶん
            KeyCode::Backspace if cmd => self.delete_to_line_start(vp),
            KeyCode::Backspace if word => self.delete_word_left(vp),
            KeyCode::Backspace => self.backspace(vp),
            KeyCode::Delete if cmd => self.delete_to_line_end(vp),
            KeyCode::Delete if word => self.delete_word_right(vp),
            KeyCode::Delete => self.delete_forward(vp),
            KeyCode::Tab => {
                self.cursor = self.buffer.insert_typed(self.cursor, '\t');
                self.after_edit(vp);
            }
            // mac 慣習: Cmd+←/→ は行頭・行末、Cmd+↑/↓ は文書の先頭・末尾
            KeyCode::Left if cmd => self.move_home(vp),
            KeyCode::Right if cmd => {
                self.move_to((self.cursor.0, self.buffer.line_len(self.cursor.0)), vp)
            }
            KeyCode::Up if cmd => self.move_to((0, 0), vp),
            KeyCode::Down if cmd => self.move_to_end(vp),
            KeyCode::Left if word => self.word_left(vp),
            KeyCode::Right if word => self.word_right(vp),
            // VSCode 慣習: Alt+↑/↓ は行の入れ替え。Ctrl は含めない (Ctrl+↑/↓ を
            // 押しただけで行が動くのは事故になりやすい)
            KeyCode::Up if alt => self.move_line(-1, vp),
            KeyCode::Down if alt => self.move_line(1, vp),
            KeyCode::Left => self.move_left(vp),
            KeyCode::Right => self.move_right(vp),
            KeyCode::Up => self.move_vertical(-1, vp),
            KeyCode::Down => self.move_vertical(1, vp),
            KeyCode::PageUp => {
                let page = vp.height.max(1) as isize;
                self.move_vertical(-page, vp)
            }
            KeyCode::PageDown => {
                let page = vp.height.max(1) as isize;
                self.move_vertical(page, vp)
            }
            KeyCode::Home if ctrl => self.move_to((0, 0), vp),
            KeyCode::End if ctrl => self.move_to_end(vp),
            KeyCode::Home => self.move_home(vp),
            KeyCode::End => self.move_to((self.cursor.0, self.buffer.line_len(self.cursor.0)), vp),
            // Cmd/Alt 付きは未割当ショートカットの可能性が高いので文字として挿入しない
            KeyCode::Char(c) if !ctrl && !cmd && !mods.contains(KeyModifiers::ALT) => {
                self.cursor = self.buffer.insert_typed(self.cursor, c);
                self.after_edit(vp);
            }
            _ => {}
        }
        EditOutcome::Continue
    }

    /// bracketed paste の一括挿入。undo 1 単位に畳む
    pub fn paste(&mut self, text: &str, viewport: &mut Viewport) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.cursor = self.buffer.insert_block(self.cursor, &text);
        self.after_edit(viewport);
    }

    /// マウスクリック。row/col はコンテンツ領域 (枠線の内側) 相対の画面座標
    pub fn click_at(&mut self, row: usize, col: usize, vp: &Viewport) {
        // 折返し中の視覚行の辿り方は描画 (text_pane) と共有する (Viewport::locate)
        let (line, display) = vp.locate(
            row,
            col,
            self.gutter_width(),
            self.buffer.line_count(),
            |i| self.buffer.line(i),
        );
        let col = text::char_col_at(self.buffer.line(line), display);
        self.cursor = (line, col);
        self.desired_col = col;
        self.buffer.seal();
    }

    fn save(&mut self, viewer: &mut Viewer) {
        match fs::write(&self.path, self.buffer.to_text()) {
            Ok(()) => {
                self.buffer.mark_saved();
                self.saved = true;
                // cache と git 変更行マークを watcher を待たずに即時更新する
                viewer.reload(&self.path);
                // reload は hscroll を 0 に戻すため、カーソル位置まで追従し直す
                self.ensure_visible(&mut viewer.viewport);
                self.notice = Some("saved".to_string());
            }
            Err(e) => self.notice = Some(format!("save failed: {e}")),
        }
    }

    fn backspace(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if col > 0 {
            self.buffer.delete((line, col - 1), (line, col));
            self.cursor = (line, col - 1);
        } else if line > 0 {
            let prev_len = self.buffer.line_len(line - 1);
            self.buffer.delete((line - 1, prev_len), (line, 0));
            self.cursor = (line - 1, prev_len);
        } else {
            return;
        }
        self.after_edit(vp);
    }

    fn delete_forward(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if col < self.buffer.line_len(line) {
            self.buffer.delete((line, col), (line, col + 1));
        } else if line + 1 < self.buffer.line_count() {
            self.buffer.delete((line, col), (line + 1, 0));
        } else {
            return;
        }
        self.after_edit(vp);
    }

    /// Ctrl+k: カーソル行を丸ごと削除。最終行は内容だけ消す (バッファは常に 1 行以上を保つ)
    fn delete_line(&mut self, vp: &mut Viewport) {
        let (line, _) = self.cursor;
        if line + 1 < self.buffer.line_count() {
            self.buffer.delete((line, 0), (line + 1, 0));
        } else if self.buffer.line_len(line) > 0 {
            self.buffer
                .delete((line, 0), (line, self.buffer.line_len(line)));
        } else {
            return;
        }
        self.cursor = (line.min(self.buffer.line_count() - 1), 0);
        self.after_edit(vp);
    }

    fn move_left(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let target = if col > 0 {
            (line, col - 1)
        } else if line > 0 {
            (line - 1, self.buffer.line_len(line - 1))
        } else {
            return;
        };
        self.move_to(target, vp);
    }

    fn move_right(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let target = if col < self.buffer.line_len(line) {
            (line, col + 1)
        } else if line + 1 < self.buffer.line_count() {
            (line + 1, 0)
        } else {
            return;
        };
        self.move_to(target, vp);
    }

    // 上下移動は desired_col を保つため move_to を通さない
    fn move_vertical(&mut self, delta: isize, vp: &mut Viewport) {
        let last = (self.buffer.line_count() - 1) as isize;
        let line = (self.cursor.0 as isize + delta).clamp(0, last) as usize;
        self.cursor = (line, self.desired_col.min(self.buffer.line_len(line)));
        self.buffer.seal();
        self.ensure_visible(vp);
    }

    // 単語単位の移動・削除が同じ境界を見るよう、行を跨ぐ判断だけをここに置き
    // 行内の境界計算は word.rs に任せる。戻り値は移動/削除の対象位置
    fn word_right_target(&self) -> Option<(usize, usize)> {
        let (line, col) = self.cursor;
        if col >= self.buffer.line_len(line) {
            // 行末からは次行の先頭へ (改行 1 つを跨ぐ)
            return (line + 1 < self.buffer.line_count()).then_some((line + 1, 0));
        }
        Some((line, word::next_boundary(self.buffer.line(line), col)))
    }

    fn word_left_target(&self) -> Option<(usize, usize)> {
        let (line, col) = self.cursor;
        if col == 0 {
            return (line > 0).then(|| (line - 1, self.buffer.line_len(line - 1)));
        }
        Some((line, word::prev_boundary(self.buffer.line(line), col)))
    }

    fn word_right(&mut self, vp: &mut Viewport) {
        if let Some(target) = self.word_right_target() {
            self.move_to(target, vp);
        }
    }

    fn word_left(&mut self, vp: &mut Viewport) {
        if let Some(target) = self.word_left_target() {
            self.move_to(target, vp);
        }
    }

    /// Alt/Ctrl+Backspace: カーソルの手前 1 単語を消す。行頭では手前の改行を消して行を繋ぐ
    fn delete_word_left(&mut self, vp: &mut Viewport) {
        let Some(target) = self.word_left_target() else {
            return;
        };
        if target == self.cursor {
            return;
        }
        self.buffer.delete(target, self.cursor);
        self.cursor = target;
        self.after_edit(vp);
    }

    /// Alt/Ctrl+Delete: カーソルの先 1 単語を消す。行末では次の改行を消して行を繋ぐ
    fn delete_word_right(&mut self, vp: &mut Viewport) {
        let Some(target) = self.word_right_target() else {
            return;
        };
        if target == self.cursor {
            return;
        }
        self.buffer.delete(self.cursor, target);
        self.after_edit(vp);
    }

    /// Cmd+Backspace / Ctrl+u: 行頭からカーソルまでを消す (改行は跨がない)
    fn delete_to_line_start(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if col == 0 {
            return;
        }
        self.buffer.delete((line, 0), (line, col));
        self.cursor = (line, 0);
        self.after_edit(vp);
    }

    /// Cmd+Delete: カーソルから行末までを消す (改行は跨がない)
    fn delete_to_line_end(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let len = self.buffer.line_len(line);
        if col >= len {
            return;
        }
        self.buffer.delete((line, col), (line, len));
        self.after_edit(vp);
    }

    // Home / Cmd+← / Ctrl+a: インデント直後と行頭を往復する
    fn move_home(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let target = word::home_col(self.buffer.line(line), col);
        self.move_to((line, target), vp);
    }

    fn move_to_end(&mut self, vp: &mut Viewport) {
        let last = self.buffer.line_count() - 1;
        self.move_to((last, self.buffer.line_len(last)), vp);
    }

    /// Alt+↑/↓: カーソル行を隣の行と入れ替える。カーソルは動いた行に付いていく。
    /// 2 行ぶんをまとめて差し替える (EditBuffer::replace) ので undo は 1 回で戻る
    fn move_line(&mut self, delta: isize, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let target = line as isize + delta;
        if target < 0 || target >= self.buffer.line_count() as isize {
            return;
        }
        let target = target as usize;
        let (top, bottom) = (line.min(target), line.max(target));
        let swapped = format!("{}\n{}", self.buffer.line(bottom), self.buffer.line(top));
        self.buffer
            .replace((top, 0), (bottom, self.buffer.line_len(bottom)), &swapped);
        // 行の中身ごと動くので col はそのまま有効
        self.cursor = (target, col);
        self.after_edit(vp);
    }

    fn move_to(&mut self, cursor: (usize, usize), vp: &mut Viewport) {
        self.cursor = cursor;
        self.desired_col = cursor.1;
        self.buffer.seal();
        self.ensure_visible(vp);
    }

    // 編集操作の後始末: 目標列の同期・描画キャッシュの無効化・カーソル追従。
    // ここでハイライトは走らせない (次の描画で可視範囲だけ組み直される)
    fn after_edit(&mut self, vp: &mut Viewport) {
        self.desired_col = self.cursor.1;
        let touched = self.buffer.take_touched();
        if let Some(touched) = touched {
            self.render.invalidate_from(touched);
        }
        self.refresh_changed_lines(touched);
        self.ensure_visible(vp);
    }

    // 保存を待たず、未保存バッファの状態で変更行マークを更新する
    // touched は直前の編集で変わった行 (None = 編集開始時の初回計算)。
    // 触った行の一致だけを見直せば共通範囲が更新できるので、1 打鍵ごとに文書全体を
    // 舐め直さずに済む (component/editor/diff.rs::CommonTrim)
    fn refresh_changed_lines(&mut self, touched: Option<Touched>) {
        let Some(baseline) = &self.baseline else {
            self.changed_lines = None;
            return;
        };
        let current = self.buffer.lines();
        self.trim = match touched {
            Some(touched) => {
                self.trim
                    .after_edit(baseline, current, touched.from, touched.to, touched.shifted)
            }
            None => diff::CommonTrim::default(),
        };
        let (changed, trim) = diff::changed_lines(baseline, current, self.trim);
        self.trim = trim;
        self.changed_lines = Some(changed);
    }

    // カーソルが viewport に収まるよう scroll/hscroll を動かす
    fn ensure_visible(&self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if vp.wrap {
            // wrap 中に水平スクロールは存在しない。縦は視覚行数で収まりを判定する
            vp.hscroll = 0;
            if line < vp.scroll {
                vp.scroll = line;
            }
            let width = self.content_width(vp);
            let display = text::display_col(self.buffer.line(line), col);
            // 折返し位置は描画 (text_pane) と同じ規則で引く。全角文字は境界を跨げないので
            // 「表示桁 / 幅」では視覚行が求まらない
            let (cursor_row, _) = text::wrap_position(self.buffer.line(line), display, width);
            let mut rows = cursor_row + 1;
            for i in vp.scroll..line {
                rows += text::wrap_rows(self.buffer.line(i), width);
            }
            // カーソル行自体が viewport より背が高い場合は先頭合わせが限界 (閲覧時と同じ制約)
            let height = vp.height.max(1);
            while rows > height && vp.scroll < line {
                rows -= text::wrap_rows(self.buffer.line(vp.scroll), width);
                vp.scroll += 1;
            }
            return;
        }
        vp.ensure_row_visible(line);
        let display = text::display_col(self.buffer.line(line), col);
        vp.ensure_col_visible(display, self.content_width(vp));
    }

    // gutter を除いたコンテンツ部の桁数。wrap の折返し幅と hscroll のクランプ幅を兼ねる
    fn content_width(&self, vp: &Viewport) -> usize {
        vp.width.saturating_sub(self.gutter_width()).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    // handle_key を通して確かめるのは、境界計算そのもの (word.rs のテスト) ではなく
    // 「その修飾キーの組み合わせがその操作へ振り分けられるか」を見たいため
    fn session(text: &str) -> (EditState, Viewer) {
        let path = std::env::temp_dir().join(format!(
            "fv-edit-state-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::write(&path, text).unwrap();
        let state = EditState::open(&path, 0, &std::env::temp_dir()).unwrap();
        let _ = fs::remove_file(&path);
        let mut viewer = Viewer::new();
        viewer.viewport.height = 10;
        viewer.viewport.width = 40;
        (state, viewer)
    }

    #[test]
    fn alt_and_ctrl_arrows_move_by_word() {
        let (mut state, mut viewer) = session("let foo.bar = baz;\nsecond\n");
        let alt = KeyModifiers::ALT;
        let ctrl = KeyModifiers::CONTROL;

        state.handle_key(key(KeyCode::Right, alt), &mut viewer);
        assert_eq!(state.cursor, (0, 3));
        state.handle_key(key(KeyCode::Right, alt), &mut viewer);
        assert_eq!(state.cursor, (0, 7));
        // 記号も 1 つの区切りとして止まる (行末まで飛ばない)
        state.handle_key(key(KeyCode::Right, ctrl), &mut viewer);
        assert_eq!(state.cursor, (0, 8));
        state.handle_key(key(KeyCode::Left, alt), &mut viewer);
        assert_eq!(state.cursor, (0, 7));
        // Option を Meta として送る端末向けの別名。大文字で報告する端末も同じに畳む
        state.handle_key(key(KeyCode::Char('b'), alt), &mut viewer);
        assert_eq!(state.cursor, (0, 4));
        state.handle_key(key(KeyCode::Char('F'), alt), &mut viewer);
        assert_eq!(state.cursor, (0, 7));
    }

    #[test]
    fn alt_backspace_deletes_one_word() {
        let (mut state, mut viewer) = session("let foo.bar = baz;\n");
        state.cursor = (0, 7);
        state.handle_key(key(KeyCode::Backspace, KeyModifiers::ALT), &mut viewer);
        assert_eq!(state.buffer.line(0), "let .bar = baz;");
        assert_eq!(state.cursor, (0, 4));
    }

    #[test]
    fn alt_down_swaps_lines_and_carries_the_cursor() {
        let (mut state, mut viewer) = session("one\ntwo\nthree\n");
        state.cursor = (0, 2);
        state.handle_key(key(KeyCode::Down, KeyModifiers::ALT), &mut viewer);
        assert_eq!(state.buffer.lines(), ["two", "one", "three"]);
        assert_eq!(state.cursor, (1, 2));
    }

    #[test]
    fn home_toggles_and_ctrl_home_jumps_to_the_top() {
        let (mut state, mut viewer) = session("fn main() {\n    let x = 1;\n}\n");
        state.cursor = (1, 10);
        state.handle_key(key(KeyCode::Home, KeyModifiers::NONE), &mut viewer);
        assert_eq!(state.cursor, (1, 4));
        state.handle_key(key(KeyCode::Home, KeyModifiers::NONE), &mut viewer);
        assert_eq!(state.cursor, (1, 0));
        state.handle_key(key(KeyCode::End, KeyModifiers::CONTROL), &mut viewer);
        assert_eq!(state.cursor, (2, 1));
        state.handle_key(key(KeyCode::Home, KeyModifiers::CONTROL), &mut viewer);
        assert_eq!(state.cursor, (0, 0));
    }
}
