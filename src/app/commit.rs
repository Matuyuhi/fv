//! コミットメッセージ入力オーバーレイ (`c` / `C`、Mode::Commit) の開閉・編集・実行。
//! Mode::Input は 1 行入力専用でこの形を表現できないため独立したモードにしてある。
//! カーソルはバイトではなく char インデックスで扱う (日本語等でずれないため)。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::git;

use super::{App, ConfirmAction, Lane, Mode};

/// 編集中のどちらの入力欄にいるか
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum CommitField {
    #[default]
    Subject,
    Body,
}

/// 件名と本文の区切り。新規入力はこの形 (git の慣習どおり空行 1 つ) で組み立てる
const DEFAULT_SEPARATOR: &str = "\n\n";

/// 入力中のコミットメッセージ。件名と本文を別の欄として持つ。件名側は改行を受け取らない
/// (Enter は本文欄への移動にあてる) ので「件名が複数行」という状態を作れない
#[derive(Clone)]
pub struct CommitDraft {
    pub subject: String,
    pub body: String,
    pub field: CommitField,
    /// 欄ごとに持つ char インデックス。行き来しても書きかけの位置を失わない
    pub subject_cursor: usize,
    pub body_cursor: usize,
    /// 件名と本文を繋ぎ直すときの改行列。amend で開いた元メッセージが慣習どおりでない
    /// ("件名\n本文" のように空行が無い) 場合でも、開いて保存しただけで形が変わらないよう
    /// 元の形をそのまま持ち回る
    separator: String,
}

impl Default for CommitDraft {
    fn default() -> Self {
        Self {
            subject: String::new(),
            body: String::new(),
            field: CommitField::default(),
            subject_cursor: 0,
            body_cursor: 0,
            separator: DEFAULT_SEPARATOR.to_string(),
        }
    }
}

impl CommitDraft {
    /// git へ渡す形。本文が空なら件名だけにする (末尾に空行を残さない)
    fn message(&self) -> String {
        if self.body.trim().is_empty() {
            self.subject.clone()
        } else {
            format!("{}{}{}", self.subject, self.separator, self.body)
        }
    }

    /// amend のプリフィル用。1 行目が件名、それ以降が本文。間の改行は数ごと保って
    /// from_message → message のラウンドトリップで元のメッセージに戻るようにする
    fn from_message(text: &str) -> Self {
        let Some((first, rest)) = text.split_once('\n') else {
            return Self {
                subject_cursor: text.chars().count(),
                subject: text.to_string(),
                ..Self::default()
            };
        };
        let body = rest.trim_start_matches('\n');
        // '\n' は 1 バイトなので、削れた長さがそのまま改行の個数になる
        let separator = "\n".repeat(rest.len() - body.len() + 1);
        Self {
            subject_cursor: first.chars().count(),
            body_cursor: body.chars().count(),
            subject: first.to_string(),
            body: body.to_string(),
            field: CommitField::Subject,
            separator,
        }
    }

