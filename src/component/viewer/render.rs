//! ハイライト済み表示行の遅延生成。
//!
//! 「文書全体を Vec<Line> に焼く」のをやめ、**画面に映る範囲だけ**をその場で組み立てる。
//! syntect のハイライトは前の行の状態に依存する逐次処理なので、任意の行から再開できるよう
//! CHECKPOINT_STRIDE 行ごとにパーサ状態 (LineState) を保存しておき、可視範囲の直前にある
//! checkpoint から助走して必要な行だけを作る。これで
//! - ファイルを開くコストがファイルの大きさに比例しなくなる (画面 1 枚分 + 先頭からの助走)
//! - 編集の再ハイライトが「変更行以降の全部」ではなく画面の中で済む
//!
//! の両方が同じ 1 つの仕組みで片付く。
//!
//! さらに、組み立て済みの行は**可視範囲そのものではなく前後に余白を持つ帯 (Band)** として
//! 保持し、行ごとの「その行を解析する直前の状態」も一緒に持つ。これは 1 打鍵ぶんの仕事を
//! 画面の大きさから切り離すため:
//! - `j`/`k` の 1 行スクロールは、帯の端に 1 行足すだけで済む (画面 1 枚を作り直さない)
//! - 1 文字のタイピングは、変更行を作り直してパーサ状態が元へ戻った時点で打ち切れる
//!   (文字列やコメントを開閉しない限り、実際に作り直すのはその 1 行だけ)
//!
//! どちらも「再描画のコストを画面の大きさより上に持ち上げない」(CLAUDE.md) の一歩先で、
//! **1 打鍵で実際に変わった行数**にまでコストを落とすためのもの。

use std::path::{Path, PathBuf};

use ratatui::text::{Line, Span};

use super::Viewport;
use super::highlight::{Highlighter, LineState, Session, gutter_span};
use crate::text;

/// パーサ状態を保存する行間隔。小さくすると助走が短くなる代わりに、保存する状態と
/// 保存そのもののコスト (LineState の clone) が増える。帯の下端をこの境界に揃える
/// (Band::prepend) ので、上方向のスクロールで助走を払うのは STRIDE 行に 1 度だけになる
const CHECKPOINT_STRIDE: usize = 32;

/// 可視範囲の前後に保持しておく行数。スクロールの先で帯を作り直さずに済ませるための
/// 遊びで、大きくしても保持するのは行と状態だけ (組み立ては端に足す 1 行ぶん) なので
/// 1 打鍵のコストには乗らない。STRIDE の倍数にして帯の下端の切り上げと噛み合わせる
const BAND_SLACK: usize = 4 * CHECKPOINT_STRIDE;

/// ハイライト対象の行ソース。生の行 (タブ・改行を加工しない) を借りるだけで所有しない —
/// 閲覧は Content、編集は EditBuffer と持ち主が違うため
pub struct LineSource<'a> {
    pub lines: &'a [String],
    /// 最終行の後ろに改行が続くか。syntect へ渡す行末を元のテキストと一致させる
    pub trailing_newline: bool,
}

/// 編集で変わった行の範囲。起点だけでなく終点と「行番号がずれたか」も持つのは、
/// ハイライトの作り直しを打ち切れる条件がこの 2 つで決まるため (Band::repaint)。
/// カーソル位置から推測せず編集プリミティブ側で記録するのは、undo/redo が任意の位置へ飛ぶから
#[derive(Clone, Copy)]
pub struct Touched {
    /// 中身が変わった最初の行
    pub from: usize,
    /// 中身が変わった最後の行。ここより後ろは中身が変わっていないので、パーサ状態さえ
    /// 元へ戻れば組み立て済みの行をそのまま使える。shifted が true の間は意味を持たない
    pub to: usize,
    /// 行の挿入・削除で以降の行番号がずれた。ずれると帯の同じ位置が別の行を指すので、
    /// 「中身が変わっていない」という判断そのものが成り立たなくなる
    pub shifted: bool,
}

impl Touched {
    pub(crate) fn line(line: usize) -> Self {
        Self {
            from: line,
            to: line,
            shifted: false,
        }
    }

    /// 別の変更を取り込む (描画を挟まずに複数回の編集が届くことがある)。
    /// 範囲は広い方へ、ずれは片方でも起きていれば真
    pub(crate) fn merge(&mut self, other: Self) {
        self.from = self.from.min(other.from);
        self.to = self.to.max(other.to);
        self.shifted |= other.shifted;
    }

