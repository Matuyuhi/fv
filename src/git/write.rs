// ワーキングツリー・index・履歴を変える書き込み系コマンド (stage / unstage / discard / commit)。
// 実行そのものは run_git_write に寄せ、ここは「どのコマンドをどの順で試すか」だけを持つ。

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use super::{GitOutcome, run_git, run_git_write};

/// stage: modified/untracked は `git add --`、削除 (index/worktree いずれかが Deleted) を
/// 含む場合は `git add -A --` にする (プレーンな add でも現在の git は削除を拾うが、
/// 挙動をバージョンに依存させないため issue の指示通り明示的に分ける)。ディレクトリ選択時は
/// 呼び出し側 (App::toggle_stage_selected) が配下の集約結果を has_deletion として渡す
pub fn stage_path(root: &Path, path: &Path, has_deletion: bool) -> GitOutcome {
    let mut args: Vec<OsString> = vec![OsString::from("add")];
    if has_deletion {
        args.push(OsString::from("-A"));
    }
    args.push(OsString::from("--"));
    args.push(path.as_os_str().to_os_string());
    run_git_write(root, args)
}

/// discard (tracked 分): `git restore --source=HEAD --staged --worktree --`。untracked 分の
/// 削除は git ではなく呼び出し側の fs 操作で扱うため、この関数は tracked 分のみを対象にする。
///
/// HEAD の無い初期 repo では `--staged` 側の復元先が HEAD しかあり得ない (index が最初の
/// 内容そのものなので、index を復元先にする選択肢が無い) ため、`--source` を省いても
/// 同じ「HEAD を解決できない」エラーになる (`--staged --worktree` は明示 `--source=HEAD` と
/// 挙動が同じ。unstage_path の「try → だめなら別コマンド」とは違い、単純な代替コマンドが
/// 存在しない)。そこで HEAD 無し repo 限定のフォールバックとして、`--worktree` 単独
/// (index 基準で HEAD を要求しない) で worktree 側だけ index に揃えた上で、`git rm --cached`
/// (unstage_path と同じ) で index から外す。結果としてファイルは HEAD 相当の内容へは戻らず
/// (存在しないため) untracked として残る点は「破棄」として不完全だが、HEAD 未解決のまま
/// エラーを見せるよりは安全側 (誤ってファイルを消さない) に倒している
pub fn discard_path(root: &Path, path: &Path, is_dir: bool) -> GitOutcome {
    let outcome = run_git_write(
        root,
        [
            OsString::from("restore"),
            OsString::from("--source=HEAD"),
            OsString::from("--staged"),
            OsString::from("--worktree"),
            OsString::from("--"),
            path.as_os_str().to_os_string(),
        ],
    );
    if outcome.ok {
        return outcome;
    }
    let worktree_outcome = run_git_write(
        root,
        [
            OsString::from("restore"),
            OsString::from("--worktree"),
            OsString::from("--"),
            path.as_os_str().to_os_string(),
        ],
    );
    if !worktree_outcome.ok {
        return worktree_outcome;
    }
    let mut args: Vec<OsString> = vec![OsString::from("rm"), OsString::from("--cached")];
    if is_dir {
        args.push(OsString::from("-r"));
    }
    args.push(OsString::from("--"));
    args.push(path.as_os_str().to_os_string());
    run_git_write(root, args)
}

/// unstage: `git restore --staged --`。HEAD の無い初期 repo では restore が HEAD の解決を
/// 要求して失敗するため `git rm --cached --` にフォールバックする (ディレクトリは -r 必須)。
/// 失敗理由をコマンドごとに判別せず常にフォールバックを試すのは、changed_lines 等
/// 既存の「try → だめなら別コマンド」方針と揃えるため
pub fn unstage_path(root: &Path, path: &Path, is_dir: bool) -> GitOutcome {
    let outcome = run_git_write(
        root,
        [
            OsString::from("restore"),
            OsString::from("--staged"),
            OsString::from("--"),
            path.as_os_str().to_os_string(),
        ],
    );
    if outcome.ok {
        return outcome;
    }
    let mut args: Vec<OsString> = vec![OsString::from("rm"), OsString::from("--cached")];
    if is_dir {
        args.push(OsString::from("-r"));
    }
    args.push(OsString::from("--"));
    args.push(path.as_os_str().to_os_string());
    run_git_write(root, args)
}

/// amend のプリフィル用。`%B` はコミットメッセージ本文そのまま。git はコミット保存時に
/// メッセージ末尾を改行 1 個に正規化し、`log --format` はさらにエントリ区切りの改行を足すため、
/// 出力は末尾に改行が 2 個並ぶ。末尾の改行を 1 個だけ剥がすと空行が編集バッファに残ってしまうので
/// 末尾の改行は全て trim する (amend で編集するたびに空行が増えていくのを防ぐ)。
/// コミットが 1 つも無い repo では失敗するのでそのまま None
pub fn last_commit_message(root: &Path) -> Option<String> {
    let output = run_git(root, ["log", "-1", "--format=%B"])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.trim_end_matches('\n').to_string())
}

/// `c`/`C` の実行本体。メッセージは引数ではなく stdin から `-F -` で渡す (エスケープ・
/// コマンドライン長の問題を避けるため)。amend は `--amend -F -` を付けるだけで良い。
/// 成功時の message は「短縮 SHA + 件名」(notice にそのまま出せる形)、失敗時は stderr の
/// 先頭行 (複数行なら "…" を付けて省略したことを示す)
pub fn commit(root: &Path, message: &str, amend: bool) -> GitOutcome {
    let mut args: Vec<OsString> = vec![OsString::from("commit")];
    if amend {
        args.push(OsString::from("--amend"));
    }
    args.push(OsString::from("-F"));
    args.push(OsString::from("-"));

    let mut child = match Command::new("git")
        .arg("-C")
        .arg(root)
        .args(&args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return GitOutcome {
                ok: false,
                message: "git を実行できませんでした".to_string(),
            };
        }
    };
    // stdin を明示的に drop して EOF を送る (`-F -` は EOF まで読み続けるため、
    // 書き込み後に take() したハンドルをスコープ末尾で drop するだけで良い)
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(message.as_bytes());
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(_) => {
            return GitOutcome {
                ok: false,
                message: "git を実行できませんでした".to_string(),
            };
        }
    };
    if !output.status.success() {
        return GitOutcome {
            ok: false,
            message: stderr_summary(&output.stderr),
        };
    }
    // commit の stdout ("[branch hash] subject") は amend やルートコミットで書式が揺れるため、
    // 短縮 SHA は rev-parse で確実な形を取り直す
    let short = run_git(root, ["rev-parse", "--short", "HEAD"])
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let subject = message.lines().next().unwrap_or("").to_string();
    GitOutcome {
        ok: true,
        message: format!("{short} {subject}").trim().to_string(),
    }
}

// pre-commit hook 失敗時などの stderr は複数行になりうる。ステータスバー 1 行に収めるため
// 先頭の非空行だけを見せ、他にも行があれば省略したことが分かるよう "…" を付ける
fn stderr_summary(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let Some(first) = lines.next() else {
        return String::new();
    };
    if lines.next().is_some() {
        format!("{first} …")
    } else {
        first.to_string()
    }
}
