use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Mode};

// 破壊的・書き込み系操作の確認オーバーレイ。Lane と直交するので、どのレーンの上にも
// 同じ見た目で重ねる (Help/Settings と同じ centered popup パターン)
pub(super) fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Confirm { prompt, .. } = &app.mode else {
        return;
    };
    let popup = super::centered_rect(50, 20, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title("confirm");

    let lines = vec![
        Line::from(prompt.clone()),
        Line::from(""),
        Line::from(vec![
            Span::styled("y", Style::default().fg(Color::Green)),
            Span::raw("/"),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(": 実行    "),
            Span::styled("n", Style::default().fg(Color::Red)),
            Span::raw("/"),
            Span::styled("Esc", Style::default().fg(Color::Red)),
            Span::raw(": 中止"),
        ]),
    ];
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup);
}
