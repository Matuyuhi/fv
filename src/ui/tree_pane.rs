use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{List, ListItem, ListState};

use crate::app::{App, Focus};
use crate::git::{FileStatus, StatusKind};

use super::pane_block;

pub(super) fn draw_tree(frame: &mut Frame, app: &mut App, area: Rect) {
    // GIT レーンでは絞り込み中であることをタイトルで示す (行が減った理由が分かるように)
    let title = if app.tree.is_filtered() {
        format!("changes ({})", app.tree.visible_files())
    } else {
        app.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| app.root.display().to_string())
    };
    let block = pane_block(title, app.focus == Focus::Tree);
    let inner = block.inner(area);

    let total = app.tree.visible.len();
    let selected = (total > 0).then_some(app.tree.selected);
    app.tree.list_state.select(selected);

    // 行の高さは gutter 込みでも常に 1 (name に改行は入らない) なので、ratatui の
    // List が内部でやる「選択行を含む最小限のウィンドウ計算」は同じ結果になる
    // O(1) の式に置き換えられる。ここで [first, last) を確定させ、ListItem は
    // その範囲だけ組み立てる (visible 全体ではなく画面に映る行数に比例させるのが目的)
    let max_height = inner.height as usize;
    let (first, last) = if total == 0 || max_height == 0 {
        (0, 0)
    } else {
        visible_window(
            total,
            max_height,
            *app.tree.list_state.offset_mut(),
            selected,
        )
    };
    // list_state.offset() は app/mouse.rs::click_tree_row がクリック行の絶対 index 換算に使う
    // (ui→app の書き戻しパターン、tree_area 等と同じ)。ratatui 標準の List に描画を任せると
    // ウィンドウ切り出し分だけ相対化されてしまうため、絶対値をこちらで書き戻す
    *app.tree.list_state.offset_mut() = first;

    let git = app.git.as_ref();
    let items: Vec<ListItem> = app.tree.visible[first..last]
        .iter()
        .map(|row| {
            // アイコン有効時は folder の開閉アイコンが展開状態を兼ねるためマーカー不要
            let marker = if app.icons {
                ""
            } else if row.is_dir {
                if row.expanded { "▾ " } else { "▸ " }
            } else {
                "  "
            };
            // ディレクトリは git.files に直接エントリを持たないため自然に None になる
            let file_status = git.and_then(|g| g.files.get(&row.path).copied());
            let prefix = file_status.map(status_prefix).unwrap_or_default();
            let icon = if app.icons {
                let glyph = if row.is_dir {
                    super::icons::dir_icon(row.expanded)
                } else {
                    super::icons::file_icon(&row.name)
                };
                format!("{glyph} ")
            } else {
                String::new()
            };
            let label = format!(
                "{}{}{}{}{}",
                "  ".repeat(row.depth),
                marker,
                prefix,
                icon,
                row.name
            );
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
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    // items は [first, last) だけの部分列なので、List に渡す選択位置もその中の相対位置に
    // 直す必要がある。絶対値は app.tree.list_state (offset() 経由でクリック判定が読む) 側に
    // 既に書き戻し済みなので、ここは使い捨ての一時 state で構わない
    let mut render_state = ListState::default();
    if let Some(sel) = selected {
        render_state.select(Some(sel - first));
    }
    frame.render_stateful_widget(list, area, &mut render_state);
}

/// ratatui `List` が内部で行う「選択行を含む最小限のウィンドウ」計算 (get_items_bounds) と
/// 等価な結果を返す。ツリーの行は全て高さ1 (name に改行は入らない) なので、あちらのような
/// 可変高さ対応のループは要らず、offset を起点に selected が入るまでスライドするだけの
/// O(1) 計算に落とせる。selected が既にウィンドウ内なら offset をそのまま保つのがポイントで、
/// ここを毎回 selected 中心に作り直すと「選択が動くたびに画面が揺れる」挙動になってしまう
fn visible_window(
    total: usize,
    max_height: usize,
    offset: usize,
    selected: Option<usize>,
) -> (usize, usize) {
    let offset = offset.min(total - 1);
    let index_to_display = selected.map(|s| s.min(total - 1)).unwrap_or(offset);
    let mut first = offset;
    let mut last = (offset + max_height).min(total);
    if index_to_display >= last {
        first = index_to_display + 1 - max_height.min(index_to_display + 1);
        last = (first + max_height).min(total);
    } else if index_to_display < first {
        first = index_to_display;
        last = (first + max_height).min(total);
    }
    (first, last)
}

// ツリーの行頭に置く XY (index 側 + worktree 側) + 空白のマーカー。
// git status --short と同じ並びで "M " = ステージ済みのみ、" M" = 未ステージのみ、
// "MM" = 両方、"??" = untracked を表す
fn status_prefix(status: FileStatus) -> String {
    format!(
        "{}{} ",
        status_char(status.index),
        status_char(status.worktree)
    )
}

fn status_char(kind: Option<StatusKind>) -> char {
    match kind {
        None => ' ',
        Some(StatusKind::Modified) => 'M',
        Some(StatusKind::Added) => 'A',
        Some(StatusKind::Untracked) => '?',
        Some(StatusKind::Deleted) => 'D',
        Some(StatusKind::Renamed) => 'R',
    }
}

// 色は worktree 側を優先する (未ステージの変更の方がこれから触る対象として目立たせたいため)。
// 未ステージが無ければ index 側で判定する。両方 None は file_status が Some を返す限り
// 実際には起こらない (porcelain の行は必ずどちらかに変更を持つ)
fn status_color(status: FileStatus) -> Color {
    match status.worktree.or(status.index) {
        Some(StatusKind::Modified) => Color::Yellow,
        Some(StatusKind::Added) | Some(StatusKind::Untracked) | Some(StatusKind::Renamed) => {
            Color::Green
        }
        Some(StatusKind::Deleted) => Color::Red,
        None => Color::Yellow,
    }
}