    /// 編集対象の欄。挿入・削除・カーソル移動はここ 1 箇所を通すので、欄が増えても
    /// 操作側は field を意識しなくて済む
    fn active_mut(&mut self) -> (&mut String, &mut usize) {
        match self.field {
            CommitField::Subject => (&mut self.subject, &mut self.subject_cursor),
            CommitField::Body => (&mut self.body, &mut self.body_cursor),
        }
    }
}

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
            // 下書きがあれば書きかけの位置ごと戻す。無ければ直前のコミットからプリフィルする
            let draft = self
                .amend_draft
                .take()
                .or_else(|| {
                    Some(CommitDraft::from_message(&git::last_commit_message(
                        &self.root,
                    )?))
                })
                .unwrap_or_default();
            self.mode = Mode::Commit {
                draft,
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
        self.mode = Mode::Commit {
            draft: self.commit_draft.take().unwrap_or_default(),
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
        // SUPER (mac の Cmd) は kitty keyboard protocol 対応端末でのみ届く。修飾付き文字は
        // 端末により大文字で来るので小文字へ畳んでから判定する (component/editor と同じ作法)
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let code = match key.code {
            KeyCode::Char(c) if ctrl || cmd => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        match code {
            KeyCode::Esc => self.close_commit(),
            KeyCode::Char('s') if ctrl || cmd => self.submit_commit(),
            KeyCode::Tab | KeyCode::BackTab => self.commit_switch_field(),
            KeyCode::Enter => self.commit_enter(),
            KeyCode::Backspace => self.commit_backspace(),
            // mac 慣習: Cmd+←/→ は行頭・行末
            KeyCode::Left if cmd => self.commit_move_home(),
            KeyCode::Right if cmd => self.commit_move_end(),
            KeyCode::Left => self.commit_move_char(-1),
            KeyCode::Right => self.commit_move_char(1),
            KeyCode::Up => self.commit_move_line(-1),
            KeyCode::Down => self.commit_move_line(1),
            KeyCode::Home => self.commit_move_home(),
            KeyCode::End => self.commit_move_end(),
            // Cmd/Alt 付きは未割当ショートカットの可能性が高いので文字として挿入しない
            KeyCode::Char(c) if !ctrl && !cmd && !alt => self.commit_insert(c),
            _ => {}
        }
    }

    // Esc は内容を破棄しない。amend/通常で保存先を分けるのは、次に C を押した時に
    // 前回の amend 編集を (git log の再フェッチではなく) そのまま復元するため
    fn close_commit(&mut self) {
        let Mode::Commit { draft, amend, .. } = std::mem::replace(&mut self.mode, Mode::Normal)
        else {
            return;
        };
        if amend {
            self.amend_draft = Some(draft);
        } else {
            self.commit_draft = Some(draft);
        }
    }

    fn commit_draft_mut(&mut self) -> Option<&mut CommitDraft> {
        match &mut self.mode {
            Mode::Commit { draft, .. } => Some(draft),
            _ => None,
        }
    }

    fn commit_switch_field(&mut self) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        draft.field = match draft.field {
            CommitField::Subject => CommitField::Body,
            CommitField::Body => CommitField::Subject,
        };
    }

    // 件名は 1 行に保つ (git のメッセージ規約と同じ形) ので、件名欄の Enter は改行ではなく
    // 本文欄への移動にあてる
    fn commit_enter(&mut self) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        match draft.field {
            CommitField::Subject => draft.field = CommitField::Body,
            CommitField::Body => self.commit_insert('\n'),
        }
    }

    fn commit_insert(&mut self, c: char) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let (text, cursor) = draft.active_mut();
        let idx = char_byte_index(text, *cursor);
        text.insert(idx, c);
        *cursor += 1;
    }

    fn commit_backspace(&mut self) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let (text, cursor) = draft.active_mut();
        if *cursor == 0 {
            return;
        }
        let idx = char_byte_index(text, *cursor - 1);
        text.remove(idx);
        *cursor -= 1;
    }

    fn commit_move_char(&mut self, delta: isize) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let (text, cursor) = draft.active_mut();
        let len = text.chars().count() as isize;
        *cursor = (*cursor as isize + delta).clamp(0, len) as usize;
    }

    // Up/Down は同じ桁位置を保とうとし、行が短ければ行末に丸める (一般的なエディタと同じ挙動)。
    // 欄の端を越える移動は隣の欄へ渡す (件名で Down / 本文の 1 行目で Up)
    fn commit_move_line(&mut self, delta: isize) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        match (draft.field, delta) {
            (CommitField::Subject, 1) => {
                draft.field = CommitField::Body;
                return;
            }
            (CommitField::Body, -1) if line_col(&draft.body, draft.body_cursor).0 == 0 => {
                draft.field = CommitField::Subject;
                return;
            }
            _ => {}
        }
        let (text, cursor) = draft.active_mut();
        let lines: Vec<&str> = text.split('\n').collect();
        let (line, col) = line_col(text, *cursor);
        let target_line = (line as isize + delta).clamp(0, lines.len() as isize - 1) as usize;
        let target_col = col.min(lines[target_line].chars().count());
        *cursor = char_index_of(&lines, target_line, target_col);
    }

    fn commit_move_home(&mut self) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let (text, cursor) = draft.active_mut();
        let lines: Vec<&str> = text.split('\n').collect();
        let (line, _) = line_col(text, *cursor);
        *cursor = char_index_of(&lines, line, 0);
    }

    fn commit_move_end(&mut self) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let (text, cursor) = draft.active_mut();
        let lines: Vec<&str> = text.split('\n').collect();
        let (line, _) = line_col(text, *cursor);
        let end = lines[line].chars().count();
        *cursor = char_index_of(&lines, line, end);
    }

    // bracketed paste (App::on_paste から呼ぶ)。改行以外の制御文字は落とす (Input/Finder と同じ方針)。
    // 件名欄は 1 行なので改行も空白へ潰す
    pub(super) fn commit_paste(&mut self, text: &str) {
        let Some(draft) = self.commit_draft_mut() else {
            return;
        };
        let multiline = draft.field == CommitField::Body;
        let filtered: String = text
            .chars()
            .filter_map(|c| match c {
                '\n' if multiline => Some('\n'),
                '\n' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect();
        let (text, cursor) = draft.active_mut();
        let idx = char_byte_index(text, *cursor);
        let inserted = filtered.chars().count();
        text.insert_str(idx, &filtered);
        *cursor += inserted;
    }

    // Ctrl+s: amend は履歴を書き換える (push 済みの可能性がある) ので確認オーバーレイを経由させ、
    // 通常コミットは直接実行する
    fn submit_commit(&mut self) {
        let Mode::Commit { draft, amend, .. } = &self.mode else {
            return;
        };
        let message = draft.message();
        let subject = draft.subject.clone();
        let amend = *amend;
        if amend {
            // 確認をキャンセルしても下書きが残るよう、実行前に退避しておく
            self.amend_draft = self.commit_draft_mut().map(|d| d.clone());
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

// commit オーバーレイの入力欄は char インデックスで扱う (バイトインデックスだと
// 日本語等の複数バイト文字でカーソル位置がずれるため)

fn char_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

pub(super) fn line_col(s: &str, char_idx: usize) -> (usize, usize) {
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

// lines ([&str]、text.split('\n') の結果) 上の (line, col) を欄全体の char インデックスへ戻す
fn char_index_of(lines: &[&str], line: usize, col: usize) -> usize {
    let mut idx = 0;
    for l in &lines[..line] {
        idx += l.chars().count() + 1; // +1 は行を繋いでいた '\n' の分
    }
    idx + col
}
