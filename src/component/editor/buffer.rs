use std::fs;
use std::io;
use std::path::Path;

use crate::component::viewer::LineSource;

// 編集の最小単位。char 挿入・改行・行削除・ペーストを Insert/Delete の 2 種で表現すると、
// undo/redo は「逆 op の適用」(Insert の逆 = 同範囲の Delete) だけになる
enum EditOp {
    Insert {
        at: (usize, usize),
        text: String,
    },
    Delete {
        at: (usize, usize),
        text: String,
    },
    /// 削除と挿入が「1 つの操作」として取り消されるべきもの (行の入れ替え等) 用。
    /// delete + insert の 2 op で組むと undo を 2 回押す羽目になるため専用の変種にする
    Replace {
        at: (usize, usize),
        removed: String,
        inserted: String,
    },
}

pub struct EditBuffer {
    // 生テキスト (タブ・EOL を加工しない、改行なしの行)。viewer の plain は
    // タブ展開済みで保存に使えないため、disk から独立に読み直して保持する
    lines: Vec<String>,
    // 保存時に元ファイルの EOL・末尾改行を復元するための記憶
    crlf: bool,
    trailing_newline: bool,
    dirty: bool,
    undo: Vec<EditOp>,
    redo: Vec<EditOp>,
    // undo 末尾 op へタイピングを追記してよいか。カーソル移動・保存・ペースト・
    // 改行で false に戻し、undo の粒度を「入力のまとまり」にする
    coalesce: bool,
    // 前回の take_touched 以降の変更が最初に触れた行。ハイライトの再開点になる。
    // カーソル位置から推測しないのは、undo/redo が任意の位置に飛ぶため
    touched: Option<usize>,
}

impl EditBuffer {
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;
        let crlf = text.contains("\r\n");
        let trailing_newline = text.ends_with('\n');
        let mut lines: Vec<String> = text
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_string())
            .collect();
        // split('\n') は末尾改行の後ろに空要素を作る。行として存在しないので落とす
        if trailing_newline {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push(String::new());
        }
        Ok(Self {
            lines,
            crlf,
            trailing_newline,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
            coalesce: false,
            touched: None,
        })
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, idx: usize) -> &str {
        &self.lines[idx]
    }

    pub fn line_len(&self, idx: usize) -> usize {
        self.lines[idx].chars().count()
    }

    /// ライブ diff (component/editor/diff.rs) が baseline と行単位で比較するための全行ビュー
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// ハイライト用の行ソース。最終行にも改行がある扱いで固定するのは、末尾が空行の
    /// バッファでもその行が 1 行として描画から欠けないようにするため
    pub fn source(&self) -> LineSource<'_> {
        LineSource {
            lines: &self.lines,
            trailing_newline: true,
        }
    }

    /// 前回以降の変更が最初に触れた行を取り出す (以降のハイライトを作り直す起点)
    pub fn take_touched(&mut self) -> Option<usize> {
        self.touched.take()
    }

    /// タイピングのまとまりをここで区切る。カーソル移動・クリック等の編集以外の操作から呼ぶ
    pub fn seal(&mut self) {
        self.coalesce = false;
    }

    /// 保存用テキスト。EOL・末尾改行を読込時の形で復元する
    pub fn to_text(&self) -> String {
        let eol = if self.crlf { "\r\n" } else { "\n" };
        let mut text = self.lines.join(eol);
        if self.trailing_newline {
            text.push_str(eol);
        }
        text
    }

    pub fn mark_saved(&mut self) {
        self.dirty = false;
        self.coalesce = false;
    }

    /// 1 文字のタイピング挿入。直前も連続タイピングなら undo 1 単位にまとめる
    pub fn insert_typed(&mut self, at: (usize, usize), c: char) -> (usize, usize) {
        let text = c.to_string();
        let end = self.apply_insert(at, &text);
        self.dirty = true;
        self.redo.clear();
        let mut merged = false;
        if self.coalesce
            && let Some(EditOp::Insert {
                at: last_at,
                text: last_text,
            }) = self.undo.last_mut()
            && !last_text.contains('\n')
            && end_of(*last_at, last_text) == at
        {
            last_text.push(c);
            merged = true;
        }
        if !merged {
            self.undo.push(EditOp::Insert { at, text });
        }
        self.coalesce = true;
        end
    }

    /// 改行・ペーストなどの一括挿入。undo は常に独立した 1 単位になる
    pub fn insert_block(&mut self, at: (usize, usize), text: &str) -> (usize, usize) {
        let end = self.apply_insert(at, text);
        self.dirty = true;
        self.redo.clear();
        self.undo.push(EditOp::Insert {
            at,
            text: text.to_string(),
        });
        self.coalesce = false;
        end
    }

    /// 範囲を別のテキストへ差し替える。undo は常に独立した 1 単位になる。
    /// 戻り値は挿入テキスト末尾の位置
    pub fn replace(&mut self, from: (usize, usize), to: (usize, usize), text: &str) {
        let removed = self.apply_delete(from, to);
        self.apply_insert(from, text);
        self.dirty = true;
        self.redo.clear();
        self.undo.push(EditOp::Replace {
            at: from,
            removed,
            inserted: text.to_string(),
        });
        self.coalesce = false;
    }

    /// 範囲削除。1 文字削除 (Backspace/Delete 連打) は方向を判定して undo 1 単位にまとめる
    pub fn delete(&mut self, from: (usize, usize), to: (usize, usize)) {
        let removed = self.apply_delete(from, to);
        self.dirty = true;
        self.redo.clear();
        let single = !removed.contains('\n');
        let mut merged = false;
        if self.coalesce
            && single
            && let Some(EditOp::Delete { at, text }) = self.undo.last_mut()
            && !text.contains('\n')
        {
            if *at == to {
                // Backspace 連打: 削除範囲を前方に伸ばす
                *at = from;
                text.insert_str(0, &removed);
                merged = true;
            } else if *at == from {
                // Delete 連打: 削除範囲を後方に伸ばす
                text.push_str(&removed);
                merged = true;
            }
        }
        if !merged {
            self.undo.push(EditOp::Delete {
                at: from,
                text: removed,
            });
        }
        self.coalesce = single;
    }

    /// 戻り値は undo 後のカーソル位置。何も戻せなければ None
    pub fn undo(&mut self) -> Option<(usize, usize)> {
        let op = self.undo.pop()?;
        self.coalesce = false;
        self.dirty = true;
        let cursor = match &op {
            EditOp::Insert { at, text } => {
                self.apply_delete(*at, end_of(*at, text));
                *at
            }
            EditOp::Delete { at, text } => self.apply_insert(*at, text),
            EditOp::Replace {
                at,
                removed,
                inserted,
            } => {
                self.apply_delete(*at, end_of(*at, inserted));
                self.apply_insert(*at, removed);
                *at
            }
        };
        self.redo.push(op);
        Some(cursor)
    }

    pub fn redo(&mut self) -> Option<(usize, usize)> {
        let op = self.redo.pop()?;
        self.coalesce = false;
        self.dirty = true;
        let cursor = match &op {
            EditOp::Insert { at, text } => self.apply_insert(*at, text),
            EditOp::Delete { at, text } => {
                self.apply_delete(*at, end_of(*at, text));
                *at
            }
            EditOp::Replace {
                at,
                removed,
                inserted,
            } => {
                self.apply_delete(*at, end_of(*at, removed));
                self.apply_insert(*at, inserted);
                *at
            }
        };
        self.undo.push(op);
        Some(cursor)
    }

    // undo 記録なしの適用プリミティブ。戻り値は挿入テキスト末尾の位置
    fn apply_insert(&mut self, at: (usize, usize), text: &str) -> (usize, usize) {
        let (line, col) = at;
        self.touch(line);
        let byte = byte_of(&self.lines[line], col);
        if !text.contains('\n') {
            self.lines[line].insert_str(byte, text);
            return (line, col + text.chars().count());
        }
        let tail = self.lines[line].split_off(byte);
        let mut segments = text.split('\n');
        // split は少なくとも 1 要素を返す
        self.lines[line].push_str(segments.next().unwrap());
        let mut idx = line;
        let mut last_len = 0;
        for segment in segments {
            idx += 1;
            last_len = segment.chars().count();
            self.lines.insert(idx, segment.to_string());
        }
        self.lines[idx].push_str(&tail);
        (idx, last_len)
    }

    fn apply_delete(&mut self, from: (usize, usize), to: (usize, usize)) -> String {
        let (l1, c1) = from;
        let (l2, c2) = to;
        self.touch(l1);
        if l1 == l2 {
            let b1 = byte_of(&self.lines[l1], c1);
            let b2 = byte_of(&self.lines[l1], c2);
            return self.lines[l1].drain(b1..b2).collect();
        }
        let b1 = byte_of(&self.lines[l1], c1);
        let mut removed = self.lines[l1].split_off(b1);
        let b2 = byte_of(&self.lines[l2], c2);
        // to より後ろは削除対象外なので、行ごと drain する前に切り出して先頭行へ繋ぎ直す
        let tail = self.lines[l2].split_off(b2);
        for line in self.lines.drain(l1 + 1..=l2) {
            removed.push('\n');
            removed.push_str(&line);
        }
        self.lines[l1].push_str(&tail);
        removed
    }

    fn touch(&mut self, line: usize) {
        self.touched = Some(self.touched.map_or(line, |prev| prev.min(line)));
    }
}

