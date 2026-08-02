//! 描画済み Buffer を 1 枚の SVG へ落とす。README に載せる画面写真を CI で焼き直し、
//! 「実装が変わったのにスクリーンショットだけ古い」を無くすための出力。
//!
//! ラスタ画像 (PNG) にしないのは 3 つの理由による:
//! - 依存を足さずに書けるのは文字列だけ (ラスタ化にはフォントを読む依存が要る)
//! - CI のランナーには日本語フォントが無い。SVG なら描画は閲覧者のブラウザなので、
//!   合成リポジトリの日本語 (コミットメッセージ・コメント) が豆腐にならない
//! - テキストなので git の履歴に置ける (スナップショットの txt と同じ理由)
//!
//! 桁の整合は **1 文字ずつ x を指定する** ことで取る。閲覧環境の等幅フォントは
//! 送り幅がフォントごとに違う (0.55em〜0.6em) ので、run の先頭だけ置いて後は
//! フォント任せにすると行の右へ行くほどずれる。`textLength` で run ごとに幅を
//! 宣言する手もあるが、字間や字形が引き伸ばされて読みにくくなる。
//! セルの格子に 1 文字ずつ載せるのは端末そのものの振る舞いなので、これが一番素直。

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

use super::render::symbol_width;

// 1 セルの大きさ。CELL_W は主要な等幅フォントの送り幅 (0.6em) ちょうどにする —
// セルより狭いと罫線 (─ │ ┌) が繋がらず、枠が破線に見える。font-size 15 なら
// 0.6em = 9 と割り切れるので、座標が全て整数になってファイルも小さく収まる
// (1 文字ずつ x を書くので、座標の桁数がそのままファイルサイズに効く)
const FONT_SIZE: f32 = 15.0;
const CELL_W: f32 = 9.0;
const CELL_H: f32 = 20.0;
// 行の上端からベースラインまで。1 行の中で文字が上下に寄りすぎない位置
const BASELINE: f32 = 15.0;
const PAD: f32 = 14.0;
const RADIUS: f32 = 8.0;

// 閲覧環境に無いフォントを指定しても意味が無いので、OS ごとの既定の等幅を順に並べる
const FONT: &str =
    "ui-monospace, SFMono-Regular, Menlo, Consolas, &apos;DejaVu Sans Mono&apos;, monospace";

/// 端末の既定色。`Color::Reset` は「端末に任せる」という意味なので、画像に焼くには
/// ここで具体的な値を決めるしかない。既定テーマ (base16-ocean.dark) のペインが
/// 隣に並んでも浮かない明度にしてある
const DEFAULT_BG: Rgb = (0x1e, 0x22, 0x2a);
const DEFAULT_FG: Rgb = (0xc0, 0xc5, 0xce);

type Rgb = (u8, u8, u8);

/// ANSI 16 色。端末エミュレータごとに違う色を、ここで 1 つに決め打つ。
/// テーマ (syntect) 由来の色は Rgb で届くのでこの表を通らない = 実際の見た目と一致する
const PALETTE: [Rgb; 16] = [
    (0x2b, 0x30, 0x3b), // black
    (0xbf, 0x61, 0x6a), // red
    (0xa3, 0xbe, 0x8c), // green
    (0xeb, 0xcb, 0x8b), // yellow
    (0x8f, 0xa1, 0xb3), // blue
    (0xb4, 0x8e, 0xad), // magenta
    (0x96, 0xb5, 0xb4), // cyan
    (0xc0, 0xc5, 0xce), // gray (= 通常の白)
    (0x65, 0x73, 0x7e), // dark gray
    (0xd9, 0x7b, 0x84), // light red
    (0xb5, 0xcf, 0x9f), // light green
    (0xf2, 0xdb, 0xa7), // light yellow
    (0xa6, 0xb6, 0xc6), // light blue
    (0xc7, 0xa4, 0xc2), // light magenta
    (0xae, 0xca, 0xc9), // light cyan
    (0xef, 0xf1, 0xf5), // white
];

