// GitHub モードが使える環境かどうかの判定。呼ぶのは App::new / toggle_github からの
// 1 回きりで、描画のたびには叩かない (CLAUDE.md の GIT レーンと同じ「重い処理はイベントループを
// ブロックしない」方針とは別に、そもそも起動時 1 回に絞ることでブロック自体を避けている)。
//
// issues/PR タブ (#33/#34) の一覧・詳細取得もここに集約する。git.rs と同じ方針で
// serde 等の新規依存は足さず、`--json` ではなく `--template` で `\0` 区切りのプレーン
// テキストを出させ porcelain -z と同じ流儀で自前パースする。
use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

/// 使えれば Ok(())、使えなければ理由 (notice にそのまま出す文言) を返す
pub fn check_available(root: &Path) -> Result<(), String> {
    match Command::new("gh")
        .args(["auth", "status"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output()
    {
        Ok(output) if output.status.success() => {}
        Ok(_) => {
            return Err(
                "GitHub モードを有効化できません: gh が未認証です (gh auth login)".to_string(),
            );
        }
        Err(_) => {
            return Err("GitHub モードを有効化できません: gh コマンドが見つかりません".to_string());
        }
    }
    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .output();
    let is_github_remote = match remote {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).contains("github.com")
        }
        _ => false,
    };
    if !is_github_remote {
        return Err(
            "GitHub モードを有効化できません: origin が GitHub リポジトリではありません"
                .to_string(),
        );
    }
    Ok(())
}

/// issues/PR 一覧の 1 行分。#34 (pull requests タブ) が一覧の描画・絞り込み・キャッシュを
/// そのまま再利用できるよう issue 固有の項目は持たせない (`gh issue list` / `gh pr list` は
/// どちらも同じ --json フィールド名 (number/title/author/updatedAt/labels/state) を返すため、
/// 型を分ける理由がない)。state は "OPEN"/"CLOSED" (PR は将来 "MERGED" も乗る想定だが、
/// 判定は呼び出し側の StateFilter 相当に閉じ、ここでは生の文字列のまま持つ)
pub struct RemoteItem {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub state: String,
}

// number/title/author/updatedAt/labels(","区切り)/state の6フィールド。
// gh issue list --template と gh pr list --template (#34) の両方がこの並びに揃える前提
const ISSUE_LIST_TEMPLATE: &str = r#"{{range .}}{{.number}}{{"\x00"}}{{.title}}{{"\x00"}}{{.author.login}}{{"\x00"}}{{.updatedAt}}{{"\x00"}}{{range .labels}}{{.name}},{{end}}{{"\x00"}}{{.state}}{{"\x00"}}{{end}}"#;

/// `gh issue list` を叩き `RemoteItem` の一覧を返す。state は "open"/"closed" に絞らず常に
/// `all` を取得する — issues タブの `t` (state 絞り込みの循環) をローカルフィルタだけで完結させ、
/// 「タブを往復しても gh を叩かない」と同じ理由で余計な gh 呼び出しを増やさないため
pub fn list_issues(root: &Path) -> Result<Vec<RemoteItem>, String> {
    let stdout = run_gh(
        root,
        [
            "issue",
            "list",
            "--limit",
            "100",
            "--state",
            "all",
            "--json",
            "number,title,author,updatedAt,labels,state",
            "--template",
            ISSUE_LIST_TEMPLATE,
        ],
    )?;
    Ok(parse_list_template(&stdout))
}

// porcelain -z と同じ流儀: \0 区切りのフィールド列を 6 個ずつまとめる。末尾の空フィールド
// (最後のレコード後の \0) だけを落とす
fn parse_list_template(text: &str) -> Vec<RemoteItem> {
    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    fields
        .chunks_exact(6)
        .filter_map(|c| {
            let number = c[0].parse().ok()?;
            let labels = c[4]
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            Some(RemoteItem {
                number,
                title: c[1].to_string(),
                author: c[2].to_string(),
                updated_at: c[3].to_string(),
                labels,
                state: c[5].to_string(),
            })
        })
        .collect()
}

/// 詳細は `gh issue view <n>` のプレーン出力 (本文 + コメント) をそのまま行として返す。
/// `--json`/`--template` を使わないのは、issue の要求通り「gh の整形済み出力をそのまま描く」
/// ためで、パースし直す必要がない
pub fn issue_detail(root: &Path, number: u64) -> Result<Vec<String>, String> {
    let stdout = run_gh(root, ["issue", "view", &number.to_string(), "--comments"])?;
    Ok(stdout.lines().map(str::to_string).collect())
}

/// `o`: ブラウザで開く。実際にブラウザを起動するのは gh 自身で、fv 側は結果 (成功/失敗) だけ見る
pub fn open_issue_web(root: &Path, number: u64) -> Result<(), String> {
    run_gh(root, ["issue", "view", &number.to_string(), "--web"]).map(|_| ())
}

// gh の実行。読み取り専用の照会なので git.rs の run_git と同じ発想で GIT_OPTIONAL_LOCKS は
// 付けない (gh 自体は git の index を触らないため元々関係ないが、明示はしない)。
// 失敗理由は stderr 先頭の非空行に要約する (git.rs の first_line と同じ考え方だが、
// このファイルの責務は gh CLI ラッパーに閉じているため小さな重複を許容し共有しない)
fn run_gh<I, S>(root: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("gh").args(args).current_dir(root).output();
    match output {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => Err(first_line(&output.stderr)),
        Err(_) => Err("gh コマンドが見つかりません".to_string()),
    }
}

fn first_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("gh の実行に失敗しました")
        .to_string()
}