    /// 行が増減した (以降の行番号がずれた)
    pub(crate) fn mark_shifted(&mut self) {
        self.shifted = true;
    }
}

/// 行を 1 本組み立てるのに要る道具一式 (テーマ + 文法 + 行ソース + gutter 幅)。
/// 状態は持たないので、帯・checkpoint のどちらからも同じものを共有して呼べる
struct Painter<'a> {
    session: Session<'a>,
    src: &'a LineSource<'a>,
    gutter_width: usize,
}

impl Painter<'_> {
    fn start(&self) -> LineState {
        self.session.start()
    }

    /// 描画用の 1 行。state は「行 i の直前」で渡し、戻る時には「行 i+1 の直前」まで進む
    fn row(&self, i: usize, state: &mut LineState, raw: &mut String) -> Line<'static> {
        self.raw(i, raw);
        let mut spans = vec![gutter_span(i + 1, self.gutter_width)];
        self.session.line(raw, state, &mut spans);
        Line::from(spans)
    }

    /// 画面に出さない行を状態だけ進める (助走)
    fn skip(&self, i: usize, state: &mut LineState, raw: &mut String) {
        self.raw(i, raw);
        self.session.skip(raw, state);
    }

    // syntect へ渡す 1 行。行末の改行の有無を元テキストと一致させる。
    // 確保を使い回すため呼び出し側の String へ書き込む
    fn raw(&self, i: usize, out: &mut String) {
        out.clear();
        out.push_str(&self.src.lines[i]);
        if i + 1 < self.src.lines.len() || self.src.trailing_newline {
            out.push('\n');
        }
    }
}

/// 行 k * CHECKPOINT_STRIDE を解析する直前の状態の引き出し。先頭から詰めて持ち、
/// 未計算の分は「まだ無い」= 末尾より後ろとして表す。帯が届かない場所 (末尾へのジャンプ・
/// 帯より上へのスクロール) から再開するための土台
struct Checkpoints(Vec<LineState>);

impl Checkpoints {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn clear(&mut self) {
        self.0.clear();
    }

    /// line 以降の状態は変更の影響を受けるので捨てる。line より手前から始まる checkpoint は
    /// 行番号がずれない限りそのまま使える — これが「変更行より前を再ハイライトしない」担保
    fn truncate_from(&mut self, line: usize) {
        self.0.truncate(line / CHECKPOINT_STRIDE + 1);
    }

    /// 行 target を解析する直前の状態。手前の checkpoint から助走し、通過した境界を
    /// その過程で埋める (末尾へ飛んだ後の 2 回目以降は助走が STRIDE 行で済む)
    fn resume_at(&mut self, painter: &Painter<'_>, target: usize) -> LineState {
        if self.0.is_empty() {
            self.0.push(painter.start());
        }
        let index = (target / CHECKPOINT_STRIDE).min(self.0.len() - 1);
        let mut state = self.0[index].clone();
        let mut raw = String::new();
        for i in index * CHECKPOINT_STRIDE..target {
            self.note(i, &state);
            painter.skip(i, &mut state, &mut raw);
        }
        self.note(target, &state);
        state
    }

    // 先頭から詰めて持つので、埋められるのは常に「今の末尾のすぐ次」だけ
    fn note(&mut self, line: usize, state: &LineState) {
        if line.is_multiple_of(CHECKPOINT_STRIDE) && line / CHECKPOINT_STRIDE == self.0.len() {
            self.0.push(state.clone());
        }
    }
}

/// 組み立て済みの行の帯。可視範囲そのものではなく前後に余白を持つので、1 行ぶんの
/// スクロールは端に 1 行足すだけで済む
struct Band {
    /// 論理行 [start, start + rows.len()) のハイライト済み行
    rows: Vec<Line<'static>>,
    /// states[k] = 行 start + k を解析する**直前**の状態。rows より 1 つ多く持ち、
    /// 末尾は「帯の次の行」の状態 = 下へ継ぎ足す時の再開点になる。
    /// 行ごとに持つのは、編集の作り直しを「元の状態に戻ったか」で打ち切るため (repaint)
    states: Vec<LineState>,
    start: usize,
}

