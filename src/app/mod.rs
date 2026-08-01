mod branch_ops;
mod commit;
mod git_ops;
mod github_keys;
mod keys;
mod mode;
mod mouse;

pub use mode::{
    ConfirmAction, Focus, InputKind, Lane, Mode, SETTINGS_ROWS, SettingsState, Workspace,
};

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::config::Config;
use crate::editor::EditState;
use crate::git::{self, GitStatus, StatusKind};
use crate::github;
use crate::gitview::GitState;
use crate::index::FileIndex;
use crate::issuesview::IssuesState;
use crate::job;
use crate::logview::LogState;
use crate::prsview::PrsState;
use crate::tree::Tree;
use crate::viewer::{self, Viewer};
use crate::watch::FsWatcher;

// イベント嵐 (git checkout やビルド等) でツリーを毎回フル再走査しないための間引き間隔
const RESCAN_DEBOUNCE: Duration = Duration::from_millis(500);

// Space (stage/unstage トグル) のキーリピート対策。OS/端末の自動リピートは debounce より
// 十分速いので、これを下回る間隔の連打は git プロセスを起動せず無視する
const STAGE_DEBOUNCE: Duration = Duration::from_millis(150);

// App 全体の一時通知 (notice) が自動で消えるまでの表示時間
const NOTICE_DURATION: Duration = Duration::from_secs(4);

// ペイン分割の下限幅 (枠線 2 桁を含む)。左はツリーの階層インデントが、
// 右は gutter + 数十桁のコードが最低限読める幅
const MIN_TREE_WIDTH: u16 = 12;
const MIN_VIEWER_WIDTH: u16 = 24;

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    /// 持続する作業レーン (VIEW/EDIT/GIT)。Shift+Tab で循環する
    pub lane: Lane,
    /// レーンの上に重なる一時オーバーレイ。閉じてもレーンは変わらない
    pub mode: Mode,
    /// トップレベルのタブ (Lane/Mode に続く3本目の軸)。GitHub モードが無効/使えない間は
    /// 常に Viewer 固定 (workspace_available が false の間、切替キーは全て no-op にする)
    pub workspace: Workspace,
    /// issues タブ (#33) の状態。GitHub モードが無効でも構築コスト自体はゼロ (フィールドが
    /// 空のまま) なので、常に持たせて Workspace::Issues に切り替わった時だけ取得を始める
    pub issues: IssuesState,
    /// pull requests タブ (#34) の状態。issues と同じ理由で常に持たせる
    pub prs: PrsState,
    pub tree: Tree,
    pub viewer: Viewer,
    /// Finder の候補。ツリーが遅延走査になったぶん、全ファイル一覧は別に持つ
    pub file_index: FileIndex,
    // git repo でない / git 未インストールなら None のままで通常表示にフォールバックする
    pub git: Option<GitStatus>,
    /// ステータスバー常時表示用の現在ブランチ + ahead/behind。非 git repo なら None。
    /// 取得は起動時と rescan (500ms デバウンス) に相乗りさせ、描画のたびには叩かない
    pub branch_status: Option<git::BranchStatus>,
    /// レーンをまたぐ一時通知 (message, 表示開始時刻, is_error)。EditState.notice は EDIT レーン
    /// 専用の表示なのでそのまま残し、こちらは GIT の書き込み結果等レーン非依存のメッセージに使う。
    /// on_tick で期限切れにし、再描画のたびにタイマーは触らない
    pub notice: Option<(String, Instant, bool)>,
    // Nerd Font アイコン表示。起動時に確定し実行中は変わらない (判定は main 側)
    pub icons: bool,
    pub should_quit: bool,
    // g 待ち状態。Mode を増やすほどのものではないので App の小さなフラグで持つ
    pub pending_g: bool,
    // マウスのヒットテスト用。ui::draw が毎フレーム書き戻す (viewport の実測値と同じパターン)
    pub tree_area: Rect,
    pub viewer_area: Rect,
    // 左右ペインの境界 (両ペインの枠線 2 桁)。ドラッグの掴み判定用に ui::draw が書き戻す
    pub splitter_area: Rect,
    /// タブバーの各タブの矩形 (workspace_available が false の間は全て空)。
    /// ui::tab_bar が毎フレーム書き戻し、mouse.rs のクリック判定が読む
    pub tab_areas: [Rect; Workspace::LABELS.len()],
    /// スプリッタのドラッグ中はその掴んだ桁のオフセット (0 or 1) を保持する。
    /// 掴み位置を覚えることで Down の瞬間に境界が 1 桁飛ばない
    dragging_split: Option<u16>,
    // 左ペインが画面幅に占める割合 (config に永続化)
    split_ratio: f32,
    watcher: FsWatcher,
    last_rescan: Instant,
    /// ツリーの構造 (作成・削除・リネーム) が変わった疑いがあり、全走査 (tree.rescan) が要る
    rescan_pending: bool,
    /// 内容だけの変更 (Modify(Data)) があり、git status の再取得だけで足りる。
    /// rescan_pending が同時に立っていれば全走査の方が上位互換なのでこちらは無視してよい
    status_pending: bool,
    /// 直近で stage/unstage を実行した時刻 (Space のキーリピート対策)
    last_stage_toggle: Instant,
    /// `c` で開いた通常コミットの下書き。Esc で閉じても捨てず、次に `c` を押した時に復元する
    commit_draft: Option<String>,
    /// `C` で開いた amend コミットの下書き。amend は既存メッセージのプリフィルがあるため
    /// commit_draft とは別に持つ (無ければ都度 `git log -1 --format=%B` からプリフィルする)
    amend_draft: Option<String>,
    /// GitHub モードを使いたいかどうか (起動オプション or 設定トグル)。使えるかどうかは別
    /// (github_available)。3 経路 (--github / 設定トグル / config ファイル) が結局この
    /// フラグ 1 つに集約される
    pub github_enabled: bool,
    /// config ファイルへ永続化する値。cli の --github はここへは影響しない
    /// (「その起動でだけ有効」を current_config 側で守るため、実行時フラグと分けて持つ)
    github_persisted: bool,
    /// gh の有無・認証・GitHub リモートかどうかの判定結果。起動時 (または初回有効化時) に
    /// 1 度だけ github::check_available を呼んで確定させ、以後は描画のたびに再判定しない
    pub github_available: bool,
    github_checked: bool,
    /// 実行中のリモート操作 (fetch/pull/push)。ステータスバー表示と多重起動防止
    /// (f/p/P は全て .git を触るため、実行中は新しいジョブを一切受け付けず直列化する) に使う
    pending_remote_job: Option<PendingRemoteJob>,
    /// バックグラウンドスレッドからの結果。on_tick の try_recv で drain するだけで、
    /// 専用タイマー・ブロッキング read は作らない (イベントループの既存デザインをそのまま使う)
    remote_job_rx: Option<mpsc::Receiver<git::GitOutcome>>,
}

