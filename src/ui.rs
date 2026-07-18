use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};
use crate::viewer::Content;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .areas(frame.area());
    draw_tree(frame, app, left);
    draw_viewer(frame, app, right);
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
        Content::Text { lines } => {
            // Paragraph::scroll は u16 上限で巨大ファイルに届かないため、
            // 表示範囲を自前でスライスして先頭から描画する
            let start = app.viewer.scroll.min(lines.len().saturating_sub(1));
            let end = (start + inner_height).min(lines.len());
            let visible: Vec<Line> = lines[start..end].to_vec();
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