/// スタイルが同じまま続くセルの並び。SVG の要素数を桁数ではなく
/// 「スタイルの切れ目の数」に比例させる (1 セル 1 要素だと 3500 要素になる)
struct Run {
    /// 開始桁 (全角セルも 1 桁ではなく占める桁数で数える)
    col: usize,
    cols: usize,
    /// (開始桁, セルの文字)。文字列にまとめず桁を持ち回るのは、1 文字ずつ x を
    /// 書き出すのに全角セルを 2 桁ぶん進めた位置が要るため
    cells: Vec<(usize, String)>,
    fg: Rgb,
    /// None = 既定背景。背景の矩形自体を出さないことで、素の領域の要素数を 0 にする
    bg: Option<Rgb>,
    modifier: Modifier,
}

impl Run {
    fn is_blank(&self) -> bool {
        self.cells
            .iter()
            .all(|(_, symbol)| symbol.trim().is_empty())
    }
}

pub fn render(buffer: &Buffer) -> String {
    let area = buffer.area;
    let width = PAD * 2.0 + f32::from(area.width) * CELL_W;
    let height = PAD * 2.0 + f32::from(area.height) * CELL_H;

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" \
         viewBox=\"0 0 {} {}\" font-family=\"{FONT}\" font-size=\"{FONT_SIZE}\">\n",
        num(width),
        num(height),
        num(width),
        num(height)
    ));
    out.push_str(&format!(
        "<rect width=\"{}\" height=\"{}\" rx=\"{}\" fill=\"{}\"/>\n",
        num(width),
        num(height),
        num(RADIUS),
        hex(DEFAULT_BG)
    ));

    for y in area.top()..area.bottom() {
        let runs = row_runs(buffer, y);
        let top = PAD + f32::from(y - area.top()) * CELL_H;
        // 背景 → 文字の順に出す。SVG は後から書いた要素が上に来るので、
        // 行単位で層を分けないと隣の run の背景が文字を隠す
        push_backgrounds(&mut out, &runs, top);
        push_text(&mut out, &runs, top);
    }
    out.push_str("</svg>\n");
    out
}

// 隣り合う同じ背景色は 1 枚の矩形にまとめる。分けて出すと境目に髪の毛のような
// 隙間が見えることがある (小数座標の丸めで生じる)
fn push_backgrounds(out: &mut String, runs: &[Run], top: f32) {
    let mut pending: Option<(Rgb, usize, usize)> = None;
    for run in runs {
        match (&mut pending, run.bg) {
            (Some((color, _, end)), Some(bg)) if *color == bg && *end == run.col => {
                *end = run.col + run.cols;
            }
            _ => {
                if let Some((color, start, end)) = pending.take() {
                    push_rect(out, color, start, end - start, top, CELL_H);
                }
                pending = run.bg.map(|bg| (bg, run.col, run.col + run.cols));
            }
        }
    }
    if let Some((color, start, end)) = pending {
        push_rect(out, color, start, end - start, top, CELL_H);
    }
}

fn push_rect(out: &mut String, color: Rgb, col: usize, cols: usize, top: f32, height: f32) {
    out.push_str(&format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"{}\"/>\n",
        num(PAD + col as f32 * CELL_W),
        num(top),
        num(cols as f32 * CELL_W),
        num(height),
        hex(color)
    ));
}

fn push_text(out: &mut String, runs: &[Run], top: f32) {
    for run in runs {
        if run.modifier.contains(Modifier::UNDERLINED) {
            // text-decoration を解釈しない SVG レンダラがあるので矩形で引く
            push_rect(out, run.fg, run.col, run.cols, top + CELL_H - 2.0, 1.0);
        }
        // 空白しか無い run は背景 (と下線) が全てなので文字要素を出さない。
        // 画面の大半は余白なので、ここを省くだけでファイルが半分以下になる
        if run.modifier.contains(Modifier::HIDDEN) || run.is_blank() {
            continue;
        }
        let mut attrs = String::new();
        if run.modifier.contains(Modifier::BOLD) {
            attrs.push_str(" font-weight=\"bold\"");
        }
        if run.modifier.contains(Modifier::ITALIC) {
            attrs.push_str(" font-style=\"italic\"");
        }
        if run.modifier.contains(Modifier::DIM) {
            attrs.push_str(" fill-opacity=\"0.55\"");
        }
        let (xs, text) = glyphs(run);
        out.push_str(&format!(
            "<text x=\"{xs}\" y=\"{}\" fill=\"{}\"{attrs}>{text}</text>\n",
            num(top + BASELINE),
            hex(run.fg),
        ));
    }
}

