// unified diff の取得一式。VIEW の変更行マーク (changed_lines)・EDIT のライブ diff の比較元
// (baseline_lines)・GIT レーンの diff (file_diff / diff_all) が全てここを通る。

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::run_git;

/// git diff の基準。changed_lines / baseline_lines (VIEW の gutter マーク・EDIT のライブ diff)
/// はここに連動させない。GIT レーンの操作で閲覧・編集の変更行マークが勝手に変わるのを避けるため、
/// 常に HEAD 固定のまま
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DiffBase {
    Head,
    Staged,
    Unstaged,
}

impl DiffBase {
    pub fn next(self) -> Self {
        match self {
            DiffBase::Head => DiffBase::Staged,
            DiffBase::Staged => DiffBase::Unstaged,
            DiffBase::Unstaged => DiffBase::Head,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DiffBase::Head => "HEAD",
            DiffBase::Staged => "staged",
            DiffBase::Unstaged => "unstaged",
        }
    }
}

/// `git diff HEAD -U0` の hunk header から、追加・変更された行番号 (1-origin, +側) を集める。
/// HEAD の無い初期 repo では素の `git diff -U0` (index との比較) にフォールバックする。
/// 基準は常に HEAD 固定 (DiffBase を取らない)。GIT レーンで diff 基準を切り替えても
/// VIEW の変更行マークが連動して変わらないようにするため
pub fn changed_lines(root: &Path, file: &Path) -> Option<HashSet<usize>> {
    let mut output = run_git(
        root,
        diff_args(&["diff", "HEAD", "-U0", "--no-color"], Some(file)),
    );
    if !output.as_ref().is_some_and(|o| o.status.success()) {
        output = run_git(root, diff_args(&["diff", "-U0", "--no-color"], Some(file)));
    }
    let output = output?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = HashSet::new();
    for line in text.lines() {
        if let Some((start, count)) = parse_hunk_header(line) {
            lines.extend(start..start + count);
        }
    }
    Some(lines)
}

/// changed_lines と同じ基準 (HEAD → 初期 repo は index) のファイル内容を行で返す。基準は
/// changed_lines 同様 HEAD 固定。編集中のライブ diff の比較元。untracked・repo 外・取得失敗は None
pub fn baseline_lines(root: &Path, file: &Path) -> Option<Vec<String>> {
    // `./` 前置きの spec は -C の cwd 相対で解決される (repo toplevel の取得が要らない)
    let rel = file.strip_prefix(root).ok()?.to_str()?.to_string();
    let mut output = run_git(root, ["show", &format!("HEAD:./{rel}")]);
    if !output.as_ref().is_some_and(|o| o.status.success()) {
        output = run_git(root, ["show", &format!(":0:./{rel}")]);
    }
    let output = output?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
        .collect();
    if text.ends_with('\n') {
        lines.pop();
    }
    Some(lines)
}

/// GIT レーンの diff 表示用に unified diff を行で返す。基準は `DiffBase` で切替:
/// Head は changed_lines / baseline_lines と同じ (HEAD → 初期 repo は素の diff)、
/// Staged は index vs HEAD、Unstaged は worktree vs index。
/// untracked で差分が空になる場合の --no-index フォールバックは Head/Unstaged のみ
/// (Staged では「index にまだ無い」が正しい状態なので --no-index を出さない)。
pub fn file_diff(root: &Path, file: &Path, base: DiffBase) -> Option<Vec<String>> {
    let text = diff_text(root, Some(file), base);
    if !text.trim().is_empty() {
        return Some(text.lines().map(str::to_string).collect());
    }
    if base == DiffBase::Staged {
        return None;
    }
    let text = untracked_diff_text(root, file)?;
    if text.trim().is_empty() {
        return None;
    }
    Some(text.lines().map(str::to_string).collect())
}

// 上限を超える行数/バイト数の diff は打ち切る。GIT レーンでの単発の `A` 操作・PR タブの
// `gh pr diff` (#34) とはいえ、巨大な変更を丸ごと Line 化すると描画・スクロールが
// 固まりうるため、行単位で打ち切りを判定する (提案値どおり 20000 行 / 2MB)
const DIFF_ALL_LINE_LIMIT: usize = 20_000;
const DIFF_ALL_BYTE_LIMIT: usize = 2 * 1024 * 1024;

