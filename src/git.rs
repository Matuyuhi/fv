// git CLI ラッパー。git2 等の新規依存を増やさず、素の git コマンドを呼んで
// porcelain / diff 出力をパースする。git が無い・repo でない・コマンド失敗
// といった全てのケースを Option で吸収し、呼び出し側は panic せず
// 「git 情報なし」として通常表示にフォールバックできるようにする。

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileStatus {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
}

/// git status の結果一式。changed_dirs は「配下に変更ファイルを持つディレクトリ」の
/// 絶対パス集合で、files 取得時に一度だけ祖先を辿って作る。ツリー描画のたびに
/// files を全走査してディレクトリの変更有無を判定しなくて済む。
pub struct GitStatus {
    pub files: HashMap<PathBuf, FileStatus>,
    pub changed_dirs: HashSet<PathBuf>,
}

/// `git -C <root> status --porcelain -z` を実行し、変更ファイルの絶対パスと
/// 状態の対応を返す。git 未インストール・repo 外では None。
pub fn file_statuses(root: &Path) -> Option<GitStatus> {
    // status の porcelain 出力パスは -C の cwd ではなく常に repo トップレベル基準になるため、
    // トップレベルを別途取得して絶対パスの組み立てに使う
    let toplevel = git_toplevel(root)?;
    let output = run_git(
        root,
        ["status", "--porcelain", "-z", "--untracked-files=all"],
    )?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut files = HashMap::new();
    let mut changed_dirs = HashSet::new();
    // -z 区切りの各フィールドを走査。rename/copy (先頭が R/C) は "新パス" フィールドの
    // 直後に XY プレフィックスなしの "旧パス" フィールドが続く2フィールド形式なので、
    // 該当時だけ余分に1つ読み飛ばす
    let mut fields = stdout.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let bytes = entry.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        let path_str = &entry[3..];
        if x == 'R' || x == 'C' {
            fields.next();
        }

        let abs = toplevel.join(path_str);
        for dir in abs.ancestors().skip(1).take_while(|a| *a != toplevel) {
            changed_dirs.insert(dir.to_path_buf());
        }
        files.insert(abs, classify(x, y));
    }

    Some(GitStatus {
        files,
        changed_dirs,
    })
}

/// `git diff HEAD -U0` の hunk header から、追加・変更された行番号 (1-origin, +側) を集める。
/// HEAD の無い初期 repo では素の `git diff -U0` (index との比較) にフォールバックする。
pub fn changed_lines(root: &Path, file: &Path) -> Option<HashSet<usize>> {
    let mut output = run_git(
        root,
        diff_args(&["diff", "HEAD", "-U0", "--no-color"], file),
    );
    if !output.as_ref().is_some_and(|o| o.status.success()) {
        output = run_git(root, diff_args(&["diff", "-U0", "--no-color"], file));
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

/// changed_lines と同じ基準 (HEAD → 初期 repo は index) のファイル内容を行で返す。
/// 編集中のライブ diff の比較元。untracked・repo 外・取得失敗は None
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

fn diff_args(base: &[&str], file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = base.iter().map(OsString::from).collect();
    args.push("--".into());
    args.push(file.as_os_str().to_os_string());
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

fn classify(x: char, y: char) -> FileStatus {
    if x == '?' && y == '?' {
        FileStatus::Untracked
    } else if x == 'R' || y == 'R' {
        FileStatus::Renamed
    } else if x == 'A' {
        FileStatus::Added
    } else if x == 'D' || y == 'D' {
        FileStatus::Deleted
    } else {
        FileStatus::Modified
    }
}

fn git_toplevel(root: &Path) -> Option<PathBuf> {
    let output = run_git(root, ["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(PathBuf::from(text))
}

fn run_git<I, S>(root: &Path, args: I) -> Option<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        // ビューアはあくまで読み取り用途なので、index lock を取らせない
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()
}
