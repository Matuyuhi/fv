//! シーンの描画結果を画像に焼いて CI で差分を見るためのスナップショット。
//! 焼くのは SVG (preview/svg.rs) で、CI は描き直したものをコミット済みの
//! ファイルと突き合わせる。比較は Rust 側で持たず `git diff` に任せる
//! (CI の該当ステップ参照)。ここは「毎回同じバイト列を書き出す」ことだけに責任を持つ。
//!
//! **同じ画像が README にも載る**。UI の差分は GitHub が SVG を描画して見せてくれるので
//! (Files changed の画像比較)、テキストの画面をもう 1 系統持つ必要が無い。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ratatui::buffer::Buffer;

use super::render;

// 出力先はコンパイル時に確定するソースツリー。プレビューは dev 専用 feature なので、
// どこから実行してもリポジトリ内の同じ場所を更新するのが正しい。
// tests/ ではなく docs/ に置くのは、この画像が README から参照される成果物でもあるため
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("preview")
}

pub fn write(name: &str, text: &str) -> io::Result<PathBuf> {
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.svg"));
    fs::write(&path, text)?;
    Ok(path)
}

/// 全シーンを書き出した後に、もう存在しないシーンの残骸を消す。
/// シーンをリネームすると古いファイルが残り、CI が永久に無害な差分ゼロで通ってしまうため
pub fn prune(keep: &[&str]) -> io::Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    let Ok(entries) = fs::read_dir(dir()) else {
        return Ok(removed);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "svg") {
            continue;
        }
        let stale = path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|stem| !keep.contains(&stem));
        if stale {
            fs::remove_file(&path)?;
            removed.push(path);
        }
    }
    Ok(removed)
}

/// 実行のたびに変わる値 (コミット SHA・絶対日時) を、描き上がった Buffer の上で伏せる。
/// ここを通さないと「UI は何も変わっていないのに毎回差分が出る」スナップショットになって
/// 使い物にならない。**桁数は必ず保つ** — スナップショットは比較用であると同時に
/// README に載る画面でもあるので、マスクで桁がずれると罫線が崩れて読めなくなる。
///
/// 文字列に落としてから伏せるのではなく Buffer を直接書き換えるのは、SVG が
/// 「どのセルが何色か」まで焼くため。マスク後の文字だけ差し替えても、セルのスタイルが
/// 元の日付の長さに引きずられていては同じバイト列にならない (下の flatten 参照)
pub fn mask(buffer: &mut Buffer) {
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        let line = row_text(buffer, y);
        let masked = mask_hashes(&line);
        let masked = mask_date(&masked).unwrap_or(masked);
        if masked == line {
            continue;
        }
        write_row(buffer, y, &masked);
        // 日付は文字数そのものが日によって変わる (Aug 1 / Aug 10)。テキストは
        // 固定幅に詰め直しているので桁は保たれるが、**元の日付の末尾で切れていた
        // スタイルの境界**はその日の長さのまま残り、run の切れ目が 1 桁ずれた
        // SVG になってしまう。日付から行末までを 1 つのスタイルで塗り潰して
        // 境界そのものを消す (その先はペインの余白と枠だけで、失われる情報が無い)
        if let Some(col) = masked.find("<date>") {
            flatten_style(buffer, y, render::map_columns(&masked[..col]));
        }
    }
}

// 行を 1 本の文字列にする。全角セルの読み飛ばしは render::buffer_lines と同じ規則で、
// ここがずれると書き戻す位置がずれる
fn row_text(buffer: &Buffer, y: u16) -> String {
    let mut text = String::new();
    let mut skip = 0;
    for x in buffer.area.left()..buffer.area.right() {
        let Some(cell) = buffer.cell((x, y)) else {
            continue;
        };
        if skip > 0 {
            skip -= 1;
            continue;
        }
        skip = render::symbol_width(cell.symbol()).saturating_sub(1);
        text.push_str(cell.symbol());
    }
    text
}

