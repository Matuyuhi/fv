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
/// 判定は呼び出し側の StateFilter 相当に閉じ、ここでは生の文字列のまま持つ)。
/// `body` (#体感速度改善) は一覧取得の時点で受け取っておく本文。詳細を開いた瞬間にネットワーク
/// 往復を発生させない (`gh <kind> view` を待たず即座に描画する) ための核で、フォールバック
/// テンプレート使用時は空文字のまま持つ
pub struct RemoteItem {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub updated_at: String,
    pub labels: Vec<String>,
    pub state: String,
    pub body: String,
}

// number/title/author/updatedAt/labels(","区切り)/state の6フィールド。
// gh issue list --template と gh pr list --template (PR_LIST_TEMPLATE、#34) の先頭 6 個は
// この並びに揃える前提 (parse_records で共有パースする)。body は各テンプレートの末尾に
// 追加してあるので、共通 6 フィールドのパース (remote_item) には含めない
const ISSUE_LIST_TEMPLATE: &str = r#"{{range .}}{{.number}}{{"\x00"}}{{.title}}{{"\x00"}}{{.author.login}}{{"\x00"}}{{.updatedAt}}{{"\x00"}}{{range .labels}}{{.name}},{{end}}{{"\x00"}}{{.state}}{{"\x00"}}{{.body}}{{"\x00"}}{{end}}"#;
const ISSUE_LIST_JSON_FIELDS: &str = "number,title,author,updatedAt,labels,state,body";

// body を含まない従来幅のテンプレート。`--json` に body を渡すと失敗する古い gh 向けの
// フォールバック専用 (list_issues 参照)
const ISSUE_LIST_TEMPLATE_LEGACY: &str = r#"{{range .}}{{.number}}{{"\x00"}}{{.title}}{{"\x00"}}{{.author.login}}{{"\x00"}}{{.updatedAt}}{{"\x00"}}{{range .labels}}{{.name}},{{end}}{{"\x00"}}{{.state}}{{"\x00"}}{{end}}"#;
const ISSUE_LIST_JSON_FIELDS_LEGACY: &str = "number,title,author,updatedAt,labels,state";

/// `gh issue list` を叩き `RemoteItem` の一覧を返す。state は "open"/"closed" に絞らず常に
/// `all` を取得する — issues タブの `t` (state 絞り込みの循環) をローカルフィルタだけで完結させ、
/// 「タブを往復しても gh を叩かない」と同じ理由で余計な gh 呼び出しを増やさないため。
/// `--json` に `body` が無い gh バージョンだとコマンド全体が失敗し一覧ごと出なくなるため、
/// 失敗時は body 抜きの従来テンプレートで 1 回だけ再試行する (この時 body は空文字になる)
pub fn list_issues(root: &Path) -> Result<Vec<RemoteItem>, String> {
    let args = [
        "issue",
        "list",
        "--limit",
        "100",
        "--state",
        "all",
        "--json",
        ISSUE_LIST_JSON_FIELDS,
        "--template",
        ISSUE_LIST_TEMPLATE,
    ];
    if let Ok(stdout) = run_gh(root, args) {
        return Ok(parse_records(&stdout, 7)
            .into_iter()
            .filter_map(|c| {
                let mut item = remote_item(&c)?;
                item.body = c[6].to_string();
                Some(item)
            })
            .collect());
    }
    let legacy_args = [
        "issue",
        "list",
        "--limit",
        "100",
        "--state",
        "all",
        "--json",
        ISSUE_LIST_JSON_FIELDS_LEGACY,
        "--template",
        ISSUE_LIST_TEMPLATE_LEGACY,
    ];
    let stdout = run_gh(root, legacy_args)?;
    Ok(parse_records(&stdout, 6)
        .into_iter()
        .filter_map(|c| remote_item(&c))
        .collect())
}

