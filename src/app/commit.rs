//! コミットメッセージ入力オーバーレイ (`c` / `C`、Mode::Commit) の開閉・編集・実行。
//! Mode::Input は 1 行入力専用で複数行のメッセージを表現できないため独立したモードにしてある。
//! カーソルはバイトではなく char インデックスで扱う (日本語等でずれないため)。

use crossterm::event::{KeyCode, KeyEvent};

use crate::git;

use super::{App, ConfirmAction, Lane, Mode};

impl App {
    /// c / C: コミットオーバーレイを開く。GIT レーンに限定しない (変更を見て回ってから
    /// そのままコミットしたい時に、わざわざ Shift+Tab で GIT へ切り替えさせたくないため)。
    /// 使えない文脈 (repo 外・staged が空) は開かず notice で理由を出す
    pub(super) fn open_commit(&mut self, amend: bool) {
        if self.git.is_none() {
            return;
        }
        // 型上ここへは実際には来ない (Lane::Edit は印字キーを全て文字入力にするため 'c' は
        // ここまで届かない) が、issue の要求通り明示的にガードしておく
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                "未保存の変更があります。保存してからコミットしてください".to_string(),
                true,
            );
            return;
        }
        if amend {
            let buffer = self
                .amend_draft
                .take()
                .or_else(|| git::last_commit_message(&self.root))
                .unwrap_or_default();
            let cursor = buffer.chars().count();
            self.mode = Mode::Commit {
                buffer,
                cursor,
                amend: true,
                error: None,
            };
            return;
        }
        if !self.has_staged_changes() {
            self.set_notice(
                "ステージされた変更がありません (Space でステージ)".to_string(),
                true,
            );
            return;
        }
        let buffer = self.commit_draft.take().unwrap_or_default();
        let cursor = buffer.chars().count();
        self.mode = Mode::Commit {
            buffer,
            cursor,
            amend: false,
            error: None,
        };
    }

    // amend は staged が空でも許可する (issue の要求: メッセージ修正の用途) が、通常コミットは
    // index 側に何か 1 つでも乗っていないと開かせない (--allow-empty は使わない方針のため)
    fn has_staged_changes(&self) -> bool {
        self.git
            .as_ref()
            .is_some_and(|status| status.files.values().any(|f| f.index.is_some()))
    }

    pub(super) fn on_commit_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => self.close_commit(),
            KeyCode::Char('s') if ctrl => self.submit_commit(),
            KeyCode::Enter => self.commit_insert('\n'),
            KeyCode::Backspace => self.commit_backspace(),
            KeyCode::Left => self.commit_move_char(-1),
            KeyCode::Right => self.commit_move_char(1),
            KeyCode::Up => self.commit_move_line(-1),
            KeyCode::Down => self.commit_move_line(1),
            KeyCode::Home => self.commit_move_home(),
            KeyCode::End => self.commit_move_end(),
            KeyCode::Char(c) if !ctrl => self.commit_insert(c),
            _ => {}
        }
    }

    // Esc は内容を破棄しない。amend/通常で保存先を分けるのは、次に C を押した時に
    // 前回の amend 編集を (git log の再フェッチではなく) そのまま復元するため
    fn close_commit(&mut self) {
        let Mode::Commit { buffer, amend, .. } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return;
        };
        if amend {
            self.amend_draft = Some(buffer);
        } else {
            self.commit_draft = Some(buffer);
        }
    }

    fn commit_insert(&mut self, c: char) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let idx = char_byte_index(buffer, *cursor);
        buffer.insert(idx, c);
        *cursor += 1;
    }

    fn commit_backspace(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        if *cursor == 0 {
            return;
        }
        let idx = char_byte_index(buffer, *cursor - 1);
        buffer.remove(idx);
        *cursor -= 1;
    }

    fn commit_move_char(&mut self, delta: isize) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let len = buffer.chars().count() as isize;
        *cursor = (*cursor as isize + delta).clamp(0, len) as usize;
    }

    // Up/Down は同じ桁位置を保とうとし、行が短ければ行末に丸める (一般的なエディタと同じ挙動)
    fn commit_move_line(&mut self, delta: isize) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, col) = line_col(buffer, *cursor);
        let target_line = (line as isize + delta).clamp(0, lines.len() as isize - 1) as usize;
        let target_col = col.min(lines[target_line].chars().count());
        *cursor = char_index_of(&lines, target_line, target_col);
    }

    fn commit_move_home(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, _) = line_col(buffer, *cursor);
        *cursor = char_index_of(&lines, line, 0);
    }

    fn commit_move_end(&mut self) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let lines: Vec<&str> = buffer.split('\n').collect();
        let (line, _) = line_col(buffer, *cursor);
        let end = lines[line].chars().count();
        *cursor = char_index_of(&lines, line, end);
    }

    // bracketed paste (App::on_paste から呼ぶ)。改行以外の制御文字は落とす (Input/Finder と同じ方針)
    pub(super) fn commit_paste(&mut self, text: &str) {
        let Mode::Commit { buffer, cursor, .. } = &mut self.mode else {
            return;
        };
        let idx = char_byte_index(buffer, *cursor);
        let filtered: String = text
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        let inserted = filtered.chars().count();
        buffer.insert_str(idx, &filtered);
        *cursor += inserted;
    }

    // Ctrl+s: amend は履歴を書き換える (push 済みの可能性がある) ので確認オーバーレイを経由させ、
    // 通常コミットは直接実行する
    fn submit_commit(&mut self) {
        let Mode::Commit { buffer, amend, .. } = &self.mode else {
            return;
        };
        let message = buffer.clone();
        let amend = *amend;
        if amend {
            // 確認をキャンセルしても下書きが残るよう、実行前に退避しておく
            self.amend_draft = Some(message.clone());
            let subject = message.lines().next().unwrap_or("").to_string();
            self.mode = Mode::Confirm {
                prompt: format!("amend commit: 「{subject}」"),
                action: ConfirmAction::Amend { message },
            };
            return;
        }
        self.perform_commit(&message, false);
    }

    // git::commit の実行と結果反映。amend は確認オーバーレイ (run_confirm_action) から、
    // 通常コミットは Mode::Commit のまま (submit_commit) から呼ばれる
    pub(super) fn perform_commit(&mut self, message: &str, amend: bool) {
        let outcome = git::commit(&self.root, message, amend);
        if outcome.ok {
            self.mode = Mode::Normal;
            if amend {
                self.amend_draft = None;
            } else {
                self.commit_draft = None;
            }
            self.set_notice(outcome.message, false);
            // stage/unstage と同じ入口に相乗りさせる (ツリーの変更ファイル一覧・diff を揃える)
            self.rescan_now();
        } else if let Mode::Commit { error, .. } = &mut self.mode {
            // 通常コミット (Mode::Commit のまま) の失敗はオーバーレイ内にエラーを出し、
            // 書きかけのメッセージを保ったまま再試行できるようにする
            *error = Some(outcome.message);
        } else {
            // amend は確認オーバーレイ経由で mode が既に Normal に戻っているため、
            // 下書き (amend_draft) はここまでの経路で保持済み。notice で失敗を出す
            self.set_notice(outcome.message, true);
        }
    }
}

// commit オーバーレイの改行込みバッファは char インデックスで扱う (バイトインデックスだと
// 日本語等の複数バイト文字でカーソル位置がずれるため)

fn char_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

fn line_col(s: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 0usize;
    let mut col = 0usize;
    for (i, ch) in s.chars().enumerate() {
        if i == char_idx {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

// lines ([&str]、buffer.split('\n') の結果) 上の (line, col) を buffer 全体の char インデックスへ戻す
fn char_index_of(lines: &[&str], line: usize, col: usize) -> usize {
    let mut idx = 0;
    for l in &lines[..line] {
        idx += l.chars().count() + 1; // +1 は行を繋いでいた '\n' の分
    }
    idx + col
}
