use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Focus, InputKind, Mode};

pub(super) fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
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
        Mode::Help => Line::from("?: close"),
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
                "「{}」 {}/{}  n: next  N: prev  Tab: focus  q: quit  ?: help",
                search.query,
                current + 1,
                search.matches.len()
            ));
        }
    }
    // 狭い端末でも収まるよう常用キーのみに絞る。全キーは ? のヘルプに任せる
    let hint = match app.focus {
        Focus::Tree => "j/k: move  h/l: collapse/expand  Enter: open  Tab: focus  q: quit  ?: help",
        Focus::Viewer => "j/k: scroll  w: wrap  /: search  Tab: focus  q: quit  ?: help",
    };
    Line::from(hint)
}
