use std::fs;
use std::io;
use std::path::Path;

// 編集の最小単位。char 挿入・改行・行削除・ペーストを全部この 2 種で表現すると、
// undo/redo は「逆 op の適用」(Insert の逆 = 同範囲の Delete) だけになる
enum EditOp {
    Insert { at: (usize, usize), text: String },
    Delete { at: (usize, usize), text: String },
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

    /// ライブ diff (editor/diff.rs) が baseline と行単位で比較するための全行ビュー
    pub fn lines(&self) -> &[String] {
        &self.lines
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

    /// ハイライト用テキスト。常に \n 区切り + 末尾 \n にすることで、
    /// LinesWithEndings / str::lines の行数が lines.len() と必ず一致する
    /// (末尾が空行のバッファでもその行が描画から欠けない)
    pub fn display_text(&self) -> String {
        let mut text = self.lines.join("\n");
        text.push('\n');
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
        };
        self.undo.push(op);
        Some(cursor)
    }

    // undo 記録なしの適用プリミティブ。戻り値は挿入テキスト末尾の位置
    fn apply_insert(&mut self, at: (usize, usize), text: &str) -> (usize, usize) {
        let (line, col) = at;
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
