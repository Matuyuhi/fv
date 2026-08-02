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

/// セルのスタイルだけを 1 セル 1 文字に落とした「色の地図」。
/// スナップショットは ANSI を持てない (git diff が読めなくなる) 一方、色だけを変えた
/// 変更が差分に一切出ないのは検出漏れなので、文字とは別レイヤとして焼く
pub struct StyleMap {
    /// 画面と同じ行数・同じ桁数。上の画面と桁が揃うので、地図の列を見れば
    /// どのセルの色が変わったかを目で追える
    pub rows: Vec<String>,
    /// `a  fg=green` の形。凡例が無いと地図の文字が何色かを読めない
    pub legend: Vec<String>,
}

/// 素のセル (色も装飾も無い)。地図の大半を占めるので、色の付いた領域が目で浮くよう
/// 記号側に寄せておく
const PLAIN: char = '.';
// 凡例のキーに使う文字。1 セル 1 文字を崩さないため 1 文字で足りる範囲に収める。
// `.` (PLAIN) と `~` (日付マスク) は意味が衝突するので入れない
const KEYS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+*=/&%$#@!?<>";

pub fn style_map(buffer: &Buffer) -> StyleMap {
    let area = buffer.area;
    let mut styles: Vec<String> = Vec::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            let Some(desc) = describe(cell.fg, cell.bg, cell.modifier) else {
                continue;
            };
            if let Err(pos) = styles.binary_search(&desc) {
                styles.insert(pos, desc);
            }
        }
    }
    let keys = assign_keys(&styles);

    let key_of = |desc: &str| -> char {
        styles
            .binary_search(&desc.to_string())
            .ok()
            .map_or('?', |i| keys[i])
    };

    let mut rows = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut row = String::new();
        // 全角文字が覆う 2 セル目の扱いは buffer_lines と厳密に揃える。ここがずれると
        // 画面と地図の桁が合わなくなり、突き合わせて読むという用途自体が成り立たない
        let mut skip = 0;
        for x in area.left()..area.right() {
            let Some(cell) = buffer.cell((x, y)) else {
                continue;
            };
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let width = symbol_width(cell.symbol()).max(1);
            skip = width - 1;
            let key = describe(cell.fg, cell.bg, cell.modifier).map_or(PLAIN, |d| key_of(&d));
            for _ in 0..width {
                row.push(key);
            }
        }
        rows.push(row);
    }

    let legend = styles
        .iter()
        .zip(&keys)
        .map(|(desc, key)| format!("  {key}  {desc}"))
        .collect();
    StyleMap { rows, legend }
}

/// スタイル 1 つに 1 文字を振る。**キーはスタイルの内容から決める** (出現順や一覧の
/// index ではない) — index で振ると色を 1 つ足した/変えただけで以降のキーが全部ずれ、
/// 地図と凡例が丸ごと差分になる。それでは「どのセルの色が変わったか」を差分から
/// 読めず、色レイヤを焼く目的そのものが失われる。
/// 衝突した時だけ後ろへずらすので、影響はぶつかった 1〜2 個のスタイルに閉じる
fn assign_keys(styles: &[String]) -> Vec<char> {
    let mut taken = vec![false; KEYS.len()];
    let mut keys = Vec::with_capacity(styles.len());
    for desc in styles {
        let start = (fnv1a(desc) % KEYS.len() as u64) as usize;
        let slot = (0..KEYS.len())
            .map(|step| (start + step) % KEYS.len())
            .find(|i| !taken[*i]);
        match slot {
            Some(i) => {
                taken[i] = true;
                keys.push(KEYS[i] as char);
            }
            // キーを使い切るほどスタイルが多い画面は、地図を目で追うこと自体が
            // 成り立っていない。潰れたことが分かるようにだけしておく
            None => keys.push('?'),
        }
    }
    keys
}

