//! ブランチ一覧オーバーレイ (`b`、Mode::Branch) のキー処理と切替・作成の実行。
//! 一覧の絞り込み・選択状態そのものは component/branch/mod.rs (BranchState) が持ち、ここは
//! 「どの git コマンドを呼び、結果をどう表示へ反映するか」だけを持つ。

use crossterm::event::{KeyCode, KeyEvent};

use crate::component::branch::BranchState;
use crate::git;
use crate::lang::{Msg, t};

use super::{App, Lane, Mode};

impl App {
    /// b: ブランチ一覧オーバーレイを開く。使えない文脈 (非 git repo) は開かず no-op
    pub(super) fn open_branch(&mut self) {
        if !self.branch_available() {
            return;
        }
        // 型上ここへは実際には来ない (Lane::Edit は印字キーを全て文字入力にするため 'b' は
        // ここまで届かない) が、open_commit と同じく issue の要求通り明示的にガードしておく
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                t(Msg::BranchUnsavedChangesSaveBeforeSwitching).to_string(),
                true,
            );
            return;
        }
        let current = self
            .branch_status
            .as_ref()
            .filter(|s| !s.detached)
            .map(|s| s.name.as_str());
        self.mode = Mode::Branch(BranchState::new(&self.root, current));
    }

    pub(super) fn on_branch_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                return;
            }
            KeyCode::Enter => {
                self.checkout_selected_branch();
                return;
            }
            KeyCode::Char('n') if ctrl => {
                self.create_new_branch();
                return;
            }
            _ => {}
        }
        let Mode::Branch(state) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Backspace => state.backspace(),
            KeyCode::Down => state.move_selection(1),
            KeyCode::Up => state.move_selection(-1),
            KeyCode::Char('p') if ctrl => state.move_selection(-1),
            // ctrl 付きの印字キー (Ctrl+n/p 以外) はクエリに積まない (Finder と同じ方針)
            KeyCode::Char(c) if !ctrl => state.push_char(c),
            _ => {}
        }
    }

    // Enter: 選択中のブランチへ切り替える。remote 由来なら `switch --track` 相当で
    // ローカル追跡ブランチを作りつつ切り替える
    fn checkout_selected_branch(&mut self) {
        let Mode::Branch(state) = &self.mode else {
            return;
        };
        let Some(row) = state.selected_row() else {
            return;
        };
        let target = row.entry.name.clone();
        let remote = row.entry.remote;
        let outcome = if remote {
            git::switch_track_branch(&self.root, &target)
        } else {
            git::switch_branch(&self.root, &target)
        };
        self.finish_branch_action(outcome);
    }

    // Ctrl+n: 入力文字列が既存のローカルブランチと一致しない間だけ新規作成する。
    // 一致する間は誤って上書きしないよう何もせず notice で理由を示し、オーバーレイは開いたままにする
    fn create_new_branch(&mut self) {
        let Mode::Branch(state) = &self.mode else {
            return;
        };
        if state.query.is_empty() {
            return;
        }
        if state.matches_existing_local() {
            let name = state.query.clone();
            self.set_notice(crate::tr!(Msg::BranchAlreadyExists, name), true);
            return;
        }
        let name = state.query.clone();
        let outcome = git::create_branch(&self.root, &name);
        self.finish_branch_action(outcome);
    }

    // checkout/create 共通の後処理。未保存バッファの拒否もここに寄せず呼び出し元で先に弾く
    // (dirty チェックは open_branch 側にあり、型上ここへは既に届かない状態でしか呼ばれない)
    fn finish_branch_action(&mut self, outcome: git::GitOutcome) {
        self.mode = Mode::Normal;
        if !outcome.ok {
            let message = if outcome.message.is_empty() {
                t(Msg::BranchFailedRunGit).to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
            return;
        }
        // 切替先に開いていたファイルが無ければ右ペインを空にする (issue の提案通り)
        let stale = self
            .viewer
            .current
            .as_ref()
            .is_some_and(|open| !open.path.exists());
        if stale {
            self.viewer.close();
        }
        // stage/unstage・commit と同じ入口に相乗りさせる (ツリー・git status・branch_status を揃える)
        self.rescan_now();
        let branch = self
            .branch_status
            .as_ref()
            .map(|s| s.name.as_str())
            .unwrap_or("?");
        let message = if stale {
            crate::tr!(Msg::BranchSwitchedStale, branch)
        } else {
            crate::tr!(Msg::BranchSwitched, branch)
        };
        self.set_notice(message, false);
    }
}
