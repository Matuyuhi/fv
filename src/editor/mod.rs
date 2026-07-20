mod buffer;
mod diff;

pub use buffer::EditBuffer;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::Line;

use crate::git;
use crate::text;
use crate::viewer::{Highlighter, Viewer, Viewport};

/// これを超えるファイルは編集対象にしない (メモリ・再ハイライトの両面で現実的でないため)
const MAX_EDIT_BYTES: u64 = 10 * 1024 * 1024;

pub enum EditOutcome {
    Continue,
    Exit,
}

/// インライン編集の状態。閲覧側とは「Viewport (スクロール共有) と Highlighter
/// (再ハイライト) だけを借りる」関係に留め、Viewer の cache・履歴・検索には触らない。
/// 保存 (save) だけは cache の即時更新のため Viewer::reload を呼ぶ
pub struct EditState {
    pub path: PathBuf,
    pub buffer: EditBuffer,
    /// (line, col)。バッファの生テキスト上の char 座標 (タブは 1 char)
    pub cursor: (usize, usize),
    // 上下移動で維持する目標列。短い行を跨いでも元の列に戻れるようにする (vim 相当)
    desired_col: usize,
    /// 描画キャッシュ。編集操作の度に再生成し、カーソル移動だけでは触らない
    pub lines: Vec<Line<'static>>,
    /// 行番号 gutter の char 幅 (末尾空白込み)。マウス座標変換とカーソル追従が参照する
    pub gutter_width: usize,
    /// 保存エラー・discard 確認などステータスバーに出す一時メッセージ
    pub notice: Option<String>,
    confirm_discard: bool,
    // ライブ diff の比較元 (編集開始時の HEAD / index 版)。repo 外・untracked は None
    baseline: Option<Vec<String>>,
    /// 未保存バッファ vs baseline の変更行 (1-origin)。viewer の changed_lines と同じ描画に使う
    pub changed_lines: Option<HashSet<usize>>,
}

impl EditState {
    /// 編集セッションを開始する。非 UTF-8・巨大ファイル・読込失敗は None (呼び出し側で no-op)
    pub fn open(
        path: &Path,
        highlighter: &Highlighter,
        start_line: usize,
        root: &Path,
    ) -> Option<Self> {
        let size = fs::metadata(path).ok()?.len();
        if size > MAX_EDIT_BYTES {
            return None;
        }
        let buffer = EditBuffer::load(path).ok()?;
        let cursor_line = start_line.min(buffer.line_count() - 1);
        let mut state = Self {
            path: path.to_path_buf(),
            buffer,
            cursor: (cursor_line, 0),
            desired_col: 0,
            lines: Vec::new(),
            gutter_width: 0,
            notice: None,
            confirm_discard: false,
            baseline: git::baseline_lines(root, path),
            changed_lines: None,
        };
        state.rebuild(highlighter);
        Some(state)
    }

