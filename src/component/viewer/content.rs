use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::render::LineSource;
use crate::text::TAB_EXPANDED;

pub enum Content {
    Text(TextDoc),
    Binary,
    Error(String),
}

/// 表示対象のテキスト。ハイライト済みの Line は持たない — 画面に映る範囲だけを
/// HighlightCache (render.rs) が都度組み立てるので、ここは行のテキストだけを持つ。
/// この型が cache (path → Rc<Content>) の中身なので、テーマを変えても捨てる必要がない
pub struct TextDoc {
    // 生の行 (タブ・EOL 未加工)。syntect へ渡す唯一の入力
    raw: Vec<String>,
    /// normalize 済み (タブ展開後) の行。char インデックスが描画桁と 1:1 対応するので、
    /// 検索マッチの (line, start_col, end_col) はこちらの座標で表せる
    pub plain: Vec<String>,
    trailing_newline: bool,
    /// 巨大ファイルは syntect を通さずプレーン表示にする
    pub plain_only: bool,
}

impl TextDoc {
    pub fn source(&self) -> LineSource<'_> {
        LineSource {
            lines: &self.raw,
            trailing_newline: self.trailing_newline,
        }
    }

    pub fn line_count(&self) -> usize {
        self.plain.len()
    }

    /// タブ未展開の元の行。範囲選択のコピー (selection.rs) はこちらから取り出す —
    /// plain のままだとタブが空白 4 個に化けて貼り付け先のインデントが壊れる
    pub fn raw(&self) -> &[String] {
        &self.raw
    }

    pub fn has_trailing_newline(&self) -> bool {
        self.trailing_newline
    }
}

pub struct Open {
    pub title: String,
    pub path: PathBuf,
    pub content: Rc<Content>,
    // 変更行番号 (1-origin)。git 情報が取れない場合は None のままガター表示を素通しする
    pub changed_lines: Option<HashSet<usize>>,
}

pub(super) fn load(path: &Path) -> Content {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => return Content::Error(format!("failed to read: {e}")),
    };
    let sniff = &bytes[..bytes.len().min(super::BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return Content::Binary;
    }
    let text = String::from_utf8_lossy(&bytes);
    // str::lines は \n で割って行末の \r も落とす。行数は「末尾改行の後ろに空行を作らない」
    // という描画側の前提と一致する
    let mut raw: Vec<String> = text.lines().map(str::to_string).collect();
    if raw.is_empty() {
        raw.push(String::new());
    }
    let plain = raw
        .iter()
        .map(|line| line.replace('\t', TAB_EXPANDED))
        .collect();
    Content::Text(TextDoc {
        trailing_newline: text.ends_with('\n'),
        plain_only: bytes.len() > super::MAX_HIGHLIGHT_BYTES,
        raw,
        plain,
    })
}
