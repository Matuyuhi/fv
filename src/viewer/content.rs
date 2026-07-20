use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use ratatui::text::Line;

use super::{Viewer, highlight};
use crate::text::{TAB_EXPANDED, gutter_width};

pub enum Content {
    // plain は normalize 済み (タブ展開後) の行文字列。lines の span と桁位置が一致するので、
    // 検索マッチの char 列インデックスをそのままハイライト適用に使い回せる
    Text {
        lines: Vec<Line<'static>>,
        plain: Vec<String>,
    },
    Binary,
    Error(String),
}

pub struct Open {
    pub title: String,
    pub path: PathBuf,
    pub content: Rc<Content>,
    // 変更行番号 (1-origin)。git 情報が取れない場合は None のままガター表示を素通しする
    pub changed_lines: Option<HashSet<usize>>,
}

impl Viewer {
    pub(super) fn load(&self, path: &Path) -> Content {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => return Content::Error(format!("failed to read: {e}")),
        };
        let sniff = &bytes[..bytes.len().min(super::BINARY_SNIFF_BYTES)];
        if sniff.contains(&0) {
            return Content::Binary;
        }
        let text = String::from_utf8_lossy(&bytes);
        let width = gutter_width(text.lines().count());
        let lines = if bytes.len() > super::MAX_HIGHLIGHT_BYTES {
            highlight::plain_lines(&text, width)
        } else {
            self.highlighter.highlight_lines(path, &text, width)
        };
        let plain = plain_text_lines(&text);
        Content::Text { lines, plain }
    }
}

// highlight_lines/plain_lines と同じ行分割・タブ展開を行い、桁位置を一致させる
fn plain_text_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.replace('\t', TAB_EXPANDED))
        .collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