/// x 属性に並べる 1 文字ぶんずつの座標と、エスケープ済みの本文。
/// x の並びは**本文の文字数と 1 対 1**でなければならない (SVG は先頭から順に
/// 対応付ける) ので、1 セルが複数 char の書記素クラスタ (結合文字) なら
/// 同じ座標を繰り返す — 合成された印は基底文字と同じ位置に重なるのが正しい。
///
/// 空白のセルは本文から落とす。空白は何も描かず、位置決めは後続の文字が自分の x を
/// 持っているので要らない。落とすことで XML の空白の扱い (連続する空白の詰め・
/// 前後の空白の削除) に一切依存しなくなる — `xml:space="preserve"` は SVG2 で
/// 非推奨になっており、実際に Chromium では効かず字送りが 1 文字ずつずれた
fn glyphs(run: &Run) -> (String, String) {
    let mut xs = String::new();
    let mut text = String::new();
    for (col, symbol) in &run.cells {
        if symbol.trim().is_empty() {
            continue;
        }
        let x = num(PAD + *col as f32 * CELL_W);
        for c in symbol.chars() {
            if !xs.is_empty() {
                xs.push(' ');
            }
            xs.push_str(&x);
            match c {
                '&' => text.push_str("&amp;"),
                '<' => text.push_str("&lt;"),
                '>' => text.push_str("&gt;"),
                _ => text.push(c),
            }
        }
    }
    (xs, text)
}

fn row_runs(buffer: &Buffer, y: u16) -> Vec<Run> {
    let area = buffer.area;
    let mut runs: Vec<Run> = Vec::new();
    let mut col = 0usize;
    // 全角文字が覆う 2 セル目の読み飛ばしは render::buffer_lines と同じ規則。
    // ずれると画像とスナップショットで別の絵になる
    let mut skip = 0usize;
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
        let (fg, bg) = resolve(cell.fg, cell.bg, cell.modifier);
        match runs.last_mut() {
            Some(run) if run.fg == fg && run.bg == bg && run.modifier == cell.modifier => {
                run.cells.push((col, cell.symbol().to_string()));
                run.cols += width;
            }
            _ => runs.push(Run {
                col,
                cols: width,
                cells: vec![(col, cell.symbol().to_string())],
                fg,
                bg,
                modifier: cell.modifier,
            }),
        }
        col += width;
    }
    runs
}

/// セルの前景・背景を具体的な RGB へ落とす。REVERSED (カーソル・選択行) は
/// 端末側が入れ替えて描くものなので、画像に焼く時点で入れ替えておく
fn resolve(fg: Color, bg: Color, modifier: Modifier) -> (Rgb, Option<Rgb>) {
    let front = rgb(fg, DEFAULT_FG);
    let back = (bg != Color::Reset).then(|| rgb(bg, DEFAULT_BG));
    if modifier.contains(Modifier::REVERSED) {
        (back.unwrap_or(DEFAULT_BG), Some(front))
    } else {
        (front, back)
    }
}