/// 実行中のリモート操作 (f/p/P) のコンテキスト。開始時点の ahead/behind・upstream 有無を
/// 保持し、完了メッセージ (「push → origin/main (2 commits)」等) の組み立てに使う。rescan で
/// branch_status が新しい値に上書きされてしまうため、開始時点のスナップショットを別に持つ
struct PendingRemoteJob {
    kind: git::RemoteJobKind,
    branch: String,
    ahead: usize,
    behind: usize,
    had_upstream: bool,
}

impl App {
    /// github_cli は `--github` の指定。その起動限りの有効化で config には書かない
    /// (config.github との合成は github_enabled の初期値としてのみ行う)
    pub fn new(root: PathBuf, config: Config, github_cli: bool) -> Self {
        let mut tree = Tree::new(&root, config.show_hidden);
        // 再帰監視の登録は別スレッドで進む (起動を待たせない)。失敗しても
        // 監視なしで動き続ける
        let watcher = FsWatcher::new(&root, config.show_hidden);
        let git = git::file_statuses(&root);
        let branch_status = git::branch_status(&root);
        // 削除ファイルは WalkBuilder の走査に出てこないため、起動時点でも合成ノードを足しておく
        // (GIT レーンへ入る前でも status 表示・将来の選択に矛盾が出ないように)
        tree.sync_deleted(&root, &deleted_paths_of(&git));
        let mut viewer = Viewer::new();
        viewer.viewport.wrap = config.wrap_default;
        // 設定ファイルのテーマ名が壊れていても set_theme が false を返すだけで、
        // Viewer::new() が入れた既定テーマのまま起動を続ける (パニックしない)
        viewer.set_theme(&config.theme);
        let github_enabled = github_cli || config.github;
        let file_index = FileIndex::new(root.clone(), config.show_hidden);
        let mut app = Self {
            root,
            focus: Focus::Tree,
            lane: Lane::View,
            mode: Mode::Normal,
            workspace: Workspace::Viewer,
            issues: IssuesState::new(),
            prs: PrsState::new(config.wrap_default),
            tree,
            viewer,
            file_index,
            git,
            branch_status,
            notice: None,
            icons: config.icons,
            should_quit: false,
            pending_g: false,
            tree_area: Rect::default(),
            viewer_area: Rect::default(),
            splitter_area: Rect::default(),
            tab_areas: Default::default(),
            dragging_split: None,
            split_ratio: config.split_ratio,
            watcher,
            last_rescan: Instant::now(),
            rescan_pending: false,
            status_pending: false,
            last_stage_toggle: Instant::now(),
            commit_draft: None,
            amend_draft: None,
            github_enabled,
            github_persisted: config.github,
            github_available: false,
            github_checked: false,
            pending_remote_job: None,
            remote_job_rx: None,
        };
        // 判定は起動時に 1 回だけ。無効なら gh を叩くコスト自体を払わない
        if app.github_enabled {
            app.ensure_github_checked();
        }
        app
    }