/// porcelain -z と同じ流儀: \0 区切りのフィールド列を `width` 個ずつまとめる。末尾の空フィールド
/// (最後のレコード後の \0) だけを落とす。issue/PR どちらの --template パースもこれを通す
fn parse_records(text: &str, width: usize) -> Vec<Vec<&str>> {
    let mut fields: Vec<&str> = text.split('\0').collect();
    if fields.last().is_some_and(|s| s.is_empty()) {
        fields.pop();
    }
    fields.chunks_exact(width).map(|c| c.to_vec()).collect()
}

// 先頭 6 フィールド (number/title/author/updatedAt/labels/state) から RemoteItem を組み立てる。
// PR の 8/9 フィールド版 (list_prs、parse_records(..., 8|9)) もこの並びを先頭に持つので共有する。
// body (末尾フィールド、issues は index 6・PR は index 8) は位置がテンプレートごとに違うため
// ここには含めず、呼び出し側が別途 fields から拾って上書きする
fn remote_item(fields: &[&str]) -> Option<RemoteItem> {
    let number = fields[0].parse().ok()?;
    let labels = fields[4]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some(RemoteItem {
        number,
        title: fields[1].to_string(),
        author: fields[2].to_string(),
        updated_at: fields[3].to_string(),
        labels,
        state: fields[5].to_string(),
        body: String::new(),
    })
}

/// コメントだけを取得する。本文は一覧取得の時点で `RemoteItem::body` に入っているので、
/// 詳細を開いた瞬間はこれ 1 回の往復で済む (以前は本文取得 + コメント取得の 2 往復だった)
pub fn issue_comments(root: &Path, number: u64) -> Result<Vec<String>, String> {
    comments(root, "issue", number)
}

/// `o`: ブラウザで開く。実際にブラウザを起動するのは gh 自身で、fv 側は結果 (成功/失敗) だけ見る
pub fn open_issue_web(root: &Path, number: u64) -> Result<(), String> {
    run_gh(root, ["issue", "view", &number.to_string(), "--web"]).map(|_| ())
}

/// pull requests タブ (#34) の一覧行。`RemoteItem` (issues と共有) を PR 専用フィールドで
/// 汚さないよう、headRefName/isDraft はここに閉じて持つ。一覧の絞り込み・キャッシュ・描画は
/// `remotelist::ListRow` (title/state だけを見る) 越しに issues と同じ実装を再利用する
pub struct PrRow {
    pub item: RemoteItem,
    pub head_ref: String,
    pub is_draft: bool,
}

// 共通 6 フィールド + headRefName + isDraft + body の 9 フィールド。body は末尾に足しただけで
// 共通 6 個の並びは変えていない (remote_item がそのまま先頭だけ読める)
const PR_LIST_TEMPLATE: &str = r#"{{range .}}{{.number}}{{"\x00"}}{{.title}}{{"\x00"}}{{.author.login}}{{"\x00"}}{{.updatedAt}}{{"\x00"}}{{range .labels}}{{.name}},{{end}}{{"\x00"}}{{.state}}{{"\x00"}}{{.headRefName}}{{"\x00"}}{{.isDraft}}{{"\x00"}}{{.body}}{{"\x00"}}{{end}}"#;
const PR_LIST_JSON_FIELDS: &str =
    "number,title,author,updatedAt,labels,state,headRefName,isDraft,body";

// body を含まない従来幅 (8 フィールド) のフォールバック用テンプレート
const PR_LIST_TEMPLATE_LEGACY: &str = r#"{{range .}}{{.number}}{{"\x00"}}{{.title}}{{"\x00"}}{{.author.login}}{{"\x00"}}{{.updatedAt}}{{"\x00"}}{{range .labels}}{{.name}},{{end}}{{"\x00"}}{{.state}}{{"\x00"}}{{.headRefName}}{{"\x00"}}{{.isDraft}}{{"\x00"}}{{end}}"#;
const PR_LIST_JSON_FIELDS_LEGACY: &str =
    "number,title,author,updatedAt,labels,state,headRefName,isDraft";

