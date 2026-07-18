use std::collections::HashSet;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Focus, InputKind, Mode};
use crate::finder::Finder;
use crate::git::FileStatus;
use crate::viewer::{Content, SearchState};

// 通常マッチ/カレントマッチのハイライト色
const MATCH_BG: Color = Color::Rgb(80, 80, 0);
const CURRENT_MATCH_BG: Color = Color::Rgb(255, 220, 0);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let full = frame.area();
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(full);
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(main);
    // マウスのヒットテスト用に、次の on_mouse で使えるよう書き戻す (viewport_height と同じパターン)
    app.tree_area = left;
    app.viewer_area = right;
    draw_tree(frame, app, left);
    draw_viewer(frame, app, right);
    draw_status_bar(frame, app, status);
    if matches!(app.mode, Mode::Finder(_)) {
        draw_finder(frame, app, full);
    }
}

fn pane_block(title: String, focused: bool) -> Block<'static> {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
}

fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = app
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.root.display().to_string());
    let git = app.git.as_ref();
    let items: Vec<ListItem> = app
        .tree
        .visible
        .iter()
        .map(|row| {
            let marker = if row.is_dir {
                if row.expanded {
                    "▾ "
                } else {
                    "▸ "
                }
            } else {
                "  "
            };
            // ディレクトリは git.files に直接エントリを持たないため自然に None になる
            let file_status = git.and_then(|g| g.files.get(&row.path).copied());
            let prefix = file_status.map(status_prefix).unwrap_or("");
            let label = format!("{}{}{}{}", "  ".repeat(row.depth), marker, prefix, row.name);
            let style = if row.is_dir {
                let has_changes = git.is_some_and(|g| g.changed_dirs.contains(&row.path));
                let color = if has_changes { Color::Yellow } else { Color::Blue };
                Style::default().fg(color)
            } else if let Some(status) = file_status {
                Style::default()
                    .fg(status_color(status))
                    .add_modifier(Modifier::DIM)
            } else {
                Style::default()
            };
            ListItem::new(label).style(style)
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, app.focus == Focus::Tree))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let selected = (!app.tree.visible.is_empty()).then_some(app.tree.selected);
    app.tree.list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut app.tree.list_state);
}

// ツリーの行頭に置く1文字+空白のマーカー
fn status_prefix(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Modified => "M ",
        FileStatus::Added => "A ",
        FileStatus::Untracked => "? ",
        FileStatus::Deleted => "D ",
        FileStatus::Renamed => "R ",
    }
}

fn status_color(status: FileStatus) -> Color {
    match status {
        FileStatus::Modified => Color::Yellow,
        FileStatus::Added | FileStatus::Untracked | FileStatus::Renamed => Color::Green,
        FileStatus::Deleted => Color::Red,
    }
}

fn draw_viewer(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Viewer;
    let inner_height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport_height = inner_height;
    // 罫線分のみを引いた概算値。hscroll の緩いクランプにしか使わないので gutter 幅までは引かない
    app.viewer.viewport_width = area.width.saturating_sub(2) as usize;

    let Some(open) = &app.viewer.current else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("viewer"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    // hscroll > 0 の間はステータスバーではなくタイトル側に現在オフセットを出す
    let title = if !app.viewer.wrap && app.viewer.hscroll > 0 {
        format!("{}  →{}", open.title, app.viewer.hscroll)
    } else {
        open.title.clone()
    };
    let block = pane_block(title, focused);
    match open.content.as_ref() {
        Content::Text { lines, .. } => {
            // Paragraph::scroll は u16 上限で巨大ファイルに届かないため、
            // 表示範囲を自前でスライスして先頭から描画する
            let start = app.viewer.scroll.min(lines.len().saturating_sub(1));
            let end = (start + inner_height).min(lines.len());
            let wrap = app.viewer.wrap;
            let hscroll = app.viewer.hscroll;
            let visible: Vec<Line> = (start..end)
                .map(|i| {
                    let line = mark_changed_line(&lines[i], i, &open.changed_lines);
                    let line = highlight_matches(&line, i, &app.viewer.search);
                    // シフトは最後に適用する。検索ハイライトの bg 計算は元の桁位置基準なので、
                    // 先にシフトすると global col がずれてマッチと違う文字に色が付いてしまう
                    if wrap {
                        line
                    } else {
                        hscroll_line(&line, hscroll)
                    }
                })
                .collect();
            let mut paragraph = Paragraph::new(visible)
                .block(block)
                .style(Style::default().bg(app.viewer.background()));
            if wrap {
                paragraph = paragraph.wrap(Wrap { trim: false });
            }
            frame.render_widget(paragraph, area);
        }
        Content::Binary => {
            let paragraph = Paragraph::new("binary file")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(paragraph, area);
        }
        Content::Error(message) => {
            let paragraph = Paragraph::new(message.as_str())
                .block(block)
                .style(Style::default().fg(Color::Red));
            frame.render_widget(paragraph, area);
        }
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Input {
            kind: InputKind::Search,
            buffer,
        } => search_input_line(buffer),
        Mode::Input {
            kind: InputKind::Goto,
            buffer,
        } => goto_input_line(buffer),
        Mode::Finder(_) => Line::from("Enter: open  Esc: close"),
        Mode::Normal => normal_status_line(app),
    };
    let paragraph =
        Paragraph::new(line).style(Style::default().fg(Color::White).bg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

fn search_input_line(buffer: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("/{buffer}")),
        // 常に末尾に立つ簡易カーソル (このアプリの入力は末尾への追記のみ)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ])
}

fn goto_input_line(buffer: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!(":{buffer}")),
        // 常に末尾に立つ簡易カーソル (このアプリの入力は末尾への追記のみ)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
    ])
}

