// git status (porcelain -z) のパース。rename/copy の 2 パス形式や、ツリー描画で使う
// 「配下に変更を持つディレクトリ」集合の組み立てはここに閉じる。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::run_git;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusKind {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
}

/// porcelain の XY を index 側 (X) / worktree 側 (Y) に分けたまま持つ。1 種類に潰すと
/// 「ステージ済みかどうか」が表現できず、staged/unstaged diff の切替と食い違うため
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileStatus {
    pub index: Option<StatusKind>,
    pub worktree: Option<StatusKind>,
}

/// git status の結果一式。changed_dirs は「配下に変更ファイルを持つディレクトリ」の
/// 絶対パス集合で、files 取得時に一度だけ祖先を辿って作る。ツリー描画のたびに
/// files を全走査してディレクトリの変更有無を判定しなくて済む。
pub struct GitStatus {
    pub files: HashMap<PathBuf, FileStatus>,
    pub changed_dirs: HashSet<PathBuf>,
    /// 配下に未ステージ変更 (worktree 側) を持つディレクトリ。ツリーの色分けが
    /// 「配下が全て stage 済みか」を files の全走査なしに判定できるよう changed_dirs と同時に作る
    pub unstaged_dirs: HashSet<PathBuf>,
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
    let mut unstaged_dirs = HashSet::new();
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
        let status = classify(x, y);
        for dir in abs.ancestors().skip(1).take_while(|a| *a != toplevel) {
            changed_dirs.insert(dir.to_path_buf());
            if status.worktree.is_some() {
                unstaged_dirs.insert(dir.to_path_buf());
            }
        }
        files.insert(abs, status);
    }

    Some(GitStatus {
        files,
        changed_dirs,
        unstaged_dirs,
    })
}

// untracked (`??`) は index に相当するものが無いので、両側とも Untracked を入れて XY = "??" にする
fn classify(x: char, y: char) -> FileStatus {
    if x == '?' && y == '?' {
        return FileStatus {
            index: Some(StatusKind::Untracked),
            worktree: Some(StatusKind::Untracked),
        };
    }
    FileStatus {
        index: status_kind(x),
        worktree: status_kind(y),
    }
}

// porcelain の 1 文字コードを StatusKind へ。空白は「その側は変更なし」で None。
// M/C/T 等その他の文字は既存挙動 (未分類は Modified 扱い) を維持する
fn status_kind(code: char) -> Option<StatusKind> {
    match code {
        ' ' => None,
        'A' => Some(StatusKind::Added),
        'D' => Some(StatusKind::Deleted),
        'R' => Some(StatusKind::Renamed),
        _ => Some(StatusKind::Modified),
    }
}

// status の porcelain 出力パスは -C の cwd ではなく常に repo トップレベル基準になるため、
// 絶対パスの組み立てにはトップレベルが要る
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
