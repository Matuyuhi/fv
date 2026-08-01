//! 描画済み Buffer を端末へそのまま流せる文字列へ落とす。
//! ratatui の差分描画 (前フレームと違うセルだけ出力) はここでは使えない —
//! プレビューは「1 フレームを丸ごと紙に焼く」用途なので、全セルを毎回 SGR 付きで出す。

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// Buffer を 1 行 1 String に落とす。color=false なら SGR を一切付けない
/// (CI での差分比較・grep 用)
pub fn buffer_lines(buffer: &Buffer, color: bool) -> Vec<String> {
    let area = buffer.area;
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut line = String::new();
        // 直前セルのスタイル。同じ間は SGR を出さない (出力が数倍に膨らむのを防ぐ)
        let mut prev: Option<(Color, Color, Modifier)> = None;
        // 全角文字が覆う 2 セル目を読み飛ばす残り数。ratatui は覆われるセルを空白へ
        // reset するだけなので、そのまま出すと全角文字ごとに空白が 1 つ挟まる
        // (Buffer::diff が同じ理由で to_skip を持っているのと同じ処理)
        let mut skip = 0;
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            if skip > 0 {
                skip -= 1;
                continue;
            }
            skip = symbol_width(cell.symbol()).saturating_sub(1);
            if color {
                let style = (cell.fg, cell.bg, cell.modifier);
                if prev != Some(style) {
                    line.push_str(&sgr(style.0, style.1, style.2));
                    prev = Some(style);
                }
            }
            line.push_str(cell.symbol());
        }
        if color && prev.is_some() {
            line.push_str("\x1b[0m");
        }
        lines.push(line);
    }
    lines
}

/// 1 シーンぶんの出力。見出し + 上下の罫線で囲むだけで左右には枠を付けない —
/// 枠を付けると桁数が幅とずれ、「この UI は 100 桁でどう見えるか」を目で測れなくなる
pub fn card(
    name: &str,
    description: &str,
    width: u16,
    height: u16,
    body: &[String],
    color: bool,
) -> String {
    let rule: String = "─".repeat(width as usize);
    let (bold, dim, reset) = if color {
        ("\x1b[1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    let mut out = String::new();
    out.push_str(&format!(
        "{bold}{name}{reset} {dim}— {description} ({width}x{height}){reset}\n"
    ));
    out.push_str(&format!("┌{rule}┐\n"));
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(&format!("└{rule}┘\n"));
    out
}

/// セルが端末上で占める桁数。**プレビューの出力を端末の桁送りに合わせるためだけ**の
/// 近似で、アプリ本体の桁計算 (text.rs が唯一の定義) には決して使わないこと。
/// unicode-width を依存に足さない代わりに、East Asian Wide/Fullwidth の主要範囲だけを見る
fn symbol_width(symbol: &str) -> usize {
    let Some(c) = symbol.chars().next() else {
        return 0;
    };
    let code = c as u32;
    let wide = matches!(code,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xA960..=0xA97F
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

fn sgr(fg: Color, bg: Color, modifier: Modifier) -> String {
    let mut codes = vec!["0".to_string()];
    for (flag, code) in [
        (Modifier::BOLD, "1"),
        (Modifier::DIM, "2"),
        (Modifier::ITALIC, "3"),
        (Modifier::UNDERLINED, "4"),
        (Modifier::SLOW_BLINK, "5"),
        (Modifier::RAPID_BLINK, "6"),
        (Modifier::REVERSED, "7"),
        (Modifier::HIDDEN, "8"),
        (Modifier::CROSSED_OUT, "9"),
    ] {
        if modifier.contains(flag) {
            codes.push(code.to_string());
        }
    }
    if let Some(code) = color_code(fg, false) {
        codes.push(code);
    }
    if let Some(code) = color_code(bg, true) {
        codes.push(code);
    }
    format!("\x1b[{}m", codes.join(";"))
}

// crossterm backend が出すのと同じ対応表 (Gray=37 / DarkGray=90 / White=97)。
// ここがずれるとプレビューと実際の端末表示で色が食い違う
fn color_code(color: Color, background: bool) -> Option<String> {
    let offset = if background { 10 } else { 0 };
    let base = match color {
        Color::Reset => return None,
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::Gray => 37,
        Color::DarkGray => 90,
        Color::LightRed => 91,
        Color::LightGreen => 92,
        Color::LightYellow => 93,
        Color::LightBlue => 94,
        Color::LightMagenta => 95,
        Color::LightCyan => 96,
        Color::White => 97,
        Color::Rgb(r, g, b) => return Some(format!("{};2;{r};{g};{b}", 38 + offset)),
        Color::Indexed(i) => return Some(format!("{};5;{i}", 38 + offset)),
    };
    Some((base + offset).to_string())
}