    pub fn handle_key(&mut self, key: KeyEvent, viewer: &mut Viewer) -> EditOutcome {
        let mods = key.modifiers;
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        // SUPER (mac の Cmd) は kitty keyboard protocol 対応端末でのみ届く (main.rs で opt-in)
        let cmd = mods.contains(KeyModifiers::SUPER);
        let shift = mods.contains(KeyModifiers::SHIFT);
        // 修飾付き文字は端末により大文字 (Shift 畳み込み済み) で届くことがあるため小文字に揃える
        let code = match key.code {
            KeyCode::Char(c) if ctrl || cmd => KeyCode::Char(c.to_ascii_lowercase()),
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
        // 端末により word 移動は Ctrl+矢印 / Alt+矢印 のどちらでも届くため両方受ける
        let word = mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        // 保存だけは cache 再読込のため Viewer 全体が要る。先に処理して抜けることで、
        // 以降の操作は highlighter/viewport の 2 フィールドしか借りないことを型で保証する
        if code == KeyCode::Char('s') && (ctrl || cmd) {
            self.save(viewer);
            return EditOutcome::Continue;
        }
        let hl = &viewer.highlighter;
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
                    self.after_edit(hl, vp);
                }
            }
            KeyCode::Char('z') if ctrl || cmd => {
                if let Some(cursor) = self.buffer.undo() {
                    self.cursor = cursor;
                    self.after_edit(hl, vp);
                }
            }
            KeyCode::Char('y') if ctrl || cmd => {
                if let Some(cursor) = self.buffer.redo() {
                    self.cursor = cursor;
                    self.after_edit(hl, vp);
                }
            }
            KeyCode::Char('k') if ctrl => self.delete_line(hl, vp),
            KeyCode::Enter => {
                self.cursor = self.buffer.insert_block(self.cursor, "\n");
                self.after_edit(hl, vp);
            }
            KeyCode::Backspace => self.backspace(hl, vp),
            KeyCode::Delete => self.delete_forward(hl, vp),
            KeyCode::Tab => {
                self.cursor = self.buffer.insert_typed(self.cursor, '\t');
                self.after_edit(hl, vp);
            }
            // mac 慣習: Cmd+←/→ は行頭・行末
            KeyCode::Left if cmd => self.move_to((self.cursor.0, 0), vp),
            KeyCode::Right if cmd => {
                self.move_to((self.cursor.0, self.buffer.line_len(self.cursor.0)), vp)
            }
            KeyCode::Left if word => self.word_left(vp),
            KeyCode::Right if word => self.word_right(vp),
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
            KeyCode::Home => self.move_to((self.cursor.0, 0), vp),
            KeyCode::End => self.move_to((self.cursor.0, self.buffer.line_len(self.cursor.0)), vp),
            // Cmd/Alt 付きは未割当ショートカットの可能性が高いので文字として挿入しない
            KeyCode::Char(c) if !ctrl && !cmd && !mods.contains(KeyModifiers::ALT) => {
                self.cursor = self.buffer.insert_typed(self.cursor, c);
                self.after_edit(hl, vp);
            }
            _ => {}
        }
        EditOutcome::Continue
    }

    /// bracketed paste の一括挿入。undo 1 単位・再ハイライト 1 回に畳む
    pub fn paste(&mut self, text: &str, highlighter: &Highlighter, viewport: &mut Viewport) {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.cursor = self.buffer.insert_block(self.cursor, &text);
        self.after_edit(highlighter, viewport);
    }

    /// マウスクリック。row/col はコンテンツ領域 (枠線の内側) 相対の画面座標
    pub fn click_at(&mut self, row: usize, col: usize, vp: &Viewport) {
        let (line, display) = if vp.wrap {
            // 描画 (ui/text_pane) と同じ視覚行数の計算で、クリック行が
            // どの論理行の何段目かを scroll から辿って特定する
            let width = self.content_width(vp);
            let mut line = vp.scroll.min(self.buffer.line_count() - 1);
            let mut remaining = row;
            loop {
                let rows = text::wrap_rows(self.display_len(line), width);
                if remaining < rows || line + 1 >= self.buffer.line_count() {
                    remaining = remaining.min(rows - 1);
                    break;
                }
                remaining -= rows;
                line += 1;
            }
            (
                line,
                remaining * width + col.saturating_sub(self.gutter_width),
            )
        } else {
            (
                vp.scroll + row,
                vp.hscroll + col.saturating_sub(self.gutter_width),
            )
        };
        let line = line.min(self.buffer.line_count() - 1);
        let col = text::char_col_at(self.buffer.line(line), display);
        self.cursor = (line, col);
        self.desired_col = col;
        self.buffer.seal();
    }

    fn save(&mut self, viewer: &mut Viewer) {
        match fs::write(&self.path, self.buffer.to_text()) {
            Ok(()) => {
                self.buffer.mark_saved();
                // cache と git 変更行マークを watcher を待たずに即時更新する
                viewer.reload(&self.path);
                // reload は hscroll を 0 に戻すため、カーソル位置まで追従し直す
                self.ensure_visible(&mut viewer.viewport);
                self.notice = Some("saved".to_string());
            }
            Err(e) => self.notice = Some(format!("save failed: {e}")),
        }
    }

    fn backspace(&mut self, hl: &Highlighter, vp: &mut Viewport) {
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
        self.after_edit(hl, vp);
    }

    fn delete_forward(&mut self, hl: &Highlighter, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if col < self.buffer.line_len(line) {
            self.buffer.delete((line, col), (line, col + 1));
        } else if line + 1 < self.buffer.line_count() {
            self.buffer.delete((line, col), (line + 1, 0));
        } else {
            return;
        }
        self.after_edit(hl, vp);
    }

    /// Ctrl+k: カーソル行を丸ごと削除。最終行は内容だけ消す (バッファは常に 1 行以上を保つ)
    fn delete_line(&mut self, hl: &Highlighter, vp: &mut Viewport) {
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
        self.after_edit(hl, vp);
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

    /// WORD (非空白の連なり) 単位で次の語頭へ。行末からは次行頭へ
    fn word_right(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        let chars: Vec<char> = self.buffer.line(line).chars().collect();
        if col >= chars.len() {
            if line + 1 < self.buffer.line_count() {
                self.move_to((line + 1, 0), vp);
            }
            return;
        }
        let mut c = col;
        while c < chars.len() && !chars[c].is_whitespace() {
            c += 1;
        }
        while c < chars.len() && chars[c].is_whitespace() {
            c += 1;
        }
        self.move_to((line, c), vp);
    }

    fn word_left(&mut self, vp: &mut Viewport) {
        let (line, col) = self.cursor;
        if col == 0 {
            if line > 0 {
                self.move_to((line - 1, self.buffer.line_len(line - 1)), vp);
            }
            return;
        }
        let chars: Vec<char> = self.buffer.line(line).chars().collect();
        let mut c = col;
        while c > 0 && chars[c - 1].is_whitespace() {
            c -= 1;
        }
        while c > 0 && !chars[c - 1].is_whitespace() {
            c -= 1;
        }
        self.move_to((line, c), vp);
    }

    fn move_to(&mut self, cursor: (usize, usize), vp: &mut Viewport) {
        self.cursor = cursor;
        self.desired_col = cursor.1;
        self.buffer.seal();
        self.ensure_visible(vp);
    }

    // 編集操作の後始末: 目標列の同期・描画キャッシュ再生成・カーソル追従
    fn after_edit(&mut self, hl: &Highlighter, vp: &mut Viewport) {
        self.desired_col = self.cursor.1;
        self.rebuild(hl);
        self.ensure_visible(vp);
    }

    fn rebuild(&mut self, hl: &Highlighter) {
        self.lines = hl.highlight_text(&self.path, &self.buffer.display_text());
        self.gutter_width = text::gutter_width(self.buffer.line_count());
        // 保存を待たず、未保存バッファの状態で変更行マークを更新する
        self.changed_lines = self
            .baseline
            .as_ref()
            .map(|baseline| diff::changed_lines(baseline, self.buffer.lines()));
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
            let cursor_row = text::display_col(self.buffer.line(line), col) / width;
            let mut rows = cursor_row + 1;
            for i in vp.scroll..line {
                rows += text::wrap_rows(self.display_len(i), width);
            }
            // カーソル行自体が viewport より背が高い場合は先頭合わせが限界 (閲覧時と同じ制約)
            let height = vp.height.max(1);
            while rows > height && vp.scroll < line {
                rows -= text::wrap_rows(self.display_len(vp.scroll), width);
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
        vp.width.saturating_sub(self.gutter_width).max(1)
    }

    fn display_len(&self, line: usize) -> usize {
        text::display_col(self.buffer.line(line), self.buffer.line_len(line))
    }
}
