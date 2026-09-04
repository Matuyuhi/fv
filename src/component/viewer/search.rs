use super::Viewer;
use super::content::Content;

/// 1件のマッチ位置。列は plain の char 単位インデックス (gutter は含まない)
pub struct Match {
    pub line: usize,
    pub start_col: usize,
    pub end_col: usize,
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<Match>,
    // Enter で確定した後にだけ Some。n/N で動かす現在位置
    pub current: Option<usize>,
}

impl Viewer {
    /// Search 入力中のライブプレビュー。マッチを再計算するだけでジャンプはしない
    pub fn update_search(&mut self, query: &str) {
        if query.is_empty() {
            self.search = None;
            return;
        }
        let matches = self.compute_matches(query);
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current: None,
        });
    }

    /// Enter で確定。現在のスクロール位置以降の最初のマッチへジャンプ (なければ先頭へ wrap)
    pub fn confirm_search(&mut self) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let scroll = self.viewport.scroll;
        let idx = search
            .matches
            .iter()
            .position(|m| m.line >= scroll)
            .unwrap_or(0);
        let line = search.matches[idx].line;
        if let Some(search) = &mut self.search {
            search.current = Some(idx);
        }
        self.center_on(line);
    }

    pub fn cancel_search(&mut self) {
        self.search = None;
    }

    /// 横断検索 (Ctrl+f) のヒットへ着地する。同じクエリでファイル内検索を立て、その行の
    /// マッチを現在位置にしてから中央へ寄せる — `/` で同じ語を探した後と同じ状態にする
    /// (n/N が続けて効き、ハイライトも同じ色で出る)。col は plain の char 桁で、同じ行の
    /// 2 つ目以降のヒットを選んだ時に n がその次へ進めるよう、行だけでなく桁まで突き合わせる
    /// (開き直しで行がずれていて見つからなければ、その行以降の最初のマッチへ落とす)
    pub fn locate_search(&mut self, query: &str, line: usize, col: usize) {
        let matches = self.compute_matches(query);
        let current = matches
            .iter()
            .position(|m| m.line == line && m.start_col == col)
            .or_else(|| matches.iter().position(|m| m.line >= line))
            .or_else(|| (!matches.is_empty()).then_some(0));
        self.search = Some(SearchState {
            query: query.to_string(),
            matches,
            current,
        });
        self.center_on(line);
    }

    pub fn next_match(&mut self) {
        self.step_match(1);
    }

    pub fn prev_match(&mut self) {
        self.step_match(-1);
    }

    fn step_match(&mut self, delta: isize) {
        let Some(search) = &self.search else {
            return;
        };
        if search.matches.is_empty() {
            return;
        }
        let Some(current) = search.current else {
            return;
        };
        let len = search.matches.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        let line = search.matches[next].line;
        if let Some(search) = &mut self.search {
            search.current = Some(next);
        }
        self.center_on(line);
    }

    // マッチ行が viewport の中央付近に来るようスクロールする。goto_line (mod.rs) からも呼ばれる。
    // 飛んだ先が行カーソルになる — 検索や :N の着地点が「今どこを見ているか」そのものなので
    pub(super) fn center_on(&mut self, line: usize) {
        let last = self.line_count().saturating_sub(1);
        self.set_cursor(line);
        self.viewport.center_on(line, last);
        self.ensure_cursor_visible();
    }

    fn compute_matches(&self, query: &str) -> Vec<Match> {
        let Some(open) = &self.current else {
            return Vec::new();
        };
        let Content::Text(doc) = open.content.as_ref() else {
            return Vec::new();
        };
        search_matches(&doc.plain, query)
    }

    // ファイルを開き直した/reload した際、同じクエリでマッチを再計算する。
    // 確定済みだった場合は現在位置を新しいマッチ数に合わせてクランプする
    pub(super) fn recompute_search(&mut self) {
        let Some(query) = self.search.as_ref().map(|s| s.query.clone()) else {
            return;
        };
        let matches = self.compute_matches(&query);
        if let Some(search) = &mut self.search {
            let current = search
                .current
                .map(|idx| idx.min(matches.len().saturating_sub(1)));
            search.current = if matches.is_empty() { None } else { current };
            search.matches = matches;
        }
    }
}

// smart-case (クエリが全て小文字なら大小無視、大文字を含めば区別) の部分一致検索。
// 大小無視の比較は ASCII の範囲だけ行う (to_ascii_lowercase は char 数を変えないため、
// plain の char 列インデックスと桁位置が確実に一致する)。
// GIT レーンの diff 内検索 (#31, component/gitlane/) も同じマッチングを再利用するため pub(crate)。
// diff は plain な文字列を持たないので、呼び出し側が Line の span[1..] を連結して渡す
pub(crate) fn search_matches(plain: &[String], query: &str) -> Vec<Match> {
    if query.is_empty() {
        return Vec::new();
    }
    plain
        .iter()
        .enumerate()
        .flat_map(|(line, text)| {
            line_matches(text, query).map(move |(start_col, end_col)| Match {
                line,
                start_col,
                end_col,
            })
        })
        .collect()
}

/// 1 行ぶんの一致 (start_col, end_col) を先頭から順に返す。search_matches の行単位の中身で、
/// 横断検索 (component/grep/) が 1 ファイルあたりの上限まで `take` で打ち切れるよう
/// イテレータとして分けてある — 巨大な 1 行 (minified な JS 等) で全マッチを確保してから
/// 捨てる、を避けるため。規則 (smart-case・ASCII 畳み込み) はここが唯一の定義
pub(crate) fn line_matches(text: &str, query: &str) -> impl Iterator<Item = (usize, usize)> {
    let ignore_case = !query.chars().any(|c| c.is_uppercase());
    let needle: Vec<char> = fold_case(query, ignore_case).collect();
    let haystack: Vec<char> = fold_case(text, ignore_case).collect();
    let last_start = if needle.is_empty() || haystack.len() < needle.len() {
        // 空クエリは何にも一致しない扱い (search_matches の入口でも弾いている)
        0
    } else {
        haystack.len() - needle.len() + 1
    };
    (0..last_start).filter_map(move |start| {
        (haystack[start..start + needle.len()] == needle[..])
            .then_some((start, start + needle.len()))
    })
}

fn fold_case(s: &str, ignore_case: bool) -> impl Iterator<Item = char> + '_ {
    s.chars().map(move |c| {
        if ignore_case {
            c.to_ascii_lowercase()
        } else {
            c
        }
    })
}