    /// 保存された割合から左ペインの実桁数を求める。ui::draw と
    /// ドラッグ時の clamp が同じ定義を通るよう、換算はここ 1 箇所に閉じる
    pub fn tree_width(&self, total: u16) -> u16 {
        clamp_tree_width((self.split_ratio * total as f32).round() as u16, total)
    }

    /// ドラッグ中: 掴んだ桁が column に来るよう割合を更新する。
    /// 桁数で clamp してから割合に戻すので、下限幅に張り付いた後も割合が暴れない
    pub(super) fn set_split_at(&mut self, column: u16, grab: u16) {
        let total = self.tree_area.width + self.viewer_area.width;
        if total == 0 {
            return;
        }
        // 境界の桁 = 左ペインの右枠線なので、そこまでの桁数がそのまま左ペイン幅になる
        let target = column
            .saturating_sub(grab)
            .saturating_sub(self.tree_area.x)
            .saturating_add(1);
        self.split_ratio = clamp_tree_width(target, total) as f32 / total as f32;
    }

    /// watcher に溜まったファイル変更を取り込む。キー入力の有無に関わらず、
    /// イベントループの毎 tick (poll タイムアウト時も含む) で呼ばれる。
    /// 画面に出る状態が動いたら true。main.rs はこれを見て**変化があった時だけ再描画する**
    /// (毎ループ描くとアイドル時に CPU を数十 % 使い続けるため)。ここで true を返し忘れると
    /// 「次のキー入力まで画面が古いまま」になるので、状態を変える分岐を足したら必ず立てること
    pub fn on_tick(&mut self) -> bool {
        let mut changed = false;
        // watcher の有無に関わらず毎 tick 見る (watcher 初期化失敗時に notice が消えなくなるのを防ぐ)
        if self
            .notice
            .as_ref()
            .is_some_and(|(_, at, _)| at.elapsed() >= NOTICE_DURATION)
        {
            self.notice = None;
            changed = true;
        }
        // リモート操作 (f/p/P) の結果 drain。watcher の有無に関わらず毎 tick 見る
        // (ブロッキング read はせず、既存の 100ms poll ループにただ相乗りするだけ)
        if let Some(rx) = &self.remote_job_rx
            && let Ok(outcome) = rx.try_recv()
        {
            self.remote_job_rx = None;
            self.finish_remote_job(outcome);
            changed = true;
        }
        // issues/PR タブ (#33/#34) の list/detail/open ジョブも同じ 100ms poll ループに
        // 相乗りさせる。専用タイマーは作らない (job.rs の既存方針)
        for outcome in [self.issues.poll(), self.prs.poll()] {
            changed |= outcome.changed;
            if let Some((message, is_error)) = outcome.notice {
                self.set_notice(message, is_error);
            }
        }
        // PR の diff/CI 先読み。タイマー未到達の間は advance_prefetch が None を返すだけなので、
        // ここで changed を立てない (毎 tick true を返すとアイドル時の CPU を焼く)
        self.dispatch_pr_prefetch();
        // 背景走査が終わったら、開いたままの Finder の候補も差し替える
        // (走査中に開いた場合は読み込み済み分だけの暫定候補になっているため)
        if self.file_index.poll() {
            changed = true;
            if let Mode::Finder(finder) = &mut self.mode
                && let Some(files) = self.file_index.files()
            {
                finder.set_candidates(to_candidates(files));
            }
        }
        let changed_paths = self.watcher.drain();
        let open_path = self.viewer.current.as_ref().map(|open| open.path.clone());

        for change in &changed_paths {
            if open_path.as_deref() == Some(change.path.as_path()) {
                self.viewer.reload(&change.path);
                changed = true;
            } else if change.structural {
                // ファイルの作成・削除・リネーム。ツリーの行構成が変わりうるので全走査が要る
                self.rescan_pending = true;
            } else {
                // 内容だけの変更。ツリーの行は増減しないので、全走査せず git status の
                // 再取得 (+ GIT レーンの絞り込み・diff 更新) だけで追従させる
                self.status_pending = true;
            }
        }

        if (self.rescan_pending || self.status_pending)
            && self.last_rescan.elapsed() >= RESCAN_DEBOUNCE
        {
            // 構造変化が 1 件でもあれば全走査 (rescan_pending が上位互換なので status_pending は
            // 見ない)。無ければ内容変更だけなので軽量な rescan_status_only で済ませる
            if self.rescan_pending {
                self.rescan();
            } else {
                self.rescan_status_only();
            }
            self.reset_rescan_debounce();
            changed = true;
        }
        changed
    }

