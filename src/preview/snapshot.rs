//! シーンの描画結果をファイルに焼いて CI で差分を見るためのスナップショット。
//! 画像ではなくテキストで持つのは、PR の diff がそのまま「UI の差分」になるため
//! (バイナリのスクリーンショットだと「変わりました」以上のことが読めない)。
//!
//! 比較は Rust 側で持たず `git diff --exit-code` に任せる (CI の該当ステップ参照)。
//! ここは「毎回同じバイト列を書き出す」ことだけに責任を持つ。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

// 出力先はコンパイル時に確定するソースツリー。プレビューは dev 専用 feature なので、
// どこから実行してもリポジトリ内の同じ場所を更新するのが正しい
pub fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
}

pub fn write(name: &str, text: &str) -> io::Result<PathBuf> {
    let dir = dir();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.txt"));
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
        if path.extension().is_none_or(|ext| ext != "txt") {
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

/// 実行のたびに変わる値 (コミット SHA・絶対日時) を伏せる。ここを通さないと
/// 「UI は何も変わっていないのに毎回 diff が出る」スナップショットになって使い物にならない。
/// **桁数は必ず保つ** — スナップショットは比較用であると同時に目視用の画面でもあるので、
/// マスクで桁がずれると罫線が崩れて読めなくなる
pub fn normalize(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            // 1 行に両方が乗ることがある (左ペインのコミット一覧と右ペインの Date が
            // 同じバッファ行に並ぶ) ので、片方だけ返して終わりにしない
            let masked = mask_hashes(line);
            mask_date(&masked).unwrap_or(masked)
        })
        .collect()
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