// FNV-1a。std の DefaultHasher は実行ごと・バージョンごとに値が変わりうるので使えない
// (スナップショットは「同じ入力なら同じバイト列」であることに全面的に依存している)
fn fnv1a(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// スタイルを凡例 1 行ぶんの文字列にする。色も装飾も無い (= 端末の既定のまま) なら None で、
/// 地図では PLAIN になる
fn describe(fg: Color, bg: Color, modifier: Modifier) -> Option<String> {
    let mut parts = Vec::new();
    if fg != Color::Reset {
        parts.push(format!("fg={}", color_name(fg)));
    }
    if bg != Color::Reset {
        parts.push(format!("bg={}", color_name(bg)));
    }
    for (flag, name) in [
        (Modifier::BOLD, "bold"),
        (Modifier::DIM, "dim"),
        (Modifier::ITALIC, "italic"),
        (Modifier::UNDERLINED, "underlined"),
        (Modifier::SLOW_BLINK, "slow_blink"),
        (Modifier::RAPID_BLINK, "rapid_blink"),
        (Modifier::REVERSED, "reversed"),
        (Modifier::HIDDEN, "hidden"),
        (Modifier::CROSSED_OUT, "crossed_out"),
    ] {
        if modifier.contains(flag) {
            parts.push(name.to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

// RGB は 16 進で出す。テーマ由来の色は名前を持たないので、差分を見た時に
// 「どの色がどう変わったか」を数値で追えるようにする
fn color_name(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("idx({i})"),
        other => format!("{other:?}").to_lowercase(),
    }
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

/// 色の地図を card と同じ枠で出す。画面のすぐ下に同じ桁数で並べることで、
/// 「画面のこの位置のセル」と「地図のこの文字」を目で突き合わせられる
pub fn style_card(name: &str, width: u16, map: &StyleMap, color: bool) -> String {
    let rule: String = "─".repeat(width as usize);
    let (bold, dim, reset) = if color {
        ("\x1b[1m", "\x1b[2m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    let mut out = String::new();
    out.push_str(&format!(
        "{bold}{name}{reset} {dim}— セルのスタイル ({} 種){reset}\n",
        map.legend.len()
    ));
    out.push_str(&format!("┌{rule}┐\n"));
    for row in &map.rows {
        out.push_str(row);
        out.push('\n');
    }
    out.push_str(&format!("└{rule}┘\n"));
    for line in &map.legend {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// buffer_lines が出した文字列が、色の地図では何文字ぶんに相当するか。
/// 地図は 1 セル 1 文字 (全角は 2 文字) なので、文字側の位置から地図側の位置を
/// 求めるにはこの換算が要る
pub fn map_columns(text: &str) -> usize {
    text.chars()
        .map(|c| symbol_width(&c.to_string()).max(1))
        .sum()
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    fn buffer_with(width: u16, cells: &[(u16, &str, Style)]) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
        for (x, text, style) in cells {
            buffer.set_string(*x, 0, text, *style);
        }
        buffer
    }

    /// 地図の桁が画面の桁とぴったり一致すること。全角文字は 2 桁を占めるので、
    /// buffer_lines と同じ読み飛ばしをしないとここが 1 文字ずつずれていく
    /// (ずれると「画面のこのセルの色」を地図から引けなくなり、地図の意味が無くなる)
    #[test]
    fn map_columns_line_up_with_the_screen() {
        let style = Style::default().fg(Color::Green);
        let buffer = buffer_with(10, &[(0, "あa", style)]);
        let map = style_map(&buffer);
        let text = buffer_lines(&buffer, false);
        assert_eq!(map.rows[0].chars().count(), 10);
        assert_eq!(map_columns(&text[0]), 10);
        // 全角 2 桁 + 半角 1 桁が同じキーで埋まり、残りは素のセル
        let key = map.rows[0].chars().next().unwrap();
        assert_eq!(
            map.rows[0],
            format!("{}{}", key.to_string().repeat(3), ".".repeat(7))
        );
    }

    /// 色も装飾も無いセルは PLAIN のまま = 凡例に載らないこと。
    /// ここが崩れると地図が記号で埋まり、色の付いた領域が目で拾えなくなる
    #[test]
    fn plain_cells_stay_out_of_the_legend() {
        let buffer = buffer_with(4, &[(0, "ab", Style::default())]);
        let map = style_map(&buffer);
        assert_eq!(map.rows[0], "....");
        assert!(map.legend.is_empty());
    }

    /// スタイルが 1 つ増えても、既にあるスタイルのキーは動かないこと。
    /// index でキーを振ると全部ずれ、色を 1 つ変えただけで地図と凡例が丸ごと差分になる
    #[test]
    fn keys_do_not_shift_when_a_style_is_added() {
        let green = Style::default().fg(Color::Green);
        let red = Style::default().fg(Color::Red);
        let before = style_map(&buffer_with(6, &[(0, "ab", green)]));
        let after = style_map(&buffer_with(6, &[(0, "ab", green), (2, "cd", red)]));
        assert_eq!(before.rows[0][..2], after.rows[0][..2]);
        assert!(after.legend.contains(&before.legend[0]));
    }

    /// キーはスタイルの内容だけで決まる = 画面上の位置に依存しないこと
    #[test]
    fn keys_do_not_depend_on_position() {
        let style = Style::default().fg(Color::Cyan).bg(Color::Black);
        let left = style_map(&buffer_with(6, &[(0, "a", style)]));
        let right = style_map(&buffer_with(6, &[(4, "a", style)]));
        assert_eq!(left.legend, right.legend);
        assert_eq!(&left.rows[0][..1], &right.rows[0][4..5]);
    }

    /// 装飾も凡例に出ること (色は同じで bold だけ付けた、のような変更を取りこぼさない)
    #[test]
    fn modifiers_are_part_of_the_style() {
        let plain = Style::default().fg(Color::Red);
        let bold = plain.add_modifier(Modifier::BOLD);
        let map = style_map(&buffer_with(4, &[(0, "a", plain), (1, "b", bold)]));
        assert_eq!(map.legend.len(), 2);
        assert_ne!(&map.rows[0][..1], &map.rows[0][1..2]);
        assert!(map.legend.iter().any(|l| l.ends_with("fg=red bold")));
    }
}