    /// ツリーと git status をまとめて再取得する。FS 監視の間引き後 (構造変化があった時) と、
    /// 手動再走査 (r キー)・stage/unstage 実行後の両方から呼ばれる共通処理。
    fn rescan(&mut self) {
        // sync_deleted は tree.rescan (nodes を作り直す) の後に、かつ新しい git status
        // (refresh_git_status で取得済み) を使って呼ぶ必要があるため、この順序で並べる
        self.refresh_git_status();
        self.tree.rescan();
        self.tree.sync_deleted(&self.root, &self.deleted_paths());
        // ツリーに現れない (未展開の) 変更も候補一覧には効くので、次に Finder を
        // 開くときに歩き直させる。ここで走査を起こすと保存のたびに全走査になる
        self.file_index.invalidate();
        self.after_status_refresh();
    }

    /// 書き込み系操作 (stage/commit/discard/stash/branch 切替/リモート) と手動再走査 (r) の後に
    /// 呼ぶ即時再取得。FS 監視の 500ms デバウンスとは別に走らせるので、直後に自動再走査が
    /// 二重で走らないようタイマー・保留フラグもここで揃える (呼び出し側に 4 行を複製しない)
    pub(super) fn rescan_now(&mut self) {
        self.rescan();
        self.reset_rescan_debounce();
    }

    // 次の自動再走査までの間隔を測り直す。保留フラグは今の再取得で消化済みなので落とす
    fn reset_rescan_debounce(&mut self) {
        self.last_rescan = Instant::now();
        self.rescan_pending = false;
        self.status_pending = false;
    }

    /// rescan の軽量版。ファイルの中身だけが変わった FS イベントに対して使い、
    /// tree.rescan (WalkBuilder の全走査) を省略する — ツリーの行構成 (どのパスが存在するか) は
    /// 変わらない前提のため。削除・作成・リネームは常に structural = true として rescan() 側に
    /// 回るので、ここで tree.sync_deleted (削除ファイルの合成ノード追加) を呼ぶ必要もない。
    /// git status の再取得だけで GIT レーンの絞り込み (status ベース) と diff は追従する
    fn rescan_status_only(&mut self) {
        self.refresh_git_status();
        self.after_status_refresh();
    }

    fn refresh_git_status(&mut self) {
        self.git = git::file_statuses(&self.root);
        // ステータスバーの常時表示もこの 500ms デバウンスに相乗りさせる (専用タイマーは作らない)
        self.branch_status = git::branch_status(&self.root);
    }

    // rescan/rescan_status_only 共通の後処理。新しい git status を LOG からの離脱判定・GIT の
    // 絞り込み・diff 更新に反映する (走査したかどうかに関わらず同じ内容)
    fn after_status_refresh(&mut self) {
        // LOG は FS 監視の対象外 (.git は watch.rs のフィルタで除外される) なので取り直しは
        // しない。リポジトリ自体が消えた場合だけは滞在させず VIEW へ戻す
        if matches!(self.lane, Lane::Log(_)) && self.git.is_none() {
            self.lane = Lane::View;
            return;
        }
        if !matches!(self.lane, Lane::Git(_)) {
            return;
        }
        // 滞在中に変更が全部無くなった場合 (別端末での commit / stash 等)。
        // 空のツリーに取り残さず VIEW へ戻す
        if !self.git_available() {
            self.tree.set_filter(None);
            self.lane = Lane::View;
            return;
        }
        // 絞り込みも表示中 diff も新しい git status に追従させる
        self.tree.set_filter(Some(self.changed_paths()));
        let root = self.root.clone();
        let untracked = self.untracked_paths();
        if let Lane::Git(git) = &mut self.lane {
            // 背景の自動再取得 (500ms デバウンス) では打ち切りを notice に出さない
            // (毎回スパムしないため)。明示操作である A/t (on_git_key) だけが通知する
            git.refresh(&root, &untracked);
        }
    }

