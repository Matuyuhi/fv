use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus, InputKind, Mode};
use crate::viewer::{Content, SearchState};

// 通常マッチ/カレントマッチのハイライト色
const MATCH_BG: Color = Color::Rgb(80, 80, 0);
const CURRENT_MATCH_BG: Color = Color::Rgb(255, 220, 0);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [main, status] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)]).areas(main);
    draw_tree(frame, app, left);
    draw_viewer(frame, app, right);
    draw_status_bar(frame, app, status);
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
            let label = format!("{}{}{}", "  ".repeat(row.depth), marker, row.name);
            let style = if row.is_dir {
                Style::default().fg(Color::Blue)
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

fn draw_viewer(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Viewer;
    let inner_height = area.height.saturating_sub(2) as usize;
    app.viewer.viewport_height = inner_height;

    let Some(open) = &app.viewer.current else {
        let paragraph = Paragraph::new("no file selected")
            .block(pane_block(String::from("viewer"), focused))
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(paragraph, area);
        return;
    };
    let block = pane_block(open.title.clone(), focused);
    match open.content.as_ref() {
        Content::Text { lines, .. } => {
            // Paragraph::scroll は u16 上限で巨大ファイルに届かないため、
            // 表示範囲を自前でスライスして先頭から描画する
            let start = app.viewer.scroll.min(lines.len().saturating_sub(1));
            let end = (start + inner_height).min(lines.len());
            let visible: Vec<Line> = (start..end)
                .map(|i| highlight_matches(&lines[i], i, &app.viewer.search))
                .collect();
            let paragraph = Paragraph::new(visible)
                .block(block)
                .style(Style::default().bg(app.viewer.background()));
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
        Focus::Tree => "j/k: move  Enter: open/expand  Tab: focus  q: quit",
        Focus::Viewer => {
            "j/k: scroll  Ctrl+d/u: page  gg/G: top/bottom  /: search  :: goto  Tab: focus  q: quit"
        }
    };
    Line::from(hint)
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
