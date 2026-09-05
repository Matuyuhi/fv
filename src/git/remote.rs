// fetch / pull / push (#27)。認証プロンプトで裏のスレッドが無限に待つのが最悪の挙動なので、
// 実行環境の潰し方 (run_git_remote) をこのモジュール 1 箇所に閉じる。

use crate::lang::t;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use super::{GitOutcome, first_line};

/// `f`/`p`/`P` (#27) の種別。ステータスバー表示・完了メッセージの組み立て・
/// 多重起動防止 (App::pending_remote_job) に使う
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RemoteJobKind {
    Fetch,
    Pull,
    Push,
}

impl RemoteJobKind {
    pub fn label(self) -> &'static str {
        match self {
            RemoteJobKind::Fetch => "fetch",
            RemoteJobKind::Pull => "pull",
            RemoteJobKind::Push => "push",
        }
    }
}

/// fetch/pull/push 専用の実行。`run_git_write` の `GIT_TERMINAL_PROMPT=0` に加え、
/// `GIT_ASKPASS`/`SSH_ASKPASS` を空文字にしてプロンプト用の外部プロセス起動そのものを潰し、
/// `SSH_ASKPASS_REQUIRE=never` で DISPLAY の有無に関わらず ssh 側からの起動も止める。
/// 認証プロンプトで裏のスレッドが無限に待つのが最悪の挙動なので、ここで確実に潰しておく
/// (待たせるくらいなら「認証が必要」で即失敗させて notice に出す方が安全)。
/// fetch は進捗・更新内容が成功時でも stderr に出るため、stdout が空なら stderr を見る
fn run_git_remote<I, S>(root: &Path, args: I) -> GitOutcome
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let message = first_line(&output.stdout);
            GitOutcome {
                ok: true,
                message: if message.is_empty() {
                    first_line(&output.stderr)
                } else {
                    message
                },
            }
        }
        Ok(output) => GitOutcome {
            ok: false,
            message: remote_error_line(&output.stderr),
        },
        Err(_) => GitOutcome {
            ok: false,
            message: t("git を実行できませんでした", "failed to run git").to_string(),
        },
    }
}

// pull --ff-only の失敗は stderr に fetch の進捗行 ("From ...") や "hint:" 行が本当の失敗理由
// より先に混じるため、first_line をそのまま使うと肝心の "fast-forward できない" が隠れてしまう。
// "fatal:"/"error:" で始まる行を優先して拾い、無ければ従来通り先頭の非空行にフォールバックする
fn remote_error_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .or_else(|| lines.first())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// `f`: リモートの更新を取得する。ローカルを変更しないので確認は不要
pub fn fetch(root: &Path) -> GitOutcome {
    run_git_remote(root, ["fetch", "--prune"])
}

/// `p`: fast-forward のみで取り込む。マージコミットやリベースが必要な状況は fv が
/// 引き受けず、fast-forward できないときの git のエラーをそのまま呼び出し側へ返す
/// (呼び出し側がそのまま notice に出す)
pub fn pull(root: &Path) -> GitOutcome {
    run_git_remote(root, ["pull", "--ff-only"])
}

/// `P`: push。upstream が無ければ現在のブランチ名で `--set-upstream origin <branch>` を付ける
pub fn push(root: &Path, branch: &str, has_upstream: bool) -> GitOutcome {
    if has_upstream {
        run_git_remote(root, ["push"])
    } else {
        run_git_remote(root, ["push", "--set-upstream", "origin", branch])
    }
}