    // A (まとめ diff) / t (基準循環時の再取得) が使う untracked ファイル一覧。
    // status.files を毎回線形走査するだけで、頻繁な連打を想定しないため専用のキャッシュは持たない
    // (Space の STAGE_DEBOUNCE のような対策が要るキーではない)。HashMap 由来の順序は非決定的
    // なので diff の連結順が毎回揺れないようソートしておく
    fn untracked_paths(&self) -> Vec<PathBuf> {
        let Some(status) = &self.git else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = status
            .files
            .iter()
            .filter(|(_, s)| s.worktree == Some(StatusKind::Untracked))
            .map(|(p, _)| p.clone())
            .collect();
        paths.sort();
        paths
    }

    /// Shift+Tab: VIEW → EDIT → GIT → LOG → VIEW と循環する。入れないレーン
    /// (非テキストの EDIT、非 git repo の GIT/LOG) は飛ばし、一周して戻れなければ現状維持。
    pub(super) fn cycle_lane(&mut self) {
        // Issues/PR タブに Lane の概念は無いので、居る間は循環を無効にする
        // (ステータスバー側もこれに合わせてセグメントを暗くする)
        if !matches!(self.workspace, Workspace::Viewer) {
            return;
        }
        // 未保存の編集を Shift+Tab で取りこぼさない (Esc の discard 確認と同じ理由)
        if let Lane::Edit(state) = &mut self.lane
            && state.buffer.dirty()
        {
            state.notice = Some("未保存の変更があります (Ctrl+s: 保存 / Esc: 破棄)".to_string());
            return;
        }
        self.pending_g = false;
        let mut index = self.lane.index();
        for _ in 0..Lane::LABELS.len() {
            index = (index + 1) % Lane::LABELS.len();
            if self.enter_lane(index) {
                return;
            }
        }
    }

    // Lane::LABELS の index に対応するレーンへ入る。入れなければ false を返して呼び出し側で次へ送る
    fn enter_lane(&mut self, index: usize) -> bool {
        let entered = match index {
            1 => self.enter_edit(),
            2 => self.enter_git(),
            3 => self.enter_log(),
            _ => {
                self.lane = Lane::View;
                true
            }
        };
        // GIT を離れたらツリーの絞り込みを解除する。実 expanded フラグは触っていないので
        // 元の展開状態がそのまま戻る
        if entered && !matches!(self.lane, Lane::Git(_)) {
            self.tree.set_filter(None);
        }
        entered
    }

    /// EDIT レーンへ入る。非テキスト・巨大ファイル・非 UTF-8 は false (Shift+Tab では飛ばされ、
    /// e キーからは no-op になる)
    pub(super) fn enter_edit(&mut self) -> bool {
        if !self.viewer.is_text() {
            return false;
        }
        let Some(open) = &self.viewer.current else {
            return false;
        };
        let Some(state) = EditState::open(
            &open.path.clone(),
            &self.viewer.highlighter,
            self.viewer.viewport.scroll,
            &self.root,
        ) else {
            return false;
        };
        self.lane = Lane::Edit(state);
        true
    }

    /// GIT レーンに入れるか。非 git repo (git 未インストール含む) と、変更が 1 件も無いときは
    /// 入れない (空のツリーと空の diff を見せても意味がない)。ステータスバーの活性表示も
    /// 同じ判定を参照するので、見た目と実際の可否がずれない
    pub fn git_available(&self) -> bool {
        self.git
            .as_ref()
            .is_some_and(|status| !status.files.is_empty())
    }

    fn enter_git(&mut self) -> bool {
        if !self.git_available() {
            return false;
        }
        self.tree.set_filter(Some(self.changed_paths()));
        let mut git = GitState::new(self.viewer.viewport.wrap);
        if let Some(path) = self.tree.selected_or_first_file() {
            git.open(&self.root, &path);
        }
        self.lane = Lane::Git(git);
        // 入った直後は「どのファイルを見るか」の選択が主操作なのでツリー側にフォーカスを寄せる
        self.focus = Focus::Tree;
        true
    }