// text を at に挿入した (または at から text を削除する) 場合の終端位置
fn end_of(at: (usize, usize), text: &str) -> (usize, usize) {
    let newlines = text.matches('\n').count();
    // rsplit は少なくとも 1 要素を返す
    let last = text.rsplit('\n').next().unwrap();
    if newlines == 0 {
        (at.0, at.1 + last.chars().count())
    } else {
        (at.0 + newlines, last.chars().count())
    }
}

// char インデックス → byte オフセット。範囲外は末尾に丸める
fn byte_of(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(byte, _)| byte)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(text: &str) -> EditBuffer {
        let path = std::env::temp_dir().join(format!(
            "fv-edit-buffer-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::write(&path, text).unwrap();
        let buffer = EditBuffer::load(&path).unwrap();
        let _ = fs::remove_file(&path);
        buffer
    }

    #[test]
    fn replace_swaps_two_lines_and_undoes_in_one_step() {
        let mut b = buffer("one\ntwo\nthree\n");
        let len = b.line_len(1);
        b.replace((0, 0), (1, len), "two\none");
        assert_eq!(b.lines(), ["two", "one", "three"]);

        // 行の入れ替えは 1 操作。undo 1 回で元に戻る (delete + insert の 2 op にしない理由)
        assert_eq!(b.undo(), Some((0, 0)));
        assert_eq!(b.lines(), ["one", "two", "three"]);
        assert_eq!(b.redo(), Some((0, 0)));
        assert_eq!(b.lines(), ["two", "one", "three"]);
    }

    #[test]
    fn replace_keeps_the_saved_text_shape() {
        let mut b = buffer("a\nb\n");
        b.replace((0, 0), (1, 1), "b\na");
        assert_eq!(b.to_text(), "b\na\n");
    }
}
