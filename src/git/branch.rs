// ブランチ一覧オーバーレイ (`b`) とステータスバーの現在ブランチ表示が使う参照系 + 切替。

use std::path::Path;

use super::{GitOutcome, run_git, run_git_write};

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
