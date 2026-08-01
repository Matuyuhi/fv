//! git の書き込み系操作 (stage/unstage・discard・stash・fetch/pull/push) の実行と、
//! 実行後に表示を揃えるための後始末。破棄・stash・push は Mode::Confirm を挟むため、
//! 「確認を出す confirm_*」と「y/Enter 確定後に走る execute_*」を対で並べてある。

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::git;

use super::{App, ConfirmAction, Lane, Mode};

impl App {
    /// Space: 選択中のファイル/ディレクトリを stage/unstage トグルする。判定は
    /// 「worktree 側に未ステージ変更が残っているか」で、残っていれば stage、無ければ unstage
    /// (issue #23 の要求通り)。ディレクトリは配下の files を集約して同じ判定に使う
    pub(super) fn toggle_stage_selected(&mut self) {
        // キーリピート対策。debounce 中の呼び出しは git プロセスを起動せずに捨てる
        if self.last_stage_toggle.elapsed() < super::STAGE_DEBOUNCE {
            return;
        }
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let Some(status) = &self.git else {
            return;
        };
        let (has_worktree_change, has_deletion) = if is_dir {
            let mut worktree = false;
            let mut deletion = false;
            for (p, s) in &status.files {
                if !p.starts_with(&path) {
                    continue;
                }
                worktree |= s.worktree.is_some();
                deletion |= s.worktree == Some(git::StatusKind::Deleted)
                    || s.index == Some(git::StatusKind::Deleted);
            }
            (worktree, deletion)
        } else {
            let Some(s) = status.files.get(&path) else {
                // フィルタに乗っているのに status が無いのは通常起こらない (rescan 直後の
                // ズレなど)。何もせず次の rescan を待つ
                return;
            };
            (
                s.worktree.is_some(),
                s.worktree == Some(git::StatusKind::Deleted)
                    || s.index == Some(git::StatusKind::Deleted),
            )
        };
        self.last_stage_toggle = Instant::now();
        let outcome = if has_worktree_change {
            git::stage_path(&self.root, &path, has_deletion)
        } else {
            git::unstage_path(&self.root, &path, is_dir)
        };
        if outcome.ok {
            // rescan は選択位置を path ベースで維持する (Tree::rescan/set_filter の既存の
            // 復元経路をそのまま使う)。ツリーの XY 表示・絞り込み・diff の全てがここで揃う
            self.rescan_now();
        } else {
            let message = if outcome.message.is_empty() {
                "git の実行に失敗しました".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    // 未保存の編集バッファがある間は破棄・stash を実行させない (ディスクを書き換えると
    // 編集内容と食い違うため)。X/z/Z は Lane::Git 限定でしか到達せず、Lane は同時に
    // 一つしか無いため現行のキー経路では実質 true にならないが、issue #25 の安全側の
    // 作法として明示的に防御しておく (belt and suspenders)
    fn refuse_if_edit_dirty(&mut self) -> bool {
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            self.set_notice(
                "未保存の変更があります (保存または破棄してから実行してください)",
                true,
            );
            return true;
        }
        false
    }

    // 対象ファイル数と untracked を含むかどうかを集計する。confirm 時の prompt 生成と
    // execute 時の実行対象決定の両方から呼ぶ共通ロジック (件数がずれると事故るため 1 箇所にする)
    fn discard_summary(&self, path: &Path, is_dir: bool) -> Option<(usize, bool)> {
        let status = self.git.as_ref()?;
        let mut count = 0usize;
        let mut has_untracked = false;
        for (p, s) in &status.files {
            let hit = if is_dir {
                p.starts_with(path)
            } else {
                p == path
            };
            if !hit {
                continue;
            }
            count += 1;
            if s.index == Some(git::StatusKind::Untracked) {
                has_untracked = true;
            }
        }
        if count == 0 {
            None
        } else {
            Some((count, has_untracked))
        }
    }

    /// X: 選択ファイル/ディレクトリの変更破棄を確認オーバーレイに乗せる。実行そのものは
    /// execute_discard (y/Enter 確定後) が行う
    pub(super) fn confirm_discard(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let Some(row) = self.tree.visible.get(self.tree.selected) else {
            return;
        };
        let path = row.path.clone();
        let is_dir = row.is_dir;
        let Some((count, has_untracked)) = self.discard_summary(&path, is_dir) else {
            return;
        };
        // 絶対パスは確認枠をはみ出して肝心のファイル名が読めなくなるので repo 相対で出す
        // (GIT ペインのタイトルと同じ扱い)
        let shown = path.strip_prefix(&self.root).unwrap_or(&path);
        let mut prompt = format!("{count} 件の変更を破棄しますか？\n{}", shown.display());
        if has_untracked {
            prompt.push_str("\n(untracked ファイルは削除されます。破棄すると復元できません)");
        }
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::Discard { path, is_dir },
        };
    }

    pub(super) fn execute_discard(&mut self, path: PathBuf, is_dir: bool) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let Some(status) = &self.git else {
            return;
        };
        // tracked (restore 対象) と untracked (削除対象) を先に分けておく。ディレクトリは
        // 配下をまとめて扱う (issue の「パスをそのまま渡す」方針どおり restore はディレクトリ
        // ごと 1 回呼べば済むが、untracked の削除は git がタッチしないので個別に消す)
        let mut untracked_files = Vec::new();
        let mut has_tracked = false;
        for (p, s) in &status.files {
            let hit = if is_dir {
                p.starts_with(&path)
            } else {
                p == &path
            };
            if !hit {
                continue;
            }
            if s.index == Some(git::StatusKind::Untracked) {
                untracked_files.push(p.clone());
            } else {
                has_tracked = true;
            }
        }
        // Confirm 表示中の 500ms デバウンス再取得で対象が消えている可能性がある (稀)。
        // 「何もせず成功扱い」にしないよう、対象なしは明示的にエラー扱いで知らせる
        if !has_tracked && untracked_files.is_empty() {
            self.set_notice("破棄対象が見つかりませんでした", true);
            return;
        }
        let mut ok = true;
        let mut message = String::new();
        if has_tracked {
            let outcome = git::discard_path(&self.root, &path, is_dir);
            if !outcome.ok {
                ok = false;
                message = outcome.message;
            }
        }
        if ok {
            for file in &untracked_files {
                if let Err(err) = std::fs::remove_file(file) {
                    ok = false;
                    message = err.to_string();
                    break;
                }
            }
        }
        if ok {
            self.reload_if_affected(&path, is_dir);
            self.rescan_now();
            self.refresh_git_diff_selection();
            self.set_notice("変更を破棄しました", false);
        } else {
            let message = if message.is_empty() {
                "破棄に失敗しました".to_string()
            } else {
                message
            };
            self.set_notice(message, true);
        }
    }

