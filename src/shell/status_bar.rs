use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus, InputKind, Lane, Mode, Workspace};
use crate::component::editor::EditState;
use crate::component::log::LogState;
use crate::lang::{Msg, t};

pub(super) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    // レーンのセグメントは常に先頭に出す。Claude Code のモード表示と同じく
    // 「今どこにいるか」と「Shift+Tab で次に何が来るか」を同時に見せるため
    let mut spans = lane_segments(app);
    // ブランチ + ahead/behind は issue #26 の要求通り GIT レーン以外・どの Mode 中でも常時出す
    spans.extend(branch_segment(app));
    spans.extend(hint_line(app).spans);
    let paragraph = Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::White));
    frame.render_widget(paragraph, area);
}

// 現在ブランチ + ahead/behind。非 git repo (branch_status が None) では何も出さず、
// detached HEAD は短縮 SHA をそのまま名前として見せる (git::branch_status が既に解決済み)
fn branch_segment(app: &App) -> Vec<Span<'static>> {
    let Some(status) = &app.branch_status else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    let name = if status.detached {
        format!("{} (detached)", status.name)
    } else {
        status.name.clone()
    };
    spans.push(Span::styled(name, Style::default().fg(Color::Magenta)));
    if status.has_upstream {
        spans.push(Span::styled(
            format!(" ↑{} ↓{}", status.ahead, status.behind),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::raw("  "));
    spans
}

// 現在レーンは [ ] 付きの反転、入れないレーン (非テキストの EDIT / 非 git repo の GIT) は暗く出す。
// Issues/PR タブ滞在中は Lane の概念が無い (Shift+Tab も無効) ので全セグメントを暗くする
fn lane_segments(app: &App) -> Vec<Span<'static>> {
    let in_viewer_workspace = matches!(app.workspace, Workspace::Viewer);
    let current = app.lane.index();
    let available = [true, app.viewer.is_text(), app.git_available()];
    let mut spans = Vec::with_capacity(Lane::LABELS.len() + 1);
    for (i, label) in Lane::LABELS.iter().enumerate() {
        let style = if i == current && in_viewer_workspace {
            // Color::White は ANSI の bright white (97) に落ちるため、端末テーマ次第で灰色寄りになり
            // 明るい背景の上で白く見えない。現在レーンは最も目立たせたいので RGB で真っ白に固定する
            Style::new().bg(Color::Green).fg(Color::White)
        } else if available[i] && in_viewer_workspace {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM)
        };
        let text = format!(" {label} ");
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::raw("  "));
    spans
}

fn hint_line(app: &App) -> Line<'static> {
    match &app.mode {
        // Filter (issues/PR 一覧の絞り込み) も Search と同じ `/` プレフィックスで見せる
        Mode::Input {
            kind: InputKind::Search | InputKind::Filter,
            buffer,
        } => input_line('/', buffer),
        Mode::Input {
            kind: InputKind::Goto,
            buffer,
        } => input_line(':', buffer),
        Mode::Finder(_) => Line::from("Enter: open  Esc: close"),
        Mode::Grep => Line::from("Enter: open  ↑/↓: select  Ctrl+u: clear  Esc: close"),
        Mode::Branch(_) => Line::from("Enter: switch  Ctrl+n: new branch  Esc: close"),
        Mode::Help { .. } => Line::from("j/k: scroll  Ctrl+d/u: page  gg/G: top/bottom  ?: close"),
        Mode::Settings(_) => Line::from("j/k: select  h/l/Enter: change  s: close"),
        Mode::Confirm { prompt, .. } => confirm_line(prompt),
        Mode::Commit { amend, error, .. } => commit_line(*amend, error.as_deref()),
        Mode::Normal => {
            // 実行中のリモート操作 (f/p/P) は「実行中である」こと自体を見落とさせないよう
            // 一時通知よりさらに優先して見せる (終わるまで他のヒントより上に居座らせる)
            if let Some(job) = app.running_remote_job() {
                return remote_job_line(job);
            }
            // App 全体の一時通知はどのタブ・レーンでも他のヒントより優先して見せる
            // (EditState.notice は EDIT レーン専用なので edit_status_line 側に残す)
            if let Some((message, _, is_error)) = &app.notice {
                return notice_line(message, *is_error);
            }
            if !matches!(app.workspace, Workspace::Viewer) {
                return workspace_status_line(app);
            }
            match &app.lane {
                Lane::Edit(state) => edit_status_line(state),
                Lane::Git(_) => git_status_line(app),
                // コミット一覧は VIEW の中に同居するので、ヒントもフォーカス側で振り分ける
                // (normal_status_line が Focus::Log と「右ペインが diff の時」を見る)
                Lane::View => normal_status_line(app),
            }
        }
    }
}

