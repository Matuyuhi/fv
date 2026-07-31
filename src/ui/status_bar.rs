use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Focus, InputKind, Lane, Mode, Workspace};
use crate::editor::EditState;
use crate::logview::LogState;

pub(super) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    // レーンのセグメントは常に先頭に出す。Claude Code のモード表示と同じく
    // 「今どこにいるか」と「Shift+Tab で次に何が来るか」を同時に見せるため
    let mut spans = lane_segments(app);
    // ブランチ + ahead/behind は issue #26 の要求通り GIT レーン以外・どの Mode 中でも常時出す
    spans.extend(branch_segment(app));
    spans.extend(hint_line(app).spans);
    let paragraph = Paragraph::new(Line::from(spans))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
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
    let available = [
        true,
        app.viewer.is_text(),
        app.git_available(),
        app.log_available(),
    ];
    let mut spans = Vec::with_capacity(Lane::LABELS.len() + 1);
    for (i, label) in Lane::LABELS.iter().enumerate() {
        let style = if i == current && in_viewer_workspace {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else if available[i] && in_viewer_workspace {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::Gray).add_modifier(Modifier::DIM)
        };
        let text = if i == current && in_viewer_workspace {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        };
        spans.push(Span::styled(text, style));
    }
    spans.push(Span::raw("  "));
    spans
}

fn hint_line(app: &App) -> Line<'static> {
    match &app.mode {
        Mode::Input {
            kind: InputKind::Search,
            buffer,
        } => input_line('/', buffer),
        Mode::Input {
            kind: InputKind::Goto,
            buffer,
        } => input_line(':', buffer),
        Mode::Finder(_) => Line::from("Enter: open  Esc: close"),
        Mode::Branch(_) => Line::from("Enter: switch  Ctrl+n: new branch  Esc: close"),
        Mode::Help => Line::from("?: close"),
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
                Lane::Log(state) => log_status_line(app, state),
                Lane::View => normal_status_line(app),
            }
        }
    }
}

// Issues/PR タブ滞在中のヒント。中身はまだプレースホルダなので出せるキーだけを示す
fn workspace_status_line(_app: &App) -> Line<'static> {
    Line::from("Ctrl+t / Alt+1..3: タブ切替  s: 設定  q: 終了  ?: help")
}

fn confirm_line(prompt: &str) -> Line<'static> {
    Line::from(format!("{prompt}  y/Enter: 実行  n/Esc: 中止"))
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
    Line::from(format!("{title}  Enter: 改行  Ctrl+s: 確定  Esc: 閉じる"))
}

// 実行中は他の操作 (スクロール等) を妨げない旨も添えて、固まったのではないと分かるようにする
fn remote_job_line(job: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("{job} 実行中… (他の操作は続けられます)"),
        Style::default().fg(Color::Yellow),
    ))
}

fn notice_line(message: &str, is_error: bool) -> Line<'static> {
    let color = if is_error { Color::Red } else { Color::Green };
    Line::from(Span::styled(
        message.to_string(),
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
    Line::from(format!(
        "{}:{}  Ctrl+s: save  Ctrl+z/y: undo/redo  Ctrl+k: delete line  Esc: exit",
        state.cursor.0 + 1,
        state.cursor.1 + 1
    ))
}

fn git_status_line(app: &App) -> Line<'static> {
    if app.pending_g {
        return Line::from("g");
    }
    let hint = match app.focus {
        Focus::Tree => {
            "j/k: move  h/l: collapse/expand  Enter: diff  Tab: focus  Shift+Tab: mode  ?: help"
        }
        Focus::Viewer => "j/k: scroll  n/N: hunk  w: wrap  Tab: focus  Shift+Tab: mode  ?: help",
    };
    Line::from(format!("{} changes  {hint}", app.tree.visible_files()))
}

fn log_status_line(app: &App, log: &LogState) -> Line<'static> {
    if app.pending_g {
        return Line::from("g");
    }
    let hint = match app.focus {
        Focus::Tree => {
            "j/k: move  Enter/l: diff  gg/G: top/bottom  Tab: focus  Shift+Tab: mode  ?: help"
        }
        Focus::Viewer => "j/k: scroll  n/N: hunk  w: wrap  Tab: focus  Shift+Tab: mode  ?: help",
    };
    Line::from(format!("{} commits  {hint}", log.commits().len()))
}

fn normal_status_line(app: &App) -> Line<'static> {
    // g 待ち状態は vim の pending 表示相当。他のステータスより優先して出す
    if app.pending_g {
        return Line::from("g");
    }
    if let Some(search) = &app.viewer.search
        && let Some(current) = search.current
    {
        return Line::from(format!(
            "「{}」 {}/{}  n: next  N: prev  Tab: focus  q: quit  ?: help",
            search.query,
            current + 1,
            search.matches.len()
        ));
    }
    // 狭い端末でも収まるよう常用キーのみに絞る。全キーは ? のヘルプに任せる
    let hint = match app.focus {
        Focus::Tree => {
            "j/k: move  h/l: collapse/expand  a: hidden  s: settings  Shift+Tab: mode  q: quit  ?: help"
        }
        Focus::Viewer => {
            "j/k: scroll  w: wrap  /: search  e: edit  Shift+Tab: mode  q: quit  ?: help"
        }
    };
    Line::from(hint)
}