/// `gh pr list` を叩き `PrRow` の一覧を返す。issues と同じ理由で常に `--state all` を
/// 1 回だけ取得し、`t` (open/closed/merged/all の循環) はローカルフィルタに閉じる。
/// body 込みの `--json` が失敗する古い gh 向けのフォールバックも issues と同じ形で持つ
/// (body の位置が issues と違い index 8 なのは PR_LIST_TEMPLATE 参照)
pub fn list_prs(root: &Path) -> Result<Vec<PrRow>, String> {
    let args = [
        "pr",
        "list",
        "--limit",
        "100",
        "--state",
        "all",
        "--json",
        PR_LIST_JSON_FIELDS,
        "--template",
        PR_LIST_TEMPLATE,
    ];
    if let Ok(stdout) = run_gh(root, args) {
        return Ok(parse_records(&stdout, 9)
            .into_iter()
            .filter_map(|c| {
                let mut item = remote_item(&c[..6])?;
                item.body = c[8].to_string();
                Some(PrRow {
                    item,
                    head_ref: c[6].to_string(),
                    is_draft: c[7] == "true",
                })
            })
            .collect());
    }
    let legacy_args = [
        "pr",
        "list",
        "--limit",
        "100",
        "--state",
        "all",
        "--json",
        PR_LIST_JSON_FIELDS_LEGACY,
        "--template",
        PR_LIST_TEMPLATE_LEGACY,
    ];
    let stdout = run_gh(root, legacy_args)?;
    Ok(parse_records(&stdout, 8)
        .into_iter()
        .filter_map(|c| {
            let item = remote_item(&c[..6])?;
            Some(PrRow {
                item,
                head_ref: c[6].to_string(),
                is_draft: c[7] == "true",
            })
        })
        .collect())
}

/// コメントだけを取得する。説明 (本文) は一覧取得の時点で `RemoteItem::body` に入っている
/// (issue_comments と同じ理由)
pub fn pr_comments(root: &Path, number: u64) -> Result<Vec<String>, String> {
    comments(root, "pr", number)
}

// `gh <kind> view <n> --comments` はコメント一覧だけを出す (本文は出ない)。コメントが 0 件でも
// 失敗ではなく空 Vec を返す — 呼び出し側 (component::issues / component::prs) が「(no comments)」を出し分ける
fn comments(root: &Path, kind: &str, number: u64) -> Result<Vec<String>, String> {
    let number = number.to_string();
    let stdout = run_gh(root, [kind, "view", &number, "--comments"])?;
    Ok(stdout.lines().map(str::to_string).collect())
}

/// `d`: 差分。出力は `git diff` と同じ unified diff 形式なので、行の組み立ては
/// 呼び出し側 (component/prs/mod.rs) が gitlane::render_commit にそのまま渡して再利用する
/// (GIT/LOG レーンの複数ファイル diff レンダラを 2 箇所に複製しない)
pub fn pr_diff(root: &Path, number: u64) -> Result<String, String> {
    run_gh(root, ["pr", "diff", &number.to_string()])
}

/// `S`: CI ステータス。`gh pr checks` は失敗中のチェックがあると非ゼロ終了するが、その場合も
/// stdout に一覧が出ているので、失敗を隠さずそのまま見せるため終了コードでは判定しない
pub fn pr_checks(root: &Path, number: u64) -> Result<Vec<String>, String> {
    let output = Command::new("gh")
        .args(["pr", "checks", &number.to_string()])
        .current_dir(root)
        .output();
    match output {
        Ok(output) if !output.stdout.is_empty() => Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()),
        Ok(output) if output.status.success() => Ok(Vec::new()),
        Ok(output) => Err(first_line(&output.stderr)),
        Err(_) => Err("gh コマンドが見つかりません".to_string()),
    }
}

/// `o`: ブラウザで開く
pub fn open_pr_web(root: &Path, number: u64) -> Result<(), String> {
    run_gh(root, ["pr", "view", &number.to_string(), "--web"]).map(|_| ())
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