// Issues/PR タブ滞在中のヒント
fn workspace_status_line(app: &App) -> Line<'static> {
    match app.workspace {
        Workspace::Issues => issues_status_line(app),
        Workspace::PullRequests => pr_status_line(app),
        Workspace::Viewer => Line::from(t(Msg::StatusWorkspaceHint)),
    }
}

fn issues_status_line(app: &App) -> Line<'static> {
    if app.pending_g {
        return Line::from("g");
    }
    if app.issues.list_loading() && !app.issues.fetched() {
        return Line::from(t(Msg::StatusLoadingIssues));
    }
    if let Some(err) = app.issues.list_error() {
        return Line::from(Span::styled(
            crate::tr!(Msg::StatusIssuesFetchFailed, err),
            Style::default().fg(Color::Red),
        ));
    }
    let hint = match app.focus {
        Focus::Tree | Focus::Log => {
            "j/k: move  Enter/l: open  /: filter  t: state  o: web  r: refresh  Tab: focus  ?: help"
        }
        Focus::Viewer => "j/k: scroll  o: web  Tab: focus  ?: help",
    };
    Line::from(format!(
        "{}/{} issues [{}]  {hint}",
        app.issues.visible_count(),
        app.issues.total(),
        app.issues.state_filter.label()
    ))
}

fn pr_status_line(app: &App) -> Line<'static> {
    if app.pending_g {
        return Line::from("g");
    }
    if app.prs.list_loading() && !app.prs.fetched() {
        return Line::from(t(Msg::StatusLoadingPullRequests));
    }
    if let Some(err) = app.prs.list_error() {
        return Line::from(Span::styled(
            crate::tr!(Msg::StatusPrsFetchFailed, err),
            Style::default().fg(Color::Red),
        ));
    }
    let hint = match app.focus {
        Focus::Tree | Focus::Log => {
            "j/k: move  Enter/l: open  d: diff  S: checks  /: filter  t: state  o: web  r: refresh  Tab: focus  ?: help"
        }
        Focus::Viewer => {
            "j/k: cursor (diff)  d: diff  S: checks  ]/[: hunk (diff)  w: wrap (diff)  Tab: focus  ?: help"
        }
    };
    Line::from(format!(
        "{}/{} pull requests [{}]  {hint}",
        app.prs.visible_count(),
        app.prs.total(),
        app.prs.state_filter.label()
    ))
}

fn confirm_line(prompt: &str) -> Line<'static> {
    Line::from(crate::tr!(Msg::StatusConfirm, prompt))
}

// エラー (pre-commit hook 失敗など) は本文中の同じオーバーレイにも出るが、
// ステータスバー側にも要約を出して見落としを防ぐ
fn commit_line(amend: bool, error: Option<&str>) -> Line<'static> {
    if let Some(error) = error {
        return Line::from(Span::styled(
            format!("commit failed: {error}"),
            Style::default().fg(Color::Red),
        ));
    }
    let title = if amend { "amend commit" } else { "commit" };
    Line::from(crate::tr!(Msg::StatusCommit, title))
}

// 実行中は他の操作 (スクロール等) を妨げない旨も添えて、固まったのではないと分かるようにする
fn remote_job_line(job: &str) -> Line<'static> {
    Line::from(Span::styled(
        crate::tr!(Msg::StatusRemoteJobRunning, job),
        Style::default().fg(Color::Yellow),
    ))
}

fn notice_line(message: &str, is_error: bool) -> Line<'static> {
    let (icon, color) = if is_error {
        ("⚠ ", Color::Red)
    } else {
        ("✓ ", Color::Green)
    };
    Line::from(Span::styled(
        format!("{icon}{message}"),
        Style::default().fg(color),
    ))
}

fn input_line(prefix: char, buffer: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{prefix}{buffer}")),
        // 常に末尾に立つ簡易カーソル (このアプリの入力は末尾への追記のみ)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ])
}

fn edit_status_line(state: &EditState) -> Line<'static> {
    // 保存エラー・discard 確認は通常のキーヒントより優先して見せる
    if let Some(notice) = &state.notice {
        return Line::from(notice.clone());
    }
    Line::from(crate::tr!(
        Msg::StatusEdit,
        line = state.cursor.0 + 1,
        col = state.cursor.1 + 1
    ))
}

