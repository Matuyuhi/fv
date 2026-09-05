use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::app::{App, Mode};
use crate::lang::{Msg, t};

// 破壊的・書き込み系操作の確認オーバーレイ。Lane と直交するので、どのレーンの上にも
// 同じ見た目で重ねる (Help/Settings と同じ centered popup パターン)
pub(super) fn draw_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Confirm { prompt, .. } = &app.mode else {
        return;
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title("confirm");

    // 対象パス・件数・untracked の有無を複数行で出す呼び出し元 (#25 discard/stash) があるため、
    // prompt 内の改行はそのまま複数行に割る (単一行の呼び出しは従来どおり 1 行のまま)
    let mut lines: Vec<Line> = prompt
        .lines()
        .map(|line| Line::from(line.to_string()))
        .collect();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("y", Style::default().fg(Color::Green)),
        Span::raw("/"),
        Span::styled("Enter", Style::default().fg(Color::Green)),
        Span::raw(t(Msg::ConfirmRun)),
        Span::styled("n", Style::default().fg(Color::Red)),
        Span::raw("/"),
        Span::styled("Esc", Style::default().fg(Color::Red)),
        Span::raw(t(Msg::ConfirmCancel)),
    ]));
    // 高さは端末に対する割合ではなく中身から決める。割合固定だと低い端末で
    // 「復元できません」の警告や y/n の操作行が黙って切れてしまい、確認の意味が消えるため。
    // Paragraph の Wrap で折り返る分も数に入れないと同じことが起きる
    let inner_width = (area.width * PERCENT_X / 100).saturating_sub(2).max(1);
    let rows: usize = lines
        .iter()
        .map(|line| wrapped_rows(&line_text(line), inner_width))
        .sum();
    let height = (rows as u16 + 2).min(area.height);
    let popup = crate::widget::centered_rect_with_height(PERCENT_X, height, area);
    frame.render_widget(Clear, popup);

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: true });
    frame.render_widget(paragraph, popup);
}

const PERCENT_X: u16 = 55;

// ratatui の Wrap は表示幅で折り返すので、ASCII 以外を 2 桁として概算する
// (text.rs の桁換算は diff の桁対応が目的で全角を 1 桁として扱うため、ここでは使わない)
fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn wrapped_rows(text: &str, width: u16) -> usize {
    let display: usize = text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    display.div_ceil(width as usize).max(1)
}
