// git CLI ラッパー。git2 等の新規依存を増やさず、素の git コマンドを呼んで
// porcelain / diff 出力をパースする。git が無い・repo でない・コマンド失敗
// といった全てのケースを Option で吸収し、呼び出し側は panic せず
// 「git 情報なし」として通常表示にフォールバックできるようにする。
//
// 読み取り (run_git) と書き込み (run_git_write) は別関数にする。読み取りは
// GIT_OPTIONAL_LOCKS=0 で index lock を取らせないのが意図的な設計で、書き込みに
// そのまま流用すると git add 等が壊れうるため統一しない。

mod branch;
mod diff;
mod log;
mod remote;
mod status;
mod write;

pub use branch::{
    BranchEntry, BranchStatus, branch_status, branches, create_branch, switch_branch,
    switch_track_branch,
};
pub(crate) use diff::truncate_diff;
pub use diff::{DiffBase, baseline_lines, changed_lines, diff_all, file_diff};
pub use log::{CommitSummary, log, show_commit};
pub use remote::{RemoteJobKind, fetch, pull, push};
pub use status::{FileStatus, GitStatus, StatusKind, file_statuses};
pub use write::{
    apply_cached, commit, discard_path, last_commit_message, stage_path, unstage_path,
};

use crate::lang::{Msg, t};
use crate::logger;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::{Command, ExitStatus, Output};

fn run_git<I, S>(root: &Path, args: I) -> Option<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().into()).collect();
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        // ビューアはあくまで読み取り用途なので、index lock を取らせない
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();
    match output {
        Ok(output) => {
            // 読み取りの非ゼロ終了は正常系でも頻繁に起きる (upstream が無い・HEAD の無い repo・
            // 新規ファイルの HEAD 版等) ので debug に留める
            if !output.status.success() {
                logger::debug(
                    "git",
                    format_args!(
                        "git {} failed ({}): {}",
                        logger::command_label(&args, 1),
                        output.status,
                        first_line(&output.stderr)
                    ),
                );
            }
            Some(output)
        }
        Err(e) => {
            log_spawn_failure(&args, &e);
            None
        }
    }
}

/// 書き込み系コマンドの実行結果。`ok` が false のとき `message` は失敗理由 (stderr 先頭の非空行)、
/// true のときは stdout 先頭の非空行 (無ければ空文字列) が入る
pub struct GitOutcome {
    pub ok: bool,
    pub message: String,
}

/// stage / commit / discard 等、書き込み系コマンドの実行。読み取り専用の `run_git` と違い
/// `GIT_OPTIONAL_LOCKS` は付けない (index lock を正しく取らせる)。`GIT_TERMINAL_PROMPT=0` で
/// 認証待ちによる TUI ハングを防ぐ (fetch/push 等のリモート操作で効いてくる)。
/// git 未インストール・実行失敗も呼び出し側を単純にするため Option にせず ok: false に潰す
pub fn run_git_write<I, S>(root: &Path, args: I) -> GitOutcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<OsString> = args.into_iter().map(|a| a.as_ref().into()).collect();
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() => GitOutcome {
            ok: true,
            message: first_line(&output.stdout),
        },
        Ok(output) => {
            let message = first_line(&output.stderr);
            log_write_failure(&args, &output.status, &message);
            GitOutcome { ok: false, message }
        }
        Err(e) => {
            log_spawn_failure(&args, &e);
            GitOutcome {
                ok: false,
                message: t(Msg::GitCannotRun).to_string(),
            }
        }
    }
}

// ログに残すのはサブコマンド名・終了状態・UI に出すのと同じ stderr の 1 行だけ。
// 残りの引数 (パス・ブランチ名) や stdin (コミットメッセージ・パッチ) は書かない
fn log_write_failure(args: &[OsString], status: &ExitStatus, message: &str) {
    logger::warn(
        "git",
        format_args!(
            "git {} failed ({status}): {message}",
            logger::command_label(args, 1)
        ),
    );
}

fn log_spawn_failure(args: &[OsString], error: &std::io::Error) {
    logger::error(
        "git",
        format_args!("cannot run git {}: {error}", logger::command_label(args, 1)),
    );
}

// 出力の先頭の非空行を取り出す。無ければ空文字列 (成功時は notice にそのまま出しても違和感がない)
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn to_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_string)
        .collect()
}