fn normal_status_line(app: &App) -> Line<'static> {
    // g 待ち状態は vim の pending 表示相当。他のステータスより優先して出す
    if app.pending_g {
        return Line::from("g");
    }
    if let Some(search) = &app.viewer.search {
        if let Some(current) = search.current {
            return Line::from(format!(
                "「{}」 {}/{}  n: next  N: prev  Tab: focus  q: quit",
                search.query,
                current + 1,
                search.matches.len()
            ));
        }
    }
    let hint = match app.focus {
        Focus::Tree => {
            "j/k: move  h/l: collapse/expand  H: to parent  gg/G: top/bottom  r: rescan  Enter: open  Tab: focus  q: quit"
        }
        Focus::Viewer => {
            "j/k: scroll  w: wrap  h/l: hscroll  gg/G: top/bottom  /: search  n/N: match  :N: goto  Ctrl+d/u: page  Ctrl+o/i: history  Tab: focus  q: quit"
        }
    };
    Line::from(hint)
}

// 画面中央に percent_x% x percent_y% のオーバーレイ領域を切り出す
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let [_, middle, _] = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .areas(area);
    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(middle);
    center
}

fn draw_finder(frame: &mut Frame, app: &mut App, area: Rect) {
    let Mode::Finder(finder) = &mut app.mode else {
        return;
    };
    let popup = centered_rect(60, 60, area);
    // 下のツリー/ビューアを隠すため、描画前に領域をクリアする
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("finder (Ctrl+p)");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [input_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    draw_finder_input(frame, finder, input_area);
    draw_finder_list(frame, finder, list_area);
}

fn draw_finder_input(frame: &mut Frame, finder: &Finder, area: Rect) {
    let query_text = format!("> {}", finder.query);
    let count = format!("{}/{}", finder.matches.len(), finder.total());
    // 入力とカーソルの右側を件数表示までスペースで埋める。入力欄がそれより
    // 狭ければ埋めず単純に連結するだけにする (折り返しは Paragraph に任せる)
    let used = query_text.chars().count() + 1 + count.chars().count();
    let pad = (area.width as usize).saturating_sub(used);
    let line = Line::from(vec![
        Span::raw(query_text),
        // 常に末尾に立つ簡易カーソル (他の入力行と同じ表現)
        Span::styled(" ", Style::default().add_modifier(Modifier::REVERSED)),
        Span::raw(" ".repeat(pad)),
        Span::styled(count, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_finder_list(frame: &mut Frame, finder: &mut Finder, area: Rect) {
    let items: Vec<ListItem> = finder
        .matches
        .iter()
        .map(|m| {
            let path = finder.candidate_path(m.candidate).unwrap_or_default();
            ListItem::new(Line::from(highlight_finder_match(path, &m.positions)))
        })
        .collect();
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let selected = (!finder.matches.is_empty()).then_some(finder.selected);
    finder.list_state.select(selected);
    frame.render_stateful_widget(list, area, &mut finder.list_state);
}

// gutter span (span[0]) の末尾1文字 (常に半角空白) を変更行マーカーに置き換えた
// 新しい Line を返す。キャッシュ済みの Line 自体は変更しない。span 数・各 span の
// 文字数はどちらも変わらないため、highlight_matches の列計算 (span[0] を除外して
// col=0 から数える) には影響しない
fn mark_changed_line(line: &Line<'static>, line_idx: usize, changed: &Option<HashSet<usize>>) -> Line<'static> {
    let is_changed = changed
        .as_ref()
        .is_some_and(|lines| lines.contains(&(line_idx + 1)));
    if !is_changed {
        return line.clone();
    }
    let Some(gutter) = line.spans.first() else {
        return line.clone();
    };
    let mut text = gutter.content.to_string();
    text.pop();
    text.push('▎');

    let mut spans = Vec::with_capacity(line.spans.len());
    spans.push(Span::styled(text, Style::default().fg(Color::Cyan)));
    spans.extend(line.spans.iter().skip(1).cloned());
    Line::from(spans)
}

// gutter (span[0]) は固定したまま、コンテンツ部分だけ hscroll 文字分左にシフトした
// 新しい Line を返す。highlight_matches 適用後に呼ぶことで、シフトで画面外に落ちる文字ごと
// その bg スタイルも一緒に捨てられ、残った文字のハイライトは桁がずれず正しく残る
fn hscroll_line(line: &Line<'static>, hscroll: usize) -> Line<'static> {
    if hscroll == 0 {
        return line.clone();
    }
    let mut spans = Vec::with_capacity(line.spans.len());
    if let Some(gutter) = line.spans.first() {
        spans.push(gutter.clone());
    }
    let mut col = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let span_end = col + chars.len();
        if span_end <= hscroll {
            // span 全体が切り捨て範囲に収まる場合は丸ごと捨てる
            col = span_end;
            continue;
        }
        let skip = hscroll.saturating_sub(col);
        let segment: String = chars[skip..].iter().collect();
        spans.push(Span::styled(segment, span.style));
        col = span_end;
    }
    Line::from(spans)
}

// マッチした char (positions は昇順) を強調色で塗った span 列を組み立てる
fn highlight_finder_match(path: &str, positions: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut pos_iter = positions.iter().peekable();
    for (i, ch) in path.chars().enumerate() {
        let style = if pos_iter.peek() == Some(&&i) {
            pos_iter.next();
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

// キャッシュ済み span 列に背景色を重ねた新しい Line を組み立てる (キャッシュ自体は変更しない)
fn highlight_matches(line: &Line<'static>, line_idx: usize, search: &Option<SearchState>) -> Line<'static> {
    let Some(search) = search else {
        return line.clone();
    };
    let ranges: Vec<(usize, usize, bool)> = search
        .matches
        .iter()
        .enumerate()
        .filter(|(_, m)| m.line == line_idx)
        .map(|(i, m)| (m.start_col, m.end_col, Some(i) == search.current))
        .collect();
    if ranges.is_empty() {
        return line.clone();
    }

    // span[0] は行番号 gutter なのでハイライト対象から除外し、そのまま引き継ぐ
    let mut spans = Vec::with_capacity(line.spans.len());
    if let Some(gutter) = line.spans.first() {
        spans.push(gutter.clone());
    }

    let mut col = 0usize;
    for span in line.spans.iter().skip(1) {
        let chars: Vec<char> = span.content.chars().collect();
        let mut idx = 0usize;
        while idx < chars.len() {
            let global = col + idx;
            match ranges.iter().find(|(s, e, _)| *s <= global && global < *e) {
                Some(&(_, end, current)) => {
                    let seg_end = (end - col).min(chars.len());
                    let segment: String = chars[idx..seg_end].iter().collect();
                    let bg = if current { CURRENT_MATCH_BG } else { MATCH_BG };
                    let mut style = span.style.bg(bg);
                    if current {
                        style = style.fg(Color::Black);
                    }
                    spans.push(Span::styled(segment, style));
                    idx = seg_end;
                }
                None => {
                    let next_start = ranges
                        .iter()
                        .map(|(s, _, _)| *s)
                        .filter(|&s| s > global)
                        .min();
                    let seg_end = match next_start {
                        Some(s) => (s - col).min(chars.len()),
                        None => chars.len(),
                    };
                    let segment: String = chars[idx..seg_end].iter().collect();
                    spans.push(Span::styled(segment, span.style));
                    idx = seg_end;
                }
            }
        }
        col += chars.len();
    }
    Line::from(spans)
}