/// GIT レーンの `A` (全変更ファイルをまとめた diff、#31) 用。ファイル指定なしの 1 回の
/// `git diff` で tracked 分をまとめて取り、untracked 分 (index に無く上の diff には出ない)
/// は呼び出し側が渡すパス一覧を使って `--no-index` で個別に取ってから連結する。
/// 戻り値の bool は行数/バイト数上限による打ち切りが発生したかどうか (呼び出し側で notice に出す)
pub fn diff_all(root: &Path, base: DiffBase, untracked: &[PathBuf]) -> (Vec<String>, bool) {
    let mut text = diff_text(root, None, base);
    // Staged では「index にまだ無い」が正しい状態なので untracked を連結しない
    // (file_diff の --no-index フォールバックと同じ方針)
    if base != DiffBase::Staged {
        for file in untracked {
            // --no-index は repo を経由せず与えたパスをそのままヘッダに出す。絶対パスのまま
            // 渡すと「全ファイルまとめ diff」のファイル境界見出し (segment_label が
            // "+++ b/<path>" から抜き出す) が長い絶対パスになってしまうため、
            // -C root で cwd が root な間にリポジトリ相対へ変換してから渡す
            let rel = file.strip_prefix(root).unwrap_or(file);
            let Some(chunk) = untracked_diff_text(root, rel) else {
                continue;
            };
            if chunk.trim().is_empty() {
                continue;
            }
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&chunk);
        }
    }
    truncate_diff(text)
}

// pull requests タブ (#34) の `gh pr diff` 出力も同じ上限で打ち切るため pub(crate) にする
// (diff_all と同じ打ち切りロジックを 2 回書かない)
pub(crate) fn truncate_diff(text: String) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    for line in text.lines() {
        if lines.len() >= DIFF_ALL_LINE_LIMIT || bytes + line.len() > DIFF_ALL_BYTE_LIMIT {
            truncated = true;
            break;
        }
        bytes += line.len() + 1;
        lines.push(line.to_string());
    }
    (lines, truncated)
}

// untracked (index に相当するものが無い) ファイル 1 件分の diff。--no-index は差分ありを
// exit code 1 で返すので status は見ずに stdout だけ拾う
fn untracked_diff_text(root: &Path, file: &Path) -> Option<String> {
    let output = run_git(
        root,
        [
            OsString::from("diff"),
            OsString::from("--no-color"),
            OsString::from("--no-index"),
            OsString::from("--"),
            OsString::from("/dev/null"),
            file.as_os_str().to_os_string(),
        ],
    )?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn diff_text(root: &Path, file: Option<&Path>, base: DiffBase) -> String {
    let output = match base {
        DiffBase::Head => {
            let mut output = run_git(root, diff_args(&["diff", "HEAD", "--no-color"], file));
            if !output.as_ref().is_some_and(|o| o.status.success()) {
                output = run_git(root, diff_args(&["diff", "--no-color"], file));
            }
            output
        }
        // --cached は HEAD の無い初期 repo でも動く (index を空 tree と比較する)。
        // フォールバックが要らないのは Head と違う点
        DiffBase::Staged => run_git(root, diff_args(&["diff", "--cached", "--no-color"], file)),
        DiffBase::Unstaged => run_git(root, diff_args(&["diff", "--no-color"], file)),
    };
    match output {
        Some(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

// file が None ならファイル指定なし (リポジトリ全体) の diff になる (`A` まとめ diff 用)
fn diff_args(base: &[&str], file: Option<&Path>) -> Vec<OsString> {
    let mut args: Vec<OsString> = base.iter().map(OsString::from).collect();
    if let Some(file) = file {
        args.push("--".into());
        args.push(file.as_os_str().to_os_string());
    }
    args
}

// "@@ -a,b +c,d @@ ..." の +c,d 側だけを見る。d (行数) 省略時は1行、0 なら削除のみで追加行なし
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let new_range = line.split_whitespace().nth(2)?.strip_prefix('+')?;
    let mut parts = new_range.splitn(2, ',');
    let start: usize = parts.next()?.parse().ok()?;
    let count: usize = match parts.next() {
        Some(c) => c.parse().ok()?,
        None => 1,
    };
    Some((start, count))
}