impl Band {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            states: Vec::new(),
            start: 0,
        }
    }

    fn end(&self) -> usize {
        self.start + self.rows.len()
    }

    /// 行が 0 本でも「start の直前の状態」を持っていれば帯として繋げられる
    /// (起動直後の高さ 0 のフレームがこれにあたる)
    fn anchored(&self) -> bool {
        !self.states.is_empty()
    }

    fn covers(&self, start: usize, end: usize) -> bool {
        self.anchored() && self.start <= start && end <= self.end()
    }

    fn clear(&mut self) {
        self.rows.clear();
        self.states.clear();
    }

    /// 帯を捨てて [start, end) から作り直す。飛び先が帯と繋がっていない時だけ通る経路
    fn rebuild(
        &mut self,
        painter: &Painter<'_>,
        start: usize,
        end: usize,
        checkpoints: &mut Checkpoints,
    ) {
        self.clear();
        self.start = start;
        self.states.push(checkpoints.resume_at(painter, start));
        self.append(painter, end, checkpoints);
    }

    /// 下端へ行 to まで継ぎ足す。末尾の状態から続けるだけなので助走が要らない
    fn append(&mut self, painter: &Painter<'_>, to: usize, checkpoints: &mut Checkpoints) {
        let mut state = self.states.pop().expect("帯は必ず末尾の状態を持つ");
        let mut raw = String::new();
        for i in self.end()..to {
            checkpoints.note(i, &state);
            self.states.push(state.clone());
            self.rows.push(painter.row(i, &mut state, &mut raw));
        }
        self.states.push(state);
    }

    /// 上端へ行 from まで継ぎ足す。上へは末尾の状態を使えないので checkpoint から助走するが、
    /// 助走で歩いた行はそのまま帯に足すので捨てる仕事にはならない
    fn prepend(&mut self, painter: &Painter<'_>, from: usize, checkpoints: &mut Checkpoints) {
        let count = self.start - from;
        let mut state = checkpoints.resume_at(painter, from);
        let mut rows = Vec::with_capacity(count);
        let mut states = Vec::with_capacity(count);
        let mut raw = String::new();
        for i in from..self.start {
            checkpoints.note(i, &state);
            states.push(state.clone());
            rows.push(painter.row(i, &mut state, &mut raw));
        }
        // 継ぎ目の状態は帯側が既に持っている (パースは文書の先頭から決まるので必ず一致する)
        debug_assert!(
            self.states.first().is_none_or(|joint| *joint == state),
            "継ぎ足した先の状態が帯と食い違っている"
        );
        rows.append(&mut self.rows);
        states.append(&mut self.states);
        self.rows = rows;
        self.states = states;
        self.start = from;
    }

    /// 編集で変わった範囲を帯へ反映する
    fn repaint(&mut self, painter: &Painter<'_>, touched: Touched) {
        if !self.anchored() {
            return;
        }
        if touched.from < self.start {
            // 帯より手前が変わった: 帯の中に信用できる行が 1 つも残らない
            self.clear();
            return;
        }
        if touched.from >= self.end() {
            // 帯より後ろ: 行も、末尾の状態 (= 行 end() の直前) もそのまま使える
            return;
        }
        let k = touched.from - self.start;
        if touched.shifted {
            // 行が増減して以降の行番号がずれた。同じ位置が別の行を指すので繋ぎ直せない
            self.rows.truncate(k);
            self.states.truncate(k + 1);
            return;
        }
        let mut state = self.states[k].clone();
        let mut raw = String::new();
        for i in touched.from..self.end() {
            let j = i - self.start;
            self.rows[j] = painter.row(i, &mut state, &mut raw);
            // 中身が変わったのは [from, to] だけなので、そこを作り直した後にパーサ状態が
            // 元へ戻れば以降の行は色も変わらない。1 文字打つたびに画面 1 枚を
            // 作り直さずに済むのはこの打ち切りによる
            if i >= touched.to && state == self.states[j + 1] {
                return;
            }
            self.states[j + 1] = state.clone();
        }
    }

    /// 可視範囲の前後 BAND_SLACK 行に収める。落とすのは要求範囲から遠い側だけなので、
    /// 帯はスクロールの向きに付いてくる
    fn trim(&mut self, start: usize, end: usize) {
        let keep_from = start.saturating_sub(BAND_SLACK);
        if self.start < keep_from {
            let drop = keep_from - self.start;
            self.rows.drain(..drop);
            self.states.drain(..drop);
            self.start = keep_from;
        }
        let keep_to = end.saturating_add(BAND_SLACK);
        if self.end() > keep_to {
            let len = keep_to - self.start;
            self.rows.truncate(len);
            self.states.truncate(len + 1);
        }
    }
}

