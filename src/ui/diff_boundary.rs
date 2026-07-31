//! LOG の複数ファイル diff (`git show`) と GIT レーンの「全ファイルまとめ diff」(#31) が
//! 共有するファイル境界の描画ヘルパー。#40 で LOG 用に作った sticky header の見た目を
//! そのまま流用する (境界の色・truncate ロジックを 2 箇所に複製しない)。

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

// sticky header・全幅バンド化した通常のファイル境界行の固定色。端末テーマに依存させないのは
// word-level ハイライトの ADDED_WORD_BG 等と同じ方針
const BOUNDARY_BG: Color = Color::Cyan;
const BOUNDARY_FG: Color = Color::Black;

// sticky 行 (常にペイン上端に固定するファイル名バー) を組み立てる。gutter は持たせず
// (diff 本文ではなくメタ情報のため) 全幅を同じ背景色で埋めて「本文ではない」ことを示す
pub(super) fn sticky_line(label: &str, width: usize) -> Line<'static> {
    let style = Style::default()
        .fg(BOUNDARY_FG)
        .bg(BOUNDARY_BG)
        .add_modifier(Modifier::BOLD);
    let text = truncate_label(label, width.max(1));
    let pad = width.saturating_sub(text.chars().count());
    Span::styled(format!("{text}{}", " ".repeat(pad)), style).into()
}

// 流れる側 (スクロールで消えていく通常のファイル境界行) も見た目を強化する。
// render_commit がヘッダ行に付けた固定背景色を目印に、右側をペイン幅まで同じ背景で
// 埋めて全幅の帯にする。gitview 側の行組み立てには触れず、描画側だけの加工に留める
pub(super) fn widen_boundary_bands(rows: &mut [Line<'static>], width: usize) {
    for row in rows.iter_mut() {
        let Some(style) = row
            .spans
            .iter()
            .find(|s| s.style.bg == Some(BOUNDARY_BG))
            .map(|s| s.style)
        else {
            continue;
        };
        let used: usize = row.spans.iter().map(|s| s.content.chars().count()).sum();
        if used < width {
            row.spans
                .push(Span::styled(" ".repeat(width - used), style));
        }
    }
}

// 長いパスは先頭を省略する。末尾のファイル名が最も情報量が多いため、区切り文字境界で
// 前方のディレクトリ階層から落としていき、それでも収まらなければファイル名自体を
// 末尾優先で char 単位に切る
fn truncate_label(label: &str, max_width: usize) -> String {
    if label.chars().count() <= max_width {
        return label.to_string();
    }
    let mut parts: Vec<&str> = label.split('/').collect();
    while parts.len() > 1 {
        parts.remove(0);
        let candidate = format!("…/{}", parts.join("/"));
        if candidate.chars().count() <= max_width {
            return candidate;
        }
    }
    let budget = max_width.saturating_sub(1);
    let mut tail: Vec<char> = label.chars().rev().take(budget).collect();
    tail.reverse();
    let tail: String = tail.into_iter().collect();
    format!("…{tail}")
}
