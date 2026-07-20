use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::app::{App, Mode, SETTINGS_ROWS};

pub(super) fn draw_settings(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Settings(state) = &app.mode else {
        return;
    };
    let popup = super::centered_rect(50, 40, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("settings");
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let [list_area, hint_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let values = [
        on_off(app.tree.show_hidden()),
        on_off(app.icons),
        on_off(app.viewer.viewport.wrap),
        app.viewer.theme_name().to_string(),
    ];
    let items: Vec<ListItem> = SETTINGS_ROWS
        .iter()
        .zip(&values)
        .map(|(label, value)| {
            ListItem::new(Line::from(vec![
                Span::raw(format!("{label:<16}")),
                Span::styled(value.clone(), Style::default().fg(Color::Yellow)),
            ]))
        })
        .collect();
    let list = List::new(items).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let mut list_state = ListState::default().with_selected(Some(state.selected));
    frame.render_stateful_widget(list, list_area, &mut list_state);

    let hint = Paragraph::new("j/k 選択  h/l/Enter 変更  s/Esc 閉じる")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, hint_area);
}

fn on_off(v: bool) -> String {
    if v {
        "on".to_string()
    } else {
        "off".to_string()
    }
}
