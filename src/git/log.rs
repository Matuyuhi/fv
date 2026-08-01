// LOG レーン用のコミット一覧・単一コミットの表示テキスト取得。

use std::path::Path;

use super::{run_git, to_lines};

/// LOG レーンの一覧 1 行分。表示に必要な項目だけを持つ (diff 本体は選択時に別途 show_commit で取る)
pub struct CommitSummary {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub relative_time: String,
    pub subject: String,
}

/// `git log --format=... -z -n <limit> --skip=<skip>` を実行し、コミット一覧を返す。
/// porcelain -z のパースと同じ流儀で `%x00` 区切りを自前で分ける。コミットが 1 つも無い
/// repo (HEAD 無し) では git log 自体が失敗するが、それは「0 件」であって異常系ではないので
/// 空 Vec を返し呼び出し側 (LogState) は panic せず「no commits」を出すだけで良い
pub fn log(root: &Path, skip: usize, limit: usize) -> Vec<CommitSummary> {
    let mut args = vec![
        "log".to_string(),
        "--format=%H%x00%h%x00%an%x00%ar%x00%s".to_string(),
        "-z".to_string(),
        "-n".to_string(),
        limit.to_string(),
    ];
    if skip > 0 {
        args.push(format!("--skip={skip}"));
    }
    let Some(output) = run_git(root, args) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // -z は区切りをコミット末尾の NUL に変え、各コミット内のフィールドも同じ NUL (%x00) で
    // 区切られる。末尾の空文字列 (最後のコミットの後ろの NUL) だけを落とし、5 個ずつまとめる
    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    fields
        .chunks_exact(5)
        .map(|c| CommitSummary {
            hash: c[0].to_string(),
            short: c[1].to_string(),
            author: c[2].to_string(),
            relative_time: c[3].to_string(),
            subject: c[4].to_string(),
        })
        .collect()
}

/// 選択コミットの表示用テキスト (`git show` 相当) を生行で返す。マージコミットは既定の
/// `git show` が差分を出さないため、親が複数あるときは最初の親との diff を明示的に組み立てて
/// 見せる (全親の差分 (-m) は本文が膨らみすぎるため採用しない。判断は CLAUDE.md 参照)。
/// 親 0/1 のコミットは通常の `git show` の既定動作 (空 tree / 唯一の親との diff) をそのまま使う
pub fn show_commit(root: &Path, sha: &str) -> Option<Vec<String>> {
    let parents = parent_count(root, sha).unwrap_or(0);
    if parents <= 1 {
        let output = run_git(root, ["show", "--no-color", sha])?;
        if !output.status.success() {
            return None;
        }
        return Some(to_lines(&output.stdout));
    }
    let header = run_git(root, ["show", "--no-color", "--quiet", sha])?;
    if !header.status.success() {
        return None;
    }
    let diff = run_git(root, ["diff", "--no-color", &format!("{sha}^1"), sha])?;
    if !diff.status.success() {
        return None;
    }
    let mut lines = to_lines(&header.stdout);
    lines.push(String::new());
    lines.push("(merge commit: diff against first parent)".to_string());
    lines.push(String::new());
    lines.extend(to_lines(&diff.stdout));
    Some(lines)
}

fn parent_count(root: &Path, sha: &str) -> Option<usize> {
    let output = run_git(root, ["rev-list", "--parents", "-n", "1", sha])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    // "<sha> <parent1> <parent2> ..." の自分自身を除いた個数
    Some(text.split_whitespace().count().saturating_sub(1))
}