/// 可視範囲のハイライト済み行を保持するキャッシュ。閲覧 (Viewer) と編集 (EditState) が
/// それぞれ 1 つずつ持つ
pub struct HighlightCache {
    path: PathBuf,
    /// syntect を通さずプレーン表示にする (巨大ファイル)
    plain: bool,
    checkpoints: Checkpoints,
    band: Band,
    /// 編集で作り直しが要る範囲。次の描画で帯へ反映する (キー処理では色を触らない)
    dirty: Option<Touched>,
    /// rows を組み立てた時点の gutter 幅。行数の増減で変わったら作り直す
    gutter_width: usize,
}

impl Default for HighlightCache {
    fn default() -> Self {
        Self::new()
    }
}

impl HighlightCache {
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
            plain: false,
            checkpoints: Checkpoints::new(),
            band: Band::new(),
            dirty: None,
            gutter_width: 0,
        }
    }

    /// 対象ファイルを差し替えて全て捨てる (open / reload)
    pub fn reset(&mut self, path: &Path, plain: bool) {
        self.path = path.to_path_buf();
        self.plain = plain;
        self.discard();
    }

    /// テーマ差し替え。対象は変わらないが色もパーサ状態も作り直す
    pub fn invalidate_all(&mut self) {
        self.discard();
    }

    /// 編集の反映を予約する。ここでは色を作らず範囲を畳んでおくだけで、実際の作り直しは
    /// 次の描画で可視範囲のぶんだけ走る
    pub fn invalidate_from(&mut self, touched: Touched) {
        self.checkpoints.truncate_from(touched.from);
        match &mut self.dirty {
            Some(pending) => pending.merge(touched),
            None => self.dirty = Some(touched),
        }
    }

    fn discard(&mut self) {
        self.checkpoints.clear();
        self.band.clear();
        self.dirty = None;
    }

    /// vp.scroll から画面 1 枚分の行を返す。戻り値は (可視行, 先頭行の論理 index)。
    /// wrap 中でも 1 論理行は最低 1 視覚行を占めるので、height 論理行あれば画面は必ず埋まる
    pub fn rows(
        &mut self,
        highlighter: &Highlighter,
        src: LineSource<'_>,
        vp: &Viewport,
    ) -> (&[Line<'static>], usize) {
        let total = src.lines.len();
        let start = vp.scroll.min(total.saturating_sub(1));
        let count = vp.height.min(total.saturating_sub(start));
        let gutter_width = text::gutter_width(total);
        // 行数の増減で行番号の桁が変わると、組み立て済みの行は全て gutter がずれている
        if self.gutter_width != gutter_width {
            self.gutter_width = gutter_width;
            self.discard();
        }
        if self.plain {
            self.build_plain(&src, start, count);
            return (&self.band.rows, self.band.start);
        }
        // 帯が要求範囲を覆っていて編集の持ち越しも無ければ、組み立てる仕事は 1 行も無い。
        // session (テーマのセレクタ展開) すら作らずに済ませる
        if self.dirty.is_some() || !self.band.covers(start, start + count) {
            let first_line = src.lines.first().map(String::as_str).unwrap_or("");
            let painter = Painter {
                session: highlighter.session(&self.path, first_line),
                src: &src,
                gutter_width,
            };
            self.sync(&painter, start, count);
        }
        let offset = start - self.band.start;
        (&self.band.rows[offset..offset + count], start)
    }

    fn sync(&mut self, painter: &Painter<'_>, start: usize, count: usize) {
        let end = start + count;
        if let Some(touched) = self.dirty.take() {
            self.band.repaint(painter, touched);
        }
        if !self.band.anchored() || end < self.band.start || start > self.band.end() {
            // 帯と繋がっていない飛び先 (別の場所へのジャンプ・帯を丸ごと捨てた直後)
            self.band
                .rebuild(painter, start, end, &mut self.checkpoints);
        } else {
            if start < self.band.start {
                // 下端は checkpoint 境界まで一気に作る。そこまで持っておけば次の 1 行ぶんの
                // 上スクロールは帯の中で足り、助走を払うのは STRIDE 行に 1 度で済む
                let target = (start / CHECKPOINT_STRIDE) * CHECKPOINT_STRIDE;
                self.band.prepend(painter, target, &mut self.checkpoints);
            }
            if end > self.band.end() {
                self.band.append(painter, end, &mut self.checkpoints);
            }
        }
        self.band.trim(start, end);
    }

    // syntect を通さない巨大ファイル。行あたりの仕事が元々小さいので帯は持たず、
    // 要求が変わった時だけ画面 1 枚を組み直す
    fn build_plain(&mut self, src: &LineSource<'_>, start: usize, count: usize) {
        let stale = self.dirty.take().is_some()
            || self.band.start != start
            || self.band.rows.len() != count;
        if !stale {
            return;
        }
        self.band.rows.clear();
        self.band.start = start;
        for i in start..start + count {
            self.band.rows.push(Line::from(vec![
                gutter_span(i + 1, self.gutter_width),
                Span::raw(text::normalize(&src.lines[i])),
            ]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHECKPOINT_STRIDE, HighlightCache, LineSource, Touched};
    use crate::component::viewer::{Highlighter, Viewport};
    use ratatui::text::Line;
    use std::path::Path;

    const PATH: &str = "sample.rs";

    // syntect の読み込みは重いので、テストの中で 1 度だけ作って共有する
    fn highlighter() -> &'static Highlighter {
        use std::sync::OnceLock;
        static ONCE: OnceLock<Highlighter> = OnceLock::new();
        ONCE.get_or_init(Highlighter::new)
    }

    // 文字列・ブロックコメントを跨ぐ行を混ぜる (パーサ状態が行を越えて続く形を作る)
    fn document(lines: usize) -> Vec<String> {
        (0..lines)
            .map(|i| match i % 4 {
                0 => format!("/// doc comment {i}"),
                1 => format!("pub fn item_{i}(x: &str) -> String {{ format!(\"v {{x}} {i}\") }}"),
                2 => format!("    let s_{i} = \"text with {{braces}} and {i}\";"),
                _ => format!("    // trailing note {i}"),
            })
            .collect()
    }

    fn source(lines: &[String]) -> LineSource<'_> {
        LineSource {
            lines,
            trailing_newline: true,
        }
    }

    fn viewport(scroll: usize, height: usize) -> Viewport {
        let mut vp = Viewport::new(false);
        vp.scroll = scroll;
        vp.height = height;
        vp.width = 100;
        vp
    }

    fn plain(rows: &[Line<'static>]) -> Vec<String> {
        rows.iter()
            .map(|row| row.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    // 「作り直さずに済ませた行」と「先頭から作り直した行」が同じであること。
    // 帯の継ぎ足し・打ち切りの正しさは全てこの 1 つの性質に落ちる
    fn assert_matches_fresh(cache: &mut HighlightCache, lines: &[String], vp: &Viewport) {
        let mut fresh = HighlightCache::new();
        fresh.reset(Path::new(PATH), false);
        let (want, want_first) = fresh.rows(highlighter(), source(lines), vp);
        let want = (plain(want), want_first);
        let (got, got_first) = cache.rows(highlighter(), source(lines), vp);
        assert_eq!((plain(got), got_first), want);
    }

    fn opened(lines: &[String], vp: &Viewport) -> HighlightCache {
        let mut cache = HighlightCache::new();
        cache.reset(Path::new(PATH), false);
        cache.rows(highlighter(), source(lines), vp);
        cache
    }

    // 1 行ずつのスクロールは帯の端を継ぎ足すだけになるが、見える行は毎回
    // 先頭から組み立てたものと一致していなければならない
    #[test]
    fn scrolling_line_by_line_keeps_the_same_rows_as_a_fresh_build() {
        let lines = document(400);
        let height = 30;
        let mut cache = opened(&lines, &viewport(0, height));
        // 下へ: 帯の末尾の状態から 1 行ずつ継ぎ足す
        for scroll in 1..120 {
            assert_matches_fresh(&mut cache, &lines, &viewport(scroll, height));
        }
        // 上へ: checkpoint 境界まで戻る経路と、帯の中で足りる経路の両方を通す
        for scroll in (0..119).rev() {
            assert_matches_fresh(&mut cache, &lines, &viewport(scroll, height));
        }
    }

    // 帯から離れた場所へ飛んだら作り直す。戻ってきた時も同じ行が出る
    #[test]
    fn jumping_far_away_and_back_rebuilds_correctly() {
        let lines = document(400);
        let height = 20;
        let mut cache = opened(&lines, &viewport(0, height));
        for scroll in [370, 0, 200, 199, 201, 370, 5] {
            assert_matches_fresh(&mut cache, &lines, &viewport(scroll, height));
        }
    }

    // 帯より広い高さ (リサイズ) を要求されても覆い直せる
    #[test]
    fn growing_the_viewport_extends_the_band() {
        let lines = document(400);
        let mut cache = opened(&lines, &viewport(100, 10));
        for height in [10, 60, 5, 120] {
            assert_matches_fresh(&mut cache, &lines, &viewport(100, height));
        }
    }

    // 1 行の中だけの編集: 作り直しを打ち切っても、行は先頭から作ったものと一致する
    #[test]
    fn editing_one_line_keeps_the_rest_of_the_band_correct() {
        let mut lines = document(400);
        let height = 30;
        let vp = viewport(100, height);
        let mut cache = opened(&lines, &vp);
        for (i, line) in [100usize, 115, 129, 100].into_iter().enumerate() {
            lines[line].push_str(&format!(" // edit {i}"));
            cache.invalidate_from(Touched::line(line));
            assert_matches_fresh(&mut cache, &lines, &vp);
        }
    }

    // 文字列を開いたまま行を終える編集はパーサ状態を変えるので、打ち切ってはいけない。
    // 再収束の判定が甘いとここで後続行の色が古いまま残る
    #[test]
    fn an_edit_that_changes_the_parser_state_repaints_the_lines_below() {
        let mut lines = document(400);
        let vp = viewport(100, 30);
        let mut cache = opened(&lines, &vp);
        // 行末に閉じない文字列を足すと、以降の行が文字列の中に入る
        lines[105] = "    let s = \"unterminated".to_string();
        cache.invalidate_from(Touched::line(105));
        assert_matches_fresh(&mut cache, &lines, &vp);
        // 元へ戻すと以降の行も戻る
        lines[105] = document(400)[105].clone();
        cache.invalidate_from(Touched::line(105));
        assert_matches_fresh(&mut cache, &lines, &vp);
    }

    // 行の増減は帯の同じ位置が別の行を指すようになるので、そこから先は繋ぎ直せない
    #[test]
    fn inserting_and_removing_lines_invalidates_the_band_below() {
        let mut lines = document(400);
        let vp = viewport(100, 30);
        let mut cache = opened(&lines, &vp);

        let mut inserted = Touched::line(110);
        inserted.mark_shifted();
        lines.insert(110, "    let added = \"x\";".to_string());
        cache.invalidate_from(inserted);
        assert_matches_fresh(&mut cache, &lines, &vp);

        let mut removed = Touched::line(103);
        removed.mark_shifted();
        lines.remove(103);
        cache.invalidate_from(removed);
        assert_matches_fresh(&mut cache, &lines, &vp);
    }

    // 帯より手前の編集は帯の中に信用できる行を残さない
    #[test]
    fn editing_above_the_band_discards_it() {
        let mut lines = document(400);
        let vp = viewport(200, 30);
        let mut cache = opened(&lines, &vp);
        lines[3] = "/* block comment opened".to_string();
        cache.invalidate_from(Touched::line(3));
        assert_matches_fresh(&mut cache, &lines, &vp);
    }

    // 描画を挟まずに複数の編集が届いた場合、範囲は広い方へ畳まれる
    #[test]
    fn several_edits_before_a_draw_are_merged() {
        let mut lines = document(400);
        let vp = viewport(100, 30);
        let mut cache = opened(&lines, &vp);
        lines[120].push_str(" // late");
        cache.invalidate_from(Touched::line(120));
        lines[104] = "    let s = \"unterminated".to_string();
        cache.invalidate_from(Touched::line(104));
        assert_matches_fresh(&mut cache, &lines, &vp);
    }

    // 帯は可視範囲の前後 BAND_SLACK 行までに収まる (スクロールし続けても際限なく伸びない)
    #[test]
    fn the_band_stays_bounded_while_scrolling() {
        let lines = document(2000);
        let height = 30;
        let mut cache = opened(&lines, &viewport(0, height));
        for scroll in 0..600 {
            cache.rows(highlighter(), source(&lines), &viewport(scroll, height));
        }
        let bound = height + 2 * super::BAND_SLACK + CHECKPOINT_STRIDE;
        assert!(
            cache.band.rows.len() <= bound,
            "帯が {} 行まで伸びている (上限 {bound})",
            cache.band.rows.len()
        );
        assert_eq!(cache.band.states.len(), cache.band.rows.len() + 1);
    }

    // 行数が桁を跨ぐと gutter の幅が変わるので、組み立て済みの行は使い回せない
    #[test]
    fn a_gutter_width_change_rebuilds_the_rows() {
        let lines = document(99);
        let vp = viewport(0, 10);
        let mut cache = opened(&lines, &vp);
        let longer = document(120);
        assert_matches_fresh(&mut cache, &longer, &vp);
    }
}