// マスク済みの行をセルへ書き戻す。row_text が 1 セル 1 文字で組み立て、マスクは
// 文字数を変えない (桁数を保つのがマスクの前提) ので、セルと文字は 1 対 1 で対応する。
// マスクが触るのは ASCII だけなので、全角だったセルには同じ文字がそのまま返る
fn write_row(buffer: &mut Buffer, y: u16, text: &str) {
    let area = buffer.area;
    let mut chars = text.chars();
    let mut skip = 0;
    for x in area.left()..area.right() {
        let Some(cell) = buffer.cell_mut((x, y)) else {
            continue;
        };
        if skip > 0 {
            skip -= 1;
            continue;
        }
        skip = render::symbol_width(cell.symbol()).saturating_sub(1);
        let Some(symbol) = chars.next() else {
            break;
        };
        if !cell.symbol().starts_with(symbol) {
            cell.set_char(symbol);
        }
    }
}

// col 桁目から行末までを、col 桁目のセルのスタイルで塗り潰す。
// Cell::set_style は modifier を「足す/引く」ので上書きにならない — 3 つの値を直に写す
fn flatten_style(buffer: &mut Buffer, y: u16, col: usize) {
    let area = buffer.area;
    let x0 = area.left() + col as u16;
    let Some((fg, bg, modifier)) = buffer.cell((x0, y)).map(|c| (c.fg, c.bg, c.modifier)) else {
        return;
    };
    for x in x0..area.right() {
        if let Some(cell) = buffer.cell_mut((x, y)) {
            cell.fg = fg;
            cell.bg = bg;
            cell.modifier = modifier;
        }
    }
}

// `Date:   Sat Aug 1 20:32:28 2026 +0900` (git show のヘッダ)。
// 日付を 1 文字ずつ潰すだけでは足りない: 日にちの桁 (1 → 10) で日付の長さ自体が変わり、
// 後続の桁が丸ごとずれてしまう。タイムゾーン (+0900) の終わりまでを「固定文字列 + 空白詰め」で
// 置き換えることで、日付が何桁でも後ろの罫線が同じ桁に残る (末尾は元々空白なので、
// 詰めた空白と入れ替わっても結果は同じバイト列になる)
fn mask_date(line: &str) -> Option<String> {
    let idx = line.find("Date:")?;
    let (head, tail) = line.split_at(idx + "Date:".len());
    let end = timezone_end(tail)?;
    let (date, rest) = tail.split_at(end);
    let width = date.chars().count();
    let mut masked = String::from("   <date>");
    let len = masked.chars().count();
    if len > width {
        masked = masked.chars().take(width).collect();
    } else {
        masked.push_str(&" ".repeat(width - len));
    }
    Some(format!("{head}{masked}{rest}"))
}

// `+0900` / `-0500` の直後のバイト位置。日付部分の終端を機械的に決めるための目印
fn timezone_end(tail: &str) -> Option<usize> {
    let bytes = tail.as_bytes();
    bytes
        .windows(5)
        .position(|window| {
            matches!(window[0], b'+' | b'-') && window[1..].iter().all(u8::is_ascii_digit)
        })?
        .checked_add(5)
}

// 短縮 (7) / 完全 (40) のコミット SHA。長さが固定なので x で置くだけで桁は保たれる。
// 数字を 1 文字も含まない語 (英単語が偶然 a-f だけで綴られている場合) は誤爆を避けて除外する
fn mask_hashes(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if !is_hex(chars[i]) || (i > 0 && is_word(chars[i - 1])) {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && is_hex(chars[end]) {
            end += 1;
        }
        let len = end - i;
        let bounded = end >= chars.len() || !is_word(chars[end]);
        let has_digit = chars[i..end].iter().any(char::is_ascii_digit);
        if bounded && has_digit && (len == 7 || len == 40) {
            out.push_str(&"x".repeat(len));
        } else {
            out.extend(&chars[i..end]);
        }
        i = end;
    }
    out
}