fn rgb(color: Color, default: Rgb) -> Rgb {
    match color {
        Color::Reset => default,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => indexed(i),
        Color::Black => PALETTE[0],
        Color::Red => PALETTE[1],
        Color::Green => PALETTE[2],
        Color::Yellow => PALETTE[3],
        Color::Blue => PALETTE[4],
        Color::Magenta => PALETTE[5],
        Color::Cyan => PALETTE[6],
        Color::Gray => PALETTE[7],
        Color::DarkGray => PALETTE[8],
        Color::LightRed => PALETTE[9],
        Color::LightGreen => PALETTE[10],
        Color::LightYellow => PALETTE[11],
        Color::LightBlue => PALETTE[12],
        Color::LightMagenta => PALETTE[13],
        Color::LightCyan => PALETTE[14],
        Color::White => PALETTE[15],
    }
}

// xterm-256 の定義通り: 16..=231 が 6x6x6 の色立方体、232.. がグレースケール
fn indexed(index: u8) -> Rgb {
    match index {
        0..=15 => PALETTE[index as usize],
        16..=231 => {
            let i = index - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        _ => {
            let v = 8 + (index - 232) * 10;
            (v, v, v)
        }
    }
}

fn hex((r, g, b): Rgb) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

// 座標は小数第 1 位まで。整数のときに ".0" を残さないのは、ファイルを小さく保つのと
// 「同じ入力なら同じバイト列」を読みやすくするため
fn num(value: f32) -> String {
    let text = format!("{value:.1}");
    text.trim_end_matches(".0").to_string()
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

    /// run の開始桁と占める桁数が、全角文字を挟んでもセルの桁と一致すること。
    /// ここがずれると画像の中で行ごとに文字がずれ、画面として読めなくなる
    #[test]
    fn runs_keep_the_column_grid() {
        let style = Style::default().fg(Color::Green);
        let buffer = buffer_with(8, &[(0, "あa", style)]);
        let runs = row_runs(&buffer, 0);
        assert_eq!((runs[0].col, runs[0].cols), (0, 3));
        // 残りは素のセル (色も装飾も無い空白) が 1 つの run にまとまる
        assert_eq!((runs[1].col, runs[1].cols), (3, 5));
    }

    /// REVERSED は画像に焼く時点で前景・背景を入れ替えること (端末側の解釈に頼れない)。
    /// カーソルは EDIT レーンでこの表現なので、崩れるとカーソルが消える
    #[test]
    fn reversed_swaps_colors() {
        let style = Style::default()
            .fg(Color::Red)
            .add_modifier(Modifier::REVERSED);
        let runs = row_runs(&buffer_with(2, &[(0, "x", style)]), 0);
        assert_eq!(runs[0].fg, DEFAULT_BG);
        assert_eq!(runs[0].bg, Some(PALETTE[1]));
    }

    /// 素の空白は背景の矩形も文字要素も生まないこと (画面の大半がこれなので、
    /// ここが崩れると要素数がセル数まで膨らむ)
    #[test]
    fn plain_blanks_emit_nothing() {
        let svg = render(&buffer_with(10, &[]));
        assert_eq!(svg.matches("<text").count(), 0);
        // 全体の背景 1 枚だけ
        assert_eq!(svg.matches("<rect").count(), 1);
    }

    #[test]
    fn escapes_xml_special_characters() {
        let buffer = buffer_with(6, &[(0, "a<b&", Style::default().fg(Color::Red))]);
        let svg = render(&buffer);
        assert!(svg.contains(">a&lt;b&amp;<"), "{svg}");
    }

    /// x の並びは本文の文字数と 1 対 1 であること。SVG は先頭から順に対応付けるので、
    /// 数が食い違うとその行の途中から文字が桁からずれる。
    /// 全角セルはそのぶん次の座標が 2 桁ぶん進むことも同時に見る
    #[test]
    fn one_x_per_character_on_the_cell_grid() {
        let style = Style::default().fg(Color::Green);
        let runs = row_runs(&buffer_with(8, &[(0, "あa", style)]), 0);
        let (xs, text) = glyphs(&runs[0]);
        let xs: Vec<&str> = xs.split(' ').collect();
        assert_eq!(xs.len(), text.chars().count());
        assert_eq!(xs, vec![num(PAD), num(PAD + 2.0 * CELL_W)]);
    }
}
