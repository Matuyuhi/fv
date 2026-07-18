use ratatui::widgets::ListState;

/// フィルタ後の1候補。positions は candidate 文字列内でマッチした char インデックス
/// (ハイライト表示に使う。クエリが空のときは空)
pub struct FinderMatch {
    pub candidate: usize,
    pub positions: Vec<usize>,
    score: i64,
}

/// Ctrl+p ファジーファインダーの状態。候補一覧は起動時に一度だけ構築し、
/// クエリ入力の都度 rescan で線形走査してスコアリングし直す
/// (数千件規模でも実用速度で足りるため、索引構築等の最適化はしない)
pub struct Finder {
    // root からの相対パス文字列。起動時に一度だけ構築し、以降は不変
    candidates: Vec<String>,
    pub query: String,
    // クエリでフィルタし、スコア降順 (クエリ空ならパス昇順) に並べたもの
    pub matches: Vec<FinderMatch>,
    pub selected: usize,
    pub list_state: ListState,
}

impl Finder {
    pub fn new(candidates: Vec<String>) -> Self {
        let mut candidates = candidates;
        candidates.sort();
        let mut finder = Self {
            candidates,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
        };
        finder.rescan();
        finder
    }

    pub fn total(&self) -> usize {
        self.candidates.len()
    }

    pub fn candidate_path(&self, idx: usize) -> Option<&str> {
        self.candidates.get(idx).map(String::as_str)
    }

    pub fn selected_path(&self) -> Option<&str> {
        let m = self.matches.get(self.selected)?;
        self.candidate_path(m.candidate)
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.rescan();
    }

    pub fn backspace(&mut self) {
        self.query.pop();
        self.rescan();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.matches.is_empty() {
            return;
        }
        let last = self.matches.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    fn rescan(&mut self) {
        self.matches = if self.query.is_empty() {
            // 空クエリはスコアリングせず、事前ソート済みの candidates をそのままパス昇順で使う
            (0..self.candidates.len())
                .map(|i| FinderMatch {
                    candidate: i,
                    positions: Vec::new(),
                    score: 0,
                })
                .collect()
        } else {
            let mut scored: Vec<FinderMatch> = self
                .candidates
                .iter()
                .enumerate()
                .filter_map(|(i, path)| {
                    let (score, positions) = fuzzy_match(path, &self.query)?;
                    Some(FinderMatch {
                        candidate: i,
                        positions,
                        score,
                    })
                })
                .collect();
            scored.sort_by_key(|m| std::cmp::Reverse(m.score));
            scored
        };
        self.selected = self.selected.min(self.matches.len().saturating_sub(1));
    }
}

// クエリの各文字を candidate 内で順序を保って探す (大小無視)。マッチしなければ None。
// 大小無視は既存の検索実装 (viewer.rs の fold_case) と同じ理由で to_ascii_lowercase を使う:
// char 数を変えないため、返す positions がそのまま元の文字列の char インデックスとして使える。
//
// 各クエリ文字は「直前の一致位置より後で最初に現れる位置」を貪欲に選ぶ2ポインタ法。
// 最適なアラインメントを保証するものではないが、クエリを連続入力すれば自然と
// 連続一致になるため実用上は十分で、数千件を毎キー入力で線形走査しても軽い
fn fuzzy_match(candidate: &str, query: &str) -> Option<(i64, Vec<usize>)> {
    let hay: Vec<char> = candidate.chars().map(|c| c.to_ascii_lowercase()).collect();
    let needle: Vec<char> = query.chars().map(|c| c.to_ascii_lowercase()).collect();
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }

    let mut positions = Vec::with_capacity(needle.len());
    let mut cursor = 0usize;
    for &qc in &needle {
        let pos = hay[cursor..].iter().position(|&hc| hc == qc)? + cursor;
        positions.push(pos);
        cursor = pos + 1;
    }

    Some((score(&hay, &positions), positions))
}

// スコアリングの根拠 (fzf 等の一般的なファジーマッチと同じ発想を簡略化したもの):
// (a) 連続一致は「意図した部分文字列」を捉えている可能性が高いのでボーナス
// (b) '/' 直後や単語区切り直後の一致はファイル名・ディレクトリ名の先頭に
//     ヒットしていることが多く、視認性の高い結果になるのでボーナス
// (c) マッチ開始位置が後ろにずれるほど「無関係な前置き」が長いとみなし減点
// (d) 同程度のマッチなら短いパスの方が対象を絞り込めているとみなし優先
fn score(hay: &[char], positions: &[usize]) -> i64 {
    const CONSECUTIVE_BONUS: i64 = 15;
    const BOUNDARY_BONUS: i64 = 10;

    let mut s = 0i64;
    for (i, &pos) in positions.iter().enumerate() {
        if i > 0 && pos == positions[i - 1] + 1 {
            s += CONSECUTIVE_BONUS;
        }
        let is_boundary = pos == 0 || matches!(hay[pos - 1], '/' | '_' | '-' | '.' | ' ');
        if is_boundary {
            s += BOUNDARY_BONUS;
        }
    }
    // positions は昇順なので先頭が最初のマッチ位置
    s -= positions[0] as i64;
    s -= hay.len() as i64;
    s
}
