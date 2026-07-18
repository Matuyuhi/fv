use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem};

use crate::app::{App, Focus};
use crate::git::FileStatus;

use super::pane_block;

pub(super) fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
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
                if row.expanded { "▾ " } else { "▸ " }
            } else {
                "  "
            };
            // ディレクトリは git.files に直接エントリを持たないため自然に None になる
            let file_status = git.and_then(|g| g.files.get(&row.path).copied());
            let prefix = file_status.map(status_prefix).unwrap_or("");
            let label = format!("{}{}{}{}", "  ".repeat(row.depth), marker, prefix, row.name);
            let style = if row.is_dir {
                let has_changes = git.is_some_and(|g| g.changed_dirs.contains(&row.path));
                let color = if has_changes {
                    Color::Yellow
                } else {
                    Color::Blue
                };
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
