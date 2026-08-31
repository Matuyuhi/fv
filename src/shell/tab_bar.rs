use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Lane, Workspace};

// GitHub モード有効時だけ現れるヘッダ 1 行。shell::draw が workspace_available のときだけ呼ぶ
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
        // Add the keyboard shortcut hint (1, 2, 3...) to the tab label
        let mut text = format!("{}: {}", i + 1, label);
        if i == 0 && viewer_dirty {
            text.push_str(" ●");
        }
        let text = format!(" {text} ");
        let width = text.chars().count() as u16;
        areas[i] = Rect {
            x,
            y: area.y,
            width: width.min(area.right().saturating_sub(x)),
            height: area.height,
        };
        x = x.saturating_add(width).saturating_add(1);
        let style = if i == current {
            Style::default().fg(Color::White).bg(Color::Blue)
        } else {
            Style::default().fg(Color::White).bg(Color::Rgb(42, 42, 42))
        };
        spans.push(Span::styled(text, style));
        spans.push(Span::raw(" "));
    }
    app.tab_areas = areas;

    let paragraph = Paragraph::new(Line::from(spans)).style(Style::default().fg(Color::White));
    frame.render_widget(paragraph, area);
}
