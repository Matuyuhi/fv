//! ハイライト済み表示行の遅延生成。
//!
//! 「文書全体を Vec<Line> に焼く」のをやめ、**画面に映る 1 枚分だけ**をその場で組み立てる。
//! syntect のハイライトは前の行の状態に依存する逐次処理なので、任意の行から再開できるよう
//! CHECKPOINT_STRIDE 行ごとにパーサ状態 (LineState) を保存しておき、可視範囲の直前にある
//! checkpoint から助走して必要な行だけを作る。これで
//! - ファイルを開くコストがファイルの大きさに比例しなくなる (画面 1 枚分 + 先頭からの助走)
//! - 編集の再ハイライトが「変更行以降の全部」ではなく「画面 1 枚分」で済む
//!
//! の両方が同じ 1 つの仕組みで片付く。

use std::path::{Path, PathBuf};

use ratatui::text::{Line, Span};

use super::Viewport;
use super::highlight::{Highlighter, LineState, gutter_span};
use crate::text;

/// パーサ状態を保存する行間隔。小さくすると助走が短くなる代わりに保存する状態が増える。
/// 助走 (最大 STRIDE 行) は画面 1 枚分の描画と同程度に収まる大きさを選んである
const CHECKPOINT_STRIDE: usize = 128;

/// ハイライト対象の行ソース。生の行 (タブ・改行を加工しない) を借りるだけで所有しない —
/// 閲覧は Content、編集は EditBuffer と持ち主が違うため
pub struct LineSource<'a> {
    pub lines: &'a [String],
    /// 最終行の後ろに改行が続くか。syntect へ渡す行末を元のテキストと一致させる
    pub trailing_newline: bool,
}

/// 可視範囲のハイライト済み行を保持するキャッシュ。閲覧 (Viewer) と編集 (EditState) が
/// それぞれ 1 つずつ持つ
pub struct HighlightCache {
    path: PathBuf,
    /// syntect を通さずプレーン表示にする (巨大ファイル)
    plain: bool,
    /// checkpoints[k] = 行 k * CHECKPOINT_STRIDE を解析する直前の状態。先頭から詰めて持ち、
    /// 未計算の分は「まだ無い」= 末尾より後ろとして表す
    checkpoints: Vec<LineState>,
    /// 直近に組み立てた可視ウィンドウ。同じ範囲の再描画では作り直さない
    rows: Vec<Line<'static>>,
    start: usize,
    valid: bool,
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
            checkpoints: Vec::new(),
            rows: Vec::new(),
            start: 0,
            valid: false,
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

    /// line 以降を作り直す (編集)。line より手前から始まる checkpoint は行番号がずれないので
    /// 残せる — これが「変更行より前を再ハイライトしない」ことの担保
    pub fn invalidate_from(&mut self, line: usize) {
        self.checkpoints.truncate(line / CHECKPOINT_STRIDE + 1);
        self.valid = false;
    }

    fn discard(&mut self) {
        self.checkpoints.clear();
        self.rows.clear();
        self.valid = false;
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
        if !self.valid
            || self.start != start
            || self.rows.len() != count
            || self.gutter_width != gutter_width
        {
            self.build(highlighter, &src, start, count, gutter_width);
        }
        (&self.rows, self.start)
    }

    fn build(
        &mut self,
        highlighter: &Highlighter,
        src: &LineSource<'_>,
        start: usize,
        count: usize,
        gutter_width: usize,
    ) {
        self.rows.clear();
        self.start = start;
        self.gutter_width = gutter_width;
        self.valid = true;
        let end = start + count;
        if self.plain {
            for i in start..end {
                self.rows.push(Line::from(vec![
                    gutter_span(i + 1, gutter_width),
                    Span::raw(text::normalize(&src.lines[i])),
                ]));
            }
            return;
        }

        let first_line = src.lines.first().map(String::as_str).unwrap_or("");
        let session = highlighter.session(&self.path, first_line);
        if self.checkpoints.is_empty() {
            self.checkpoints.push(session.start());
        }
        // 可視範囲の直前にある最新の checkpoint から助走する。まだ届いていない範囲を
        // 要求された時 (末尾へのジャンプ等) はここで初めて先頭から歩くが、その過程で
        // checkpoint が埋まるので同じ範囲の 2 回目からは助走も STRIDE 行で済む
        let resume = (start / CHECKPOINT_STRIDE).min(self.checkpoints.len() - 1);
        let mut state = self.checkpoints[resume].clone();
        let mut raw = String::new();
        for i in resume * CHECKPOINT_STRIDE..end {
            if i.is_multiple_of(CHECKPOINT_STRIDE)
                && i / CHECKPOINT_STRIDE == self.checkpoints.len()
            {
                self.checkpoints.push(state.clone());
            }
            raw.clear();
            raw.push_str(&src.lines[i]);
            if i + 1 < src.lines.len() || src.trailing_newline {
                raw.push('\n');
            }
            if i < start {
                session.skip(&raw, &mut state);
                continue;
            }
            let mut spans = vec![gutter_span(i + 1, gutter_width)];
            session.line(&raw, &mut state, &mut spans);
            self.rows.push(Line::from(spans));
        }
    }
}