    /// LOG レーンに入れるか。GIT の git_available (変更が 1 件以上) と違い、コミット履歴の
    /// 閲覧は変更の有無を問わない。git repo でありさえすれば良く、コミットが 0 件でも
    /// 一覧側で「no commits」を出すだけで panic しない (LogState::new / git::log が空 Vec で吸収)
    pub fn log_available(&self) -> bool {
        self.git.is_some()
    }

    fn enter_log(&mut self) -> bool {
        if !self.log_available() {
            return false;
        }
        self.lane = Lane::Log(LogState::new(&self.root, self.viewer.viewport.wrap));
        // GIT と同じ理由 (入った直後の主操作は一覧側の選択) でツリー相当のフォーカスに寄せる
        self.focus = Focus::Tree;
        true
    }

    /// ブランチ一覧オーバーレイ (`b`) を開けるか。LOG と同じく変更の有無を問わず、
    /// git repo でありさえすればよい (一覧が空でも BranchState 側が空を前提に組んである)
    pub fn branch_available(&self) -> bool {
        self.git.is_some()
    }

    /// GitHub モードのタブバーを実際に出してよいか。有効化されていて (github_enabled) かつ
    /// 使える環境 (github_available) の両方が揃って初めて true。タブバーの描画・Ctrl+t /
    /// Alt+1..3 のキー・クリック判定が全てこの 1 箇所を参照する
    pub fn workspace_available(&self) -> bool {
        self.github_enabled && self.github_available
    }

    // gh の有無・認証・GitHub リモートかどうかを判定する。1 度確定したら github_checked で
    // 以後は呼び出しても何もしない (起動時 1 回・初回有効化時 1 回だけに絞るため)
    fn ensure_github_checked(&mut self) {
        if self.github_checked {
            return;
        }
        self.github_checked = true;
        match github::check_available(&self.root) {
            Ok(()) => self.github_available = true,
            Err(reason) => {
                self.github_available = false;
                self.set_notice(reason, true);
            }
        }
    }

    /// 設定オーバーレイの "github tabs" トグル。cli 由来の有効化と違い、ここでの変更は
    /// そのまま config に永続化する (persisted と実行時フラグを同じ値に揃えることで、
    /// 一度トグルすれば --github の有無に関わらずその通りの状態になる)
    pub(super) fn toggle_github(&mut self) {
        self.github_enabled = !self.github_enabled;
        self.github_persisted = self.github_enabled;
        self.persist_config();
        if self.github_enabled {
            self.ensure_github_checked();
        }
        if !self.workspace_available() {
            self.workspace = Workspace::Viewer;
        }
    }

    /// Ctrl+t: 次のタブへ循環する。使えない (workspace_available が false) 間は無効化時と
    /// 同じ経路を通るだけの no-op になる
    pub(super) fn cycle_workspace(&mut self) {
        if !self.workspace_available() {
            return;
        }
        self.workspace = Workspace::from_index((self.workspace.index() + 1) % 3);
        self.after_workspace_change();
    }

    /// Alt+1..3・タブクリック共通の直接指定。使えない間は no-op
    pub(super) fn set_workspace(&mut self, workspace: Workspace) {
        if !self.workspace_available() {
            return;
        }
        self.workspace = workspace;
        self.after_workspace_change();
    }

    // cycle_workspace/set_workspace 共通の遷移後処理。同じガード (issues/PR が初回取得済みか) を
    // 呼び出し側に重複させないための唯一の入口 (keys.rs の関数コピーを避ける方針と同じ理由)
    fn after_workspace_change(&mut self) {
        // 入った直後の主操作は一覧側の選択なので、GIT/LOG と同じくツリー相当にフォーカスを寄せる
        match self.workspace {
            Workspace::Issues => {
                self.focus = Focus::Tree;
                // 初回タブ表示時に 1 回だけ取得する。タブを往復しても再取得しない (issue #33 の要求)
                if !self.issues.fetched() && !self.issues.list_loading() {
                    self.refresh_issues();
                }
            }
            Workspace::PullRequests => {
                self.focus = Focus::Tree;
                if !self.prs.fetched() && !self.prs.list_loading() {
                    self.refresh_prs();
                }
            }
            Workspace::Viewer => {}
        }
    }

    /// ツリー・ファインダーからの「開く」の振り分け。VIEW/EDIT は viewer、GIT は diff を差し替える
    pub(super) fn open_selected(&mut self, path: &Path) {
        match &mut self.lane {
            Lane::Git(git) => {
                // ツリーでファイルを選び直したら「全ファイルまとめ」表示 (#31) は解除する
                git.exit_all();
                git.open(&self.root, path);
            }
            _ => self.viewer.open(path, &self.root),
        }
    }