fn is_hex(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, 'a'..='f')
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};

    fn buffer_with(text: &str, width: u16) -> Buffer {
        let mut buffer = Buffer::empty(Rect::new(0, 0, width, 1));
        buffer.set_string(0, 0, text, Style::default());
        buffer
    }

    fn line_of(buffer: &Buffer) -> String {
        row_text(buffer, 0)
    }

    /// 日にちが 1 桁か 2 桁かで日付の長さが変わっても、マスク後は同じ画面になること。
    /// ここが崩れると「UI は何も変えていないのに月初を境に差分が出始める」
    #[test]
    fn date_mask_is_stable_across_day_of_month_width() {
        let width = 60;
        let mut short = buffer_with("   Date:   Sat Aug 1 20:32:28 2026 +0900", width);
        let mut long = buffer_with("   Date:   Sun Aug 10 20:32:28 2026 +0900", width);
        mask(&mut short);
        mask(&mut long);
        assert_eq!(line_of(&short), line_of(&long));
        assert_eq!(line_of(&short).chars().count(), width as usize);
        assert!(line_of(&short).contains("<date>"));
    }

    /// 日付より後ろのセルはスタイルも 1 つに潰すこと。文字だけ揃えても、元の日付の
    /// 末尾で切れていたスタイルの境界が残ると SVG の run が 1 桁ずれる
    #[test]
    fn date_mask_flattens_the_style_after_it() {
        let width = 60;
        let mut buffer = buffer_with("   Date:   Sat Aug 1 20:32:28 2026 +0900", width);
        // 実際の描画と同じく「本文だけ色付き、その後ろの余白は別スタイル」を作る
        buffer.set_style(Rect::new(0, 0, 40, 1), Style::default().fg(Color::Gray));
        mask(&mut buffer);
        let colors: Vec<Color> = (11..width)
            .map(|x| buffer.cell((x, 0)).unwrap().fg)
            .collect();
        assert!(colors.windows(2).all(|w| w[0] == w[1]), "{colors:?}");
    }

    /// 短縮 SHA と完全 SHA を桁数を保ったまま伏せること。桁が変わると罫線が崩れ、
    /// 画面としても読めなくなる
    #[test]
    fn hashes_are_masked_without_changing_width() {
        let line = "> 7bd8ba2  commit 3674252a115234f083666b8957d81d9ef3c3cbfb";
        let mut buffer = buffer_with(line, 60);
        mask(&mut buffer);
        assert_eq!(
            line_of(&buffer).trim_end(),
            "> xxxxxxx  commit xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
        );
    }

    /// 左ペインのコミット一覧と右ペインの Date は同じバッファ行に並ぶ。
    /// 片方だけ処理して終わると SHA が生のまま残る (実際に踏んだ)
    #[test]
    fn masks_hash_and_date_on_the_same_line() {
        let mut buffer = buffer_with(
            "  7bd8ba2  3 days ago ││   Date:   Sat Aug 1 20:32:28 2026 +0900",
            80,
        );
        mask(&mut buffer);
        let line = line_of(&buffer);
        assert!(line.contains("xxxxxxx"), "{line}");
        assert!(line.contains("<date>"), "{line}");
    }

    /// 全角セルを跨いでも書き戻す位置がずれないこと (全角 1 セル = 2 桁)
    #[test]
    fn masking_keeps_wide_cells_in_place() {
        let mut buffer = buffer_with("あ 7bd8ba2 い", 40);
        mask(&mut buffer);
        assert_eq!(line_of(&buffer).trim_end(), "あ xxxxxxx い");
    }

    /// a-f だけで綴られた英単語 (数字を含まない) を SHA と誤認しないこと
    #[test]
    fn leaves_hex_looking_words_alone() {
        let mut buffer = buffer_with("acceded deface", 40);
        mask(&mut buffer);
        assert_eq!(line_of(&buffer).trim_end(), "acceded deface");
    }
}