fn git_status_line(app: &App) -> Line<'static> {
    if app.pending_g {
        return Line::from("g");
    }
    let Lane::Git(git) = &app.lane else {
        return Line::from("");
    };
    // 検索確定中は他のヒントより優先して出す (normal_status_line の VIEW 検索と同じ扱い)
    if let Some(search) = git.search()
        && let Some(current) = search.current
    {
        return Line::from(crate::tr!(
            Msg::StatusGitSearch,
            query = search.query,
            current = current + 1,
            total = search.matches.len()
        ));
    }
    let hint = match app.focus {
        // GIT レーンにコミット一覧は出ない (L は VIEW 限定) ので Log はツリー扱いで足りる
        Focus::Tree | Focus::Log => {
            "j/k: move  h/l: collapse/expand  Enter: diff  Tab: focus  Shift+Tab: mode  ?: help"
                .to_string()
        }
        Focus::Viewer => {
            // Space/Enter の向きは diff 基準で決まるので、押す前にどちらになるかを出す
            let verb = if git.unstaging() { "unstage" } else { "stage" };
            // 選択中は他のヒントより優先して出す (VIEW の範囲選択と同じ扱い)。
            // 何行掴んでいるかは帯の色だけでは画面外へ伸びた分まで追えない
            let mut hint = match git.selected_row_count() {
                Some(rows) => {
                    crate::tr!(Msg::StatusGitLinesSelected, rows, verb)
                }
                None => "j/k: cursor  ]/[: hunk".to_string(),
            };
            if !git.showing_all() && git.selected_row_count().is_none() {
                hint.push_str(&format!("  Space: {verb} hunk"));
                // Enter/V は行単位ステージが効く表示でだけ案内する (side-by-side・まとめ
                // 表示では current_line_patch が必ず断るので、出すと可否と食い違う)
                if git.line_selection_available() {
                    hint.push_str(&format!("  Enter: {verb} line  V: select"));
                }
            }
            hint.push_str("  /: search  A: all files");
            if git.showing_all() {
                hint.push_str("  }/{: file");
            }
            hint.push_str("  w: wrap  Tab: focus  Shift+Tab: mode  ?: help");
            hint
        }
    };
    Line::from(format!("{} changes  {hint}", app.tree.visible_files()))
}

// コミット一覧ペインにフォーカスがある間のヒント
fn log_list_status_line(log: &LogState) -> Line<'static> {
    Line::from(format!(
        "{} commits  j/k: move  Enter/l: diff  gg/G: top/bottom  L/Esc: close  Tab: focus  ?: help",
        log.commits().len()
    ))
}

// 右ペインにコミット diff を出している間のヒント
fn log_diff_status_line() -> Line<'static> {
    Line::from("j/k: cursor  ]/[: hunk  w: wrap  Esc: close diff  Tab: focus  ?: help")
}

fn normal_status_line(app: &App) -> Line<'static> {
    // g 待ち状態は vim の pending 表示相当。他のステータスより優先して出す
    if app.pending_g {
        return Line::from("g");
    }
    // コミット一覧ペイン (`L`) が絡む文脈は専用のヒントに振り分ける。**選択・検索より前に
    // 見る**のが要点で、どちらも viewer (ファイル表示) に紐づく状態なのに残り続けるため、
    // 後ろに置くと「検索したまま L を押す」だけでキーの宛先 (コミット一覧) とヒント (検索) が
    // 食い違う。パネルを出していない間はここを素通りするので従来の見え方は変わらない
    if app.focus == Focus::Log
        && let Some(log) = &app.log
    {
        return log_list_status_line(log);
    }
    if app.focus == Focus::Viewer && app.showing_commit_diff() {
        return log_diff_status_line();
    }
    // 選択中は他のヒントより優先して出す。y を押すまで何行取れるのかが見えないと、
    // マウスのドラッグで「どこまで掴めているか」を確かめる手段が色だけになる
    if let Some(lines) = app.viewer.selected_line_count() {
        return Line::from(format!(
            "selected {lines} lines  y: copy  Esc: clear  Y: copy whole file"
        ));
    }
    if let Some(search) = &app.viewer.search
        && let Some(current) = search.current
    {
        return Line::from(crate::tr!(
            Msg::StatusViewSearch,
            query = search.query,
            current = current + 1,
            total = search.matches.len()
        ));
    }
    // 狭い端末でも収まるよう常用キーのみに絞る。全キーは ? のヘルプに任せる
    let hint = match app.focus {
        Focus::Tree | Focus::Log => {
            "j/k: move  h/l: collapse/expand  a: hidden  L: log  s: settings  Shift+Tab: mode  ?: help"
        }
        Focus::Viewer => {
            "j/k: cursor  w: wrap  /: search  v: select  y: copy  e: edit  L: log  Shift+Tab: mode  ?: help"
        }
    };
    Line::from(hint)
}