    // 変更ファイルとその祖先ディレクトリ。どちらも git status 取得時に組んだ集合を使い回すだけで、
    // ツリーの再走査は要らない
    fn changed_paths(&self) -> HashSet<PathBuf> {
        match &self.git {
            Some(status) => status
                .files
                .keys()
                .cloned()
                .chain(status.changed_dirs.iter().cloned())
                .collect(),
            None => HashSet::new(),
        }
    }

    // Tree::sync_deleted へ渡す削除パス集合。App::new (self 未構築) でも使えるよう
    // 本体は自由関数 (deleted_paths_of) に持たせ、ここは self.git を渡すだけの薄いラッパー
    fn deleted_paths(&self) -> HashSet<PathBuf> {
        deleted_paths_of(&self.git)
    }

    /// bracketed paste (main のイベントループから)。編集バッファへは複数行のまま、
    /// Search/Goto/Finder の 1 行入力へは制御文字を落として流す
    /// (paste 有効化前は生キー入力として届いていた挙動の維持)
    pub fn on_paste(&mut self, text: &str) {
        if let Lane::Edit(state) = &mut self.lane {
            state.paste(text, &self.viewer.highlighter, &mut self.viewer.viewport);
            return;
        }
        if let Mode::Commit { .. } = &self.mode {
            self.commit_paste(text);
            return;
        }
        match &mut self.mode {
            Mode::Input { kind, buffer } => {
                let kind = *kind;
                for c in text.chars().filter(|c| !c.is_control()) {
                    // キー入力側 (on_input_key) と同じ Goto の数字ガードを通す
                    if kind == InputKind::Goto && !c.is_ascii_digit() {
                        continue;
                    }
                    buffer.push(c);
                }
                self.live_update_input(kind);
            }
            Mode::Finder(finder) => {
                for c in text.chars().filter(|c| !c.is_control()) {
                    finder.push_char(c);
                }
            }
            _ => {}
        }
    }

    pub fn toggle_hidden(&mut self) {
        let show_hidden = self.tree.toggle_hidden();
        self.file_index.set_show_hidden(show_hidden);
        // toggle_hidden 内部の rescan で nodes が作り直されるため、削除ファイルの合成ノードも
        // 都度足し直さないと隠れてしまう (git status 自体は変わらないので既存 self.git を使う)
        self.tree.sync_deleted(&self.root, &self.deleted_paths());
        // 既存 watcher のキューには切替前のフィルタ結果が残るため、監視も作り直して揃える。
        self.watcher = FsWatcher::new(&self.root, show_hidden);
        self.reset_rescan_debounce();
        self.persist_config();
    }

    pub fn toggle_icons(&mut self) {
        self.icons = !self.icons;
        self.persist_config();
    }

    pub fn toggle_wrap(&mut self) {
        self.viewer.viewport.toggle_wrap();
        self.persist_config();
    }

    /// delta の符号方向に THEME_NAMES を巡回する (設定画面の h/l 用)
    pub fn cycle_theme(&mut self, delta: isize) {
        let names = viewer::THEME_NAMES;
        let current = self.viewer.theme_name();
        let idx = names.iter().position(|n| *n == current).unwrap_or(0) as isize;
        let len = names.len() as isize;
        let next = (idx + delta).rem_euclid(len) as usize;
        self.viewer.set_theme(names[next]);
        self.persist_config();
    }

    /// 全レーン共通の一時通知をセットする。書き込み系操作 (run_git_write) の結果表示など、
    /// GIT レーンを離れても見せたいメッセージから呼ぶ
    pub(super) fn set_notice(&mut self, message: impl Into<String>, is_error: bool) {
        self.notice = Some((message.into(), Instant::now(), is_error));
    }

