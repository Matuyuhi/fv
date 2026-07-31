// git CLI ラッパー。git2 等の新規依存を増やさず、素の git コマンドを呼んで
// porcelain / diff 出力をパースする。git が無い・repo でない・コマンド失敗
// といった全てのケースを Option で吸収し、呼び出し側は panic せず
// 「git 情報なし」として通常表示にフォールバックできるようにする。
//
// 読み取り (run_git) と書き込み (run_git_write) は別関数にする。読み取りは
// GIT_OPTIONAL_LOCKS=0 で index lock を取らせないのが意図的な設計で、書き込みに
// そのまま流用すると git add 等が壊れうるため統一しない。

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

// 上限を超える行数/バイト数の diff は打ち切る。GIT レーンでの単発の `A` 操作とはいえ、
// 巨大なリポジトリでの一括変更 (依存更新等) を丸ごと Line 化すると描画・スクロールが
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

fn truncate_diff(text: String) -> (Vec<String>, bool) {
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

fn to_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_string)
        .collect()
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
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output();
    match output {
        Ok(output) if output.status.success() => GitOutcome {
            ok: true,
            message: first_line(&output.stdout),
        },
        Ok(output) => GitOutcome {
            ok: false,
            message: first_line(&output.stderr),
        },
        Err(_) => GitOutcome {
            ok: false,
            message: "git を実行できませんでした".to_string(),
        },
    }
}

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

// 出力の先頭の非空行を取り出す。無ければ空文字列 (成功時は notice にそのまま出しても違和感がない)
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
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

/// ブランチ一覧オーバーレイ (`b`) の 1 行分
pub struct BranchEntry {
    pub name: String, // refname:short ("main" / "origin/feature")
    pub remote: bool,
    pub upstream: Option<String>,
    pub relative_time: String,
    pub subject: String,
}

/// ローカル・リモート追跡ブランチを一括取得する。取得失敗 (git 未インストール・非 repo・
/// ref が1つも無い) は空 Vec (BranchState は空を前提に組んである)。
/// refname:short だけでは local/remote を判別できない (両方とも単なる短縮名なため) ので、
/// 判別用にフルの refname も同じ呼び出しで一緒に取得する
pub fn branches(root: &Path) -> Vec<BranchEntry> {
    let Some(output) = run_git(
        root,
        [
            "for-each-ref",
            "--format=%(refname)%00%(refname:short)%00%(upstream:short)%00%(committerdate:relative)%00%(subject)",
            "refs/heads",
            "refs/remotes",
            "--sort=-committerdate",
        ],
    ) else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\0');
            let full = fields.next()?;
            let name = fields.next()?.to_string();
            let upstream = fields.next().filter(|s| !s.is_empty()).map(str::to_string);
            let relative_time = fields.next().unwrap_or_default().to_string();
            let subject = fields.next().unwrap_or_default().to_string();
            // origin/HEAD のようなリモートの symbolic ref はブランチとして無意味なので除く
            if full.ends_with("/HEAD") {
                return None;
            }
            Some(BranchEntry {
                remote: full.starts_with("refs/remotes/"),
                name,
                upstream,
                relative_time,
                subject,
            })
        })
        .collect()
}

/// ステータスバー常時表示用。detached HEAD は短縮 SHA を name に入れ detached を立てる。
/// upstream 無しは ahead/behind を 0 のまま has_upstream: false で返す (rev-list の失敗を
/// 「情報なし」として吸収するだけで、呼び出し側にエラー扱いさせない)
pub struct BranchStatus {
    pub name: String,
    pub detached: bool,
    pub has_upstream: bool,
    pub ahead: usize,
    pub behind: usize,
}

/// 現在のブランチと ahead/behind を取得する。非 git repo・git 未インストールは None。
/// 取得は呼び出し側 (App::rescan の 500ms デバウンス) に任せ、ここでは毎回素直に叩く
pub fn branch_status(root: &Path) -> Option<BranchStatus> {
    let output = run_git(root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detached = head == "HEAD";
    let name = if detached {
        // detached HEAD は abbrev-ref がそのまま "HEAD" を返すので、短縮 SHA を別途取る
        run_git(root, ["rev-parse", "--short", "HEAD"])
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or(head)
    } else {
        head
    };
    let mut status = BranchStatus {
        name,
        detached,
        has_upstream: false,
        ahead: 0,
        behind: 0,
    };
    if detached {
        return Some(status);
    }
    // upstream 未設定だと @{upstream} の解決に失敗して非 0 exit になる。それは異常系ではなく
    // 「まだ紐付いていない」なので、ahead/behind 0 のまま has_upstream: false で返す
    if let Some(output) = run_git(
        root,
        ["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    ) && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.split_whitespace();
        if let (Some(behind), Some(ahead)) = (parts.next(), parts.next()) {
            status.behind = behind.parse().unwrap_or(0);
            status.ahead = ahead.parse().unwrap_or(0);
            status.has_upstream = true;
        }
    }
    Some(status)
}

/// ローカルブランチへ切り替える (`git switch <name>`)
pub fn switch_branch(root: &Path, name: &str) -> GitOutcome {
    run_git_write(root, ["switch", name])
}

/// リモート追跡ブランチを新しいローカルブランチとして切り替える。
/// remote_ref はリモート側の refname:short ("origin/feature" 等) をそのまま渡す
pub fn switch_track_branch(root: &Path, remote_ref: &str) -> GitOutcome {
    run_git_write(root, ["switch", "--track", remote_ref])
}

/// 新規ブランチを作成して切り替える (`git switch -c <name>`)
pub fn create_branch(root: &Path, name: &str) -> GitOutcome {
    run_git_write(root, ["switch", "-c", name])
}

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
            message: "git を実行できませんでした".to_string(),
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
