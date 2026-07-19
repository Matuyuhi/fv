use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::centered_rect;

// キーバインド一覧のオーバーレイ。実装済みのハンドラ (app/keys.rs の on_*_key) と
// 一対一で対応させる。ここに書いた内容と実際の挙動がずれないよう追加時は両方直す
pub(super) fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(70, 80, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("help");

    let mut lines: Vec<Line> = Vec::new();
    push_help_section(
        &mut lines,
        "Global",
        &[
            ("Ctrl+c", "終了"),
            ("q", "終了"),
            ("Tab", "フォーカス切替 (Tree/Viewer)"),
            ("Ctrl+p", "ファインダーを開く"),
            ("?", "このヘルプを開く"),
            ("s", "設定画面を開く"),
            ("a", "隠し項目の表示を切替"),
            ("-a, --hidden", "起動時に隠し項目を表示"),
        ],
    );
    push_help_section(
        &mut lines,
        "Tree",
        &[
            ("j/k ↑/↓", "上下移動"),
            ("l →", "展開 / 開く"),
            ("h ←", "折りたたみ / 親へ"),
            ("H", "親を選択して折りたたむ"),
            ("Enter", "開く / 展開切替"),
            ("gg / G", "先頭 / 末尾へ"),
            ("r", "再走査"),
        ],
    );
    push_help_section(
        &mut lines,
        "Viewer",
        &[
            ("j/k ↑/↓", "スクロール"),
            ("Ctrl+d/u", "半ページスクロール"),
            ("gg / G", "先頭 / 末尾へ"),
            ("w", "折り返し切替"),
            ("h/l ←/→", "水平スクロール"),
            ("0", "水平スクロールをリセット"),
            ("Ctrl+o", "履歴を戻る (Backspace も同様)"),
            ("Ctrl+i", "履歴を進む"),
            (":N Enter", "N 行目へジャンプ"),
            ("/", "検索"),
            ("n / N", "次 / 前のマッチへ"),
            ("e", "編集モードに入る"),
        ],
    );
    push_help_section(
        &mut lines,
        "Edit (e)",
        &[
            ("文字入力", "挿入 (クリックでカーソル移動)"),
            ("↑/↓/←/→", "カーソル移動"),
            ("Ctrl+←/→", "単語単位で移動"),
            ("Home/End", "行頭 / 行末へ"),
            ("Ctrl+s", "保存"),
            ("Ctrl+z / Ctrl+y", "undo / redo"),
            ("Ctrl+k", "行削除"),
            ("Esc", "終了 (未保存なら確認)"),
        ],
    );
    push_help_section(
        &mut lines,
        "Finder (Ctrl+p)",
        &[
            ("文字入力", "クエリを絞り込み"),
            ("↑/↓ Ctrl+n/p", "候補選択"),
            ("Backspace", "一文字削除"),
            ("Enter", "開く"),
            ("Esc", "閉じる"),
        ],
    );
    push_help_section(
        &mut lines,
        "Search・Goto (/ と :N)",
        &[
            ("文字入力", "入力 (Goto は数字のみ)"),
            ("Backspace", "一文字削除"),
            ("Enter", "確定"),
            ("Esc", "キャンセル"),
        ],
    );

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup);
}

// key 列を固定幅で左詰めし、"キー  説明" の2カラム風に整列させる
fn push_help_section(lines: &mut Vec<Line<'static>>, title: &str, entries: &[(&str, &str)]) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, desc) in entries {
        lines.push(Line::from(format!("  {key:<16}{desc}")));
    }
    lines.push(Line::from(""));
}