    /// ステータスバー表示用。実行中のリモート操作が無ければ None
    pub fn running_remote_job(&self) -> Option<&'static str> {
        self.pending_remote_job.as_ref().map(|p| p.kind.label())
    }

    /// f/p/P 共通のジョブ起動。実行中は新しいジョブを一切受け付けない (多重起動防止) ことと、
    /// 完了メッセージ用のスナップショット保存をここに集約し、app/git_ops.rs 側の各キー処理で
    /// 同じガードを重複させない
    pub(super) fn start_remote_job<F>(&mut self, kind: git::RemoteJobKind, work: F)
    where
        F: FnOnce() -> git::GitOutcome + Send + 'static,
    {
        if self.pending_remote_job.is_some() {
            return;
        }
        let status = self.branch_status.as_ref();
        self.pending_remote_job = Some(PendingRemoteJob {
            kind,
            branch: status.map(|s| s.name.clone()).unwrap_or_default(),
            ahead: status.map(|s| s.ahead).unwrap_or(0),
            behind: status.map(|s| s.behind).unwrap_or(0),
            had_upstream: status.is_some_and(|s| s.has_upstream),
        });
        self.remote_job_rx = Some(job::spawn(work));
    }

    // 完了後の反映。成功なら他の書き込み系操作と同じ rescan (500ms デバウンスとは別に即時実行) に
    // 相乗りさせて status/ahead-behind/表示中 diff をまとめて取り直してから要約を notice に出す
    fn finish_remote_job(&mut self, outcome: git::GitOutcome) {
        let Some(pending) = self.pending_remote_job.take() else {
            return;
        };
        if outcome.ok {
            self.rescan_now();
            self.set_notice(summarize_remote_job(&pending, &outcome), false);
        } else {
            let message = if outcome.message.is_empty() {
                format!("{} に失敗しました", pending.kind.label())
            } else {
                outcome.message
            };
            self.set_notice(message, true);
        }
    }

    fn current_config(&self) -> Config {
        Config {
            show_hidden: self.tree.show_hidden(),
            icons: self.icons,
            wrap_default: self.viewer.viewport.wrap,
            theme: self.viewer.theme_name().to_string(),
            split_ratio: self.split_ratio,
            // github_enabled ではなく github_persisted を使う。cli 由来の一時的な有効化が
            // 他の設定操作 (ペイン幅ドラッグ等) の persist_config に巻き込まれて書き込まれるのを防ぐ
            github: self.github_persisted,
        }
    }

    // 保存失敗 (権限なし等) はここで握り潰す。読み取り専用ビューアの付随機能が
    // ファイル書き込み失敗でクラッシュ・エラー表示をする理由はない
    fn persist_config(&self) {
        let _ = self.current_config().save();
    }
}

// git status で削除 (index/worktree いずれか) 扱いのパス集合。App::new は self 構築前に
// これが要るため、App::deleted_paths から使い回せるよう自由関数にしてある
fn deleted_paths_of(git: &Option<GitStatus>) -> HashSet<PathBuf> {
    match git {
        Some(status) => status
            .files
            .iter()
            .filter(|(_, s)| {
                s.index == Some(StatusKind::Deleted) || s.worktree == Some(StatusKind::Deleted)
            })
            .map(|(p, _)| p.clone())
            .collect(),
        None => HashSet::new(),
    }
}

// リモート操作の完了メッセージ組み立て。pending は実行開始時点の ahead/behind、outcome は
// git の実行結果。issue の例 (「push → origin/main (2 commits)」) に合わせた形式にする。
// ahead/behind が 0 のときは「N commits」ではなく「up to date」にして違和感を減らす
fn summarize_remote_job(pending: &PendingRemoteJob, outcome: &git::GitOutcome) -> String {
    match pending.kind {
        git::RemoteJobKind::Fetch => {
            if outcome.message.is_empty() {
                "fetch 完了".to_string()
            } else {
                format!("fetch 完了: {}", outcome.message)
            }
        }
        git::RemoteJobKind::Pull => {
            if pending.behind == 0 {
                format!("pull → {} (up to date)", pending.branch)
            } else {
                format!("pull → {} ({} commits)", pending.branch, pending.behind)
            }
        }
        git::RemoteJobKind::Push => {
            let target = format!("origin/{}", pending.branch);
            // upstream が無かった push は ahead が常に 0 (branch_status が算出できないため) で
            // コミット数として意味を持たないので、「新規ブランチ」だと分かる表記にする
            if !pending.had_upstream {
                format!("push → {target} (new branch)")
            } else if pending.ahead == 0 {
                format!("push → {target} (up to date)")
            } else {
                format!("push → {target} ({} commits)", pending.ahead)
            }
        }
    }
}

// Finder の候補は相対パス文字列。FileIndex とツリー (走査完了前の代用) の
// どちらから来ても同じ形に揃える
pub(super) fn to_candidates(files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

// 最小幅を満たせない極端に狭い端末では下限を諦めて半分ずつにする
// (clamp の lo > hi でパニックさせない)
fn clamp_tree_width(width: u16, total: u16) -> u16 {
    if total < MIN_TREE_WIDTH + MIN_VIEWER_WIDTH {
        return total / 2;
    }
    width.clamp(MIN_TREE_WIDTH, total - MIN_VIEWER_WIDTH)
}