    /// z: stash push (-u) を確認オーバーレイに乗せる
    pub(super) fn confirm_stash_push(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let count = self.git.as_ref().map_or(0, |status| status.files.len());
        if count == 0 {
            return;
        }
        let prompt = format!(
            "{count} 件の変更を stash に退避しますか？\n(untracked ファイルも含めて退避します)"
        );
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::StashPush,
        };
    }

    pub(super) fn execute_stash_push(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        // stash 自体が git 上で日時を持つため、message は識別用のラベルで十分
        // (chrono 等の新規依存を増やさないよう epoch 秒のまま出す)
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let message = format!("fv: {secs}");
        let outcome = git::run_git_write(&self.root, ["stash", "push", "-u", "-m", &message]);
        if outcome.ok {
            self.reload_current_view();
            self.rescan_now();
            self.refresh_git_diff_selection();
            self.set_notice("変更を stash に退避しました", false);
        } else {
            let message = if outcome.message.is_empty() {
                "stash push に失敗しました".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    /// Z: stash pop を確認オーバーレイに乗せる
    pub(super) fn confirm_stash_pop(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let prompt =
            "直近の stash を pop しますか？\n(コンフリクト時は stash を残したままエラーを表示します)"
                .to_string();
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::StashPop,
        };
    }

    pub(super) fn execute_stash_pop(&mut self) {
        if self.refuse_if_edit_dirty() {
            return;
        }
        let outcome = git::run_git_write(&self.root, ["stash", "pop"]);
        // pop はコンフリクト時に非 0 で終了するが、stash entry を残すのは git 自身の挙動
        // なので追加の後始末は不要。成功・失敗いずれでも worktree は変わりうるので rescan は必ず行う
        self.reload_current_view();
        self.rescan_now();
        self.refresh_git_diff_selection();
        if outcome.ok {
            self.set_notice("stash を復元しました", false);
        } else {
            let message = if outcome.message.is_empty() {
                "stash pop に失敗しました (コンフリクトの可能性があります)".to_string()
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    // GIT レーンの diff は「開いていたファイルの内容そのものが変わった」ケース (discard/stash)
    // では自動追従しない設計のままだと古い内容を映してしまう (通常の j/k 移動時に diff を
    // 追従させない設計とは理由が異なる: あちらはキーリピートで git を連打しないためで、
    // こちらはファイルが破棄され `changed_paths()` から外れた後も GitState::refresh が
    // 同じ path で再取得を試み、その結果を `file_diff` の untracked フォールバックが
    // 「新規ファイルの全行追加」として誤表示してしまうため)。rescan 後にツリー側の新しい
    // 選択へ diff を明示的に向け直す
    fn refresh_git_diff_selection(&mut self) {
        let Some(path) = self.tree.selected_or_first_file() else {
            return;
        };
        let root = self.root.clone();
        if let Lane::Git(git) = &mut self.lane {
            git.open(&root, &path);
        }
    }

    // VIEW で開いていたファイルが破棄対象に含まれる場合だけ reload する (保存時と同じ経路)。
    // GIT レーンの diff 側は refresh_git_diff_selection が別途面倒を見るのでここでは触らない
    fn reload_if_affected(&mut self, path: &Path, is_dir: bool) {
        let Some(open_path) = self.viewer.current.as_ref().map(|open| open.path.clone()) else {
            return;
        };
        let affected = if is_dir {
            open_path.starts_with(path)
        } else {
            open_path == *path
        };
        if affected {
            self.viewer.reload(&open_path);
        }
    }

    // stash は working tree 全体に影響するため、対象パスを絞らずに現在表示中のファイルを
    // 無条件で reload する (discard の reload_if_affected と違い、影響範囲を事前に特定できない)
    fn reload_current_view(&mut self) {
        if let Some(path) = self.viewer.current.as_ref().map(|open| open.path.clone()) {
            self.viewer.reload(&path);
        }
    }

    /// f: リモートの更新を取得する。fetch はローカルを変更しないので確認は不要 (issue の要求通り)
    pub(super) fn start_fetch(&mut self) {
        if !self.branch_available() {
            return;
        }
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Fetch, move || git::fetch(&root));
    }

    /// p: fast-forward のみで取り込む。マージ・リベースが必要な状況は fv が引き受けず、
    /// fast-forward できないときの git のエラーをそのまま notice に出す (issue の要求通り)
    pub(super) fn start_pull(&mut self) {
        if !self.branch_available() {
            return;
        }
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Pull, move || git::pull(&root));
    }

    /// P: push は確認オーバーレイを必須にする (issue の要求。fetch/pull と違いリモートの
    /// 履歴・ブランチ構成を変えるため)。未保存の EDIT バッファは拒否まではせず、
    /// prompt に警告を足すだけに留める (issue の要求通り)。既にジョブが実行中なら
    /// 確認オーバーレイ自体を開かない (開いても実行時に start_remote_job が無視するだけで
    /// ユーザーには何も起きなかったように見えてしまうため、ここで先に弾く)。
    /// dirty チェックは open_commit/open_branch と同じ理由で型上ここへは実際には来ない
    /// (Lane::Edit は印字キーを全て文字入力にするため 'P' はここまで届かない) が、
    /// issue の要求通り明示的にガードしておく (belt and suspenders)
    pub(super) fn confirm_push(&mut self) {
        if !self.branch_available() || self.pending_remote_job.is_some() {
            return;
        }
        let Some(status) = &self.branch_status else {
            return;
        };
        let target = if status.has_upstream {
            format!("origin/{}", status.name)
        } else {
            format!("origin/{} (new upstream)", status.name)
        };
        let mut prompt = format!("push を実行しますか？\n{target}");
        if let Lane::Edit(state) = &self.lane
            && state.buffer.dirty()
        {
            prompt.push_str("\n(未保存の編集があります。保存を忘れずに)");
        }
        self.mode = Mode::Confirm {
            prompt,
            action: ConfirmAction::Push,
        };
    }

    pub(super) fn execute_push(&mut self) {
        let Some(status) = &self.branch_status else {
            return;
        };
        let branch = status.name.clone();
        let has_upstream = status.has_upstream;
        let root = self.root.clone();
        self.start_remote_job(git::RemoteJobKind::Push, move || {
            git::push(&root, &branch, has_upstream)
        });
    }
}
