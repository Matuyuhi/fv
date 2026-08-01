use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Lane, Workspace};

// GitHub モード有効時だけ現れるヘッダ 1 行。ui::draw が workspace_available のときだけ呼ぶ
pub(super) fn draw_tab_bar(frame: &mut Frame, app: &mut App, area: Rect) {
    let current = app.workspace.index();
    // 未保存の編集バッファはタブを跨いでも保持される (App::cycle_lane 参照) ので、
    // viewer タブ側に離れていることが分かるようマークを出す
    let viewer_dirty = matches!(&app.lane, Lane::Edit(state) if state.buffer.dirty());

    let mut spans = Vec::with_capacity(Workspace::LABELS.len() * 2);
    // クリック判定用に各タブの矩形を書き戻す (ui → app の既存パターン、mouse.rs が読む)
    let mut areas = [Rect::default(); Workspace::LABELS.len()];
    let mut x = area.x;
    for (i, label) in Workspace::LABELS.iter().enumerate() {
        let mut text = (*label).to_string();
        if i == 0 && viewer_dirty {
            text.push_str(" ●");
        }
        let text = if i == current {
            format!("[{text}]")
        } else {
            format!(" {text} ")
        };
        let width = text.chars().count() as u16;
        areas[i] = Rect {
            x,
            y: area.y,
            width: width.min(area.right().saturating_sub(x)),
            height: area.height,
        };
        x = x.saturating_add(width).saturating_add(1);
        let style = if i == current {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    app.tab_areas = areas;

    let paragraph = Paragraph::new(Line::from(spans))
        .style(Style::default().fg(Color::White).bg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}
