use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Focus, Lane, Workspace};
use crate::lang::t;
use crate::tr;
use crate::widget::centered_rect;

/// キーバインド一覧のオーバーレイ。実装済みのハンドラ (app/keys.rs の on_*_key) と
/// 一対一で対応させる。ここに書いた内容と実際の挙動がずれないよう追加時は両方直す。
///
/// 全レーン分を並べると 200 行近くになり端末の高さに収まらないので、他のペインと同じく
/// **自前でスライスして**描く (`Paragraph::scroll` は使わない)。以前は全行を渡すだけで
/// スクロールも打ち切りの表示も無く、Git/Log panel/Edit 以降のセクションが黙って切れていた。
///
/// さらに、**今開いている画面の節を先頭へ持ち上げる** (`current_screen` / `hoisted`)。
/// 節の並びが固定だと「GIT を見ているのに Git の節は 5 画面下」のようになり、スクロール
/// できるようにしただけでは「今押せるキー」に辿り着けないため。並び替えるのは順序だけで、
/// 節の中身も全節を載せることも変えない (探しに行けば必ずそこに在る、を壊さない)。
///
/// 戻り値は (1 画面に出せる行数, 総行数)。呼び出し側が App へ書き戻し、on_help_key が
/// クランプとページ送り量に使う (viewport.height と同じ 描画→app のパターン)
pub(super) fn draw_help(frame: &mut Frame, app: &App, scroll: usize, area: Rect) -> (usize, usize) {
    let popup = centered_rect(70, 80, area);
    frame.render_widget(Clear, popup);

    let lines = help_lines(current_screen(app));
    let height = popup.height.saturating_sub(2) as usize;
    let total = lines.len();
    // 末尾まで送り切ったら最後の 1 画面で止める (最終行より先へは進めない)
    let scroll = scroll.min(total.saturating_sub(height));

    // 「まだ先がある」ことを枠に出す。出さないと、切れているのか終わりなのかが区別できない
    let title = if total > height {
        tr!(
            " help  {}-{}/{}  j/k: スクロール  ?: 閉じる ",
            " help  {}-{}/{}  j/k: scroll  ?: close ",
            (scroll + 1).min(total),
            (scroll + height).min(total),
            total
        )
    } else {
        " help ".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(title);

    let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();
    let paragraph = Paragraph::new(visible).block(block);
    frame.render_widget(paragraph, popup);
    (height, total)
}

/// 節の識別子。並び替え (どれを先頭へ持ち上げるか) にしか使わないので、
/// 節が分かれている粒度と 1:1 で足りる
#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionId {
    Global,
    Help,
    Workspace,
    Issues,
    Prs,
    Tree,
    Viewer,
    Git,
    LogPanel,
    Edit,
    Mouse,
    Confirm,
    Commit,
    Branch,
    Remote,
    Finder,
    Grep,
    Input,
}

struct Section {
    id: SectionId,
    title: &'static str,
    // 文言は `t()` の対で持つので、選ばれた側だけが並んだ列になる。static のままだと
    // 対の片側を選ぶ場所を別に用意することになり、キー列に日本語が混ざる項目
    // (「ステータスバー」等) を同じ形で対にできない
    entries: Vec<(&'static str, &'static str)>,
}

/// ヘルプの並び替えのためだけの「今どの画面を見ているか」。Workspace/Lane/Focus の
/// 組み合わせのうち、節が分かれている粒度だけを潰して持つ
#[derive(Clone, Copy, PartialEq, Eq)]
enum Screen {
    Issues,
    PullRequests,
    Edit,
    Git,
    Log,
    Tree,
    Viewer,
}

fn current_screen(app: &App) -> Screen {
    match app.workspace {
        Workspace::Issues => Screen::Issues,
        Workspace::PullRequests => Screen::PullRequests,
        Workspace::Viewer => match &app.lane {
            Lane::Edit(_) => Screen::Edit,
            // GIT はツリー側も diff 側も 1 つの節にまとめてあるので focus で分けない
            Lane::Git(_) => Screen::Git,
            // コミット一覧はレーンではなくパネルなので、「今そこを見ている」は
            // フォーカス (一覧側) と右ペインがコミット diff かどうかで決まる
            Lane::View if app.focus == Focus::Log || app.showing_commit_diff() => Screen::Log,
            Lane::View if app.focus == Focus::Tree => Screen::Tree,
            Lane::View => Screen::Viewer,
        },
    }
}

/// 先頭へ持ち上げる節。1 つ目がその画面そのもの、2 つ目以降は同じ画面で一緒に使う節
/// (ツリーと右ペインのように、フォーカスを移せばすぐ効くもの) を添える
fn hoisted(screen: Screen) -> &'static [SectionId] {
    match screen {
        Screen::Issues => &[SectionId::Issues, SectionId::Workspace],
        Screen::PullRequests => &[SectionId::Prs, SectionId::Workspace],
        Screen::Edit => &[SectionId::Edit],
        Screen::Git => &[SectionId::Git],
        Screen::Log => &[SectionId::LogPanel, SectionId::Viewer],
        Screen::Tree => &[SectionId::Tree, SectionId::Viewer],
        Screen::Viewer => &[SectionId::Viewer, SectionId::Tree],
    }
}

fn help_lines(screen: Screen) -> Vec<Line<'static>> {
    let sections = sections();
    let first = hoisted(screen);
    // 今の画面の節 (対で使うペインも含めて連続させる) → ヘルプ自身の操作 → 残りは定義順。
    // 持ち上げた節を割らないのは、Tree と Viewer のように同時に画面へ出ているものを
    // 「今の画面のキー」として 1 かたまりで読ませるため。ヘルプ自身の操作はその直後に置くが、
    // 今の画面の節が長ければ (Git は単独で 30 行超) 1 画面目からは押し出される —
    // スクロールと閉じ方は枠のタイトルとステータスバーに常時出ているので、
    // この節が 1 画面目に入ることには依存しない
    let order = first
        .iter()
        .copied()
        .chain(std::iter::once(SectionId::Help))
        .chain(
            sections
                .iter()
                .map(|s| s.id)
                .filter(|id| *id != SectionId::Help && !first.contains(id)),
        );

    let mut lines: Vec<Line> = Vec::new();
    for id in order {
        let section = sections
            .iter()
            .find(|s| s.id == id)
            .expect("hoisted した節は sections() に必ず在る");
        // 並びが画面によって変わるので、なぜ先頭に来ているのかを見出しに書いておく
        let title = if first.contains(&id) {
            tr!("{} ← 今の画面", "{} ← current screen", section.title)
        } else {
            section.title.to_string()
        };
        push_help_section(&mut lines, &title, &section.entries);
    }
    lines
}

fn sections() -> Vec<Section> {
    vec![
        Section {
            id: SectionId::Global,
            title: "Global",
            entries: vec![
                ("Ctrl+c", t("終了", "quit")),
                ("q", t("終了", "quit")),
                (
                    "Shift+Tab",
                    t(
                        "モード切替 (VIEW → EDIT → GIT)",
                        "switch lane (VIEW → EDIT → GIT)",
                    ),
                ),
                (
                    "Tab",
                    t(
                        "フォーカス切替 (Tree → Viewer。コミット一覧を出している間だけ Log を挟む)",
                        "switch focus (Tree → Viewer; Log is inserted only while the commit list is shown)",
                    ),
                ),
                (
                    "L",
                    t(
                        "コミット一覧パネルの表示切替 (VIEW のみ・左ペイン下半分)",
                        "toggle the commit list panel (VIEW only, lower half of the left pane)",
                    ),
                ),
                ("Ctrl+p", t("ファインダーを開く", "open the finder")),
                (
                    "Ctrl+f",
                    t("ワークスペース横断検索を開く", "open workspace-wide search"),
                ),
                (
                    "b",
                    t(
                        "ブランチ一覧オーバーレイを開く (git repo でのみ)",
                        "open the branch list overlay (git repos only)",
                    ),
                ),
                ("?", t("このヘルプを開く", "open this help")),
                ("s", t("設定画面を開く", "open settings")),
                ("a", t("隠し項目の表示を切替", "toggle hidden items")),
                (
                    "i",
                    t(
                        "無視ファイル (.gitignore/.ignore/exclude) の表示を切替",
                        "toggle ignored files (.gitignore/.ignore/exclude)",
                    ),
                ),
                (
                    "-a, --hidden",
                    t("起動時に隠し項目を表示", "show hidden items on startup"),
                ),
                (
                    "-i, --ignored",
                    t(
                        "起動時に無視ファイルも表示",
                        "show ignored files on startup",
                    ),
                ),
                (
                    t("ステータスバー", "status bar"),
                    t(
                        "現在ブランチ + ahead/behind を常時表示 (git repo のみ)",
                        "always shows the current branch + ahead/behind (git repos only)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Help,
            title: "Help (?)",
            entries: vec![
                ("j/k ↑/↓", t("1 行スクロール", "scroll one line")),
                ("Ctrl+d/u", t("半ページスクロール", "scroll half a page")),
                ("gg / G", t("先頭 / 末尾へ", "go to top / bottom")),
                ("? / Esc / q", t("閉じる", "close")),
                (
                    t("節の並び", "section order"),
                    t(
                        "今開いている画面の節が先頭に来る (残りは定義順)",
                        "the section for the current screen comes first (the rest keep their defined order)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Workspace,
            title: t(
                "Workspace (GitHub モード、既定は無効)",
                "Workspace (GitHub mode, off by default)",
            ),
            entries: vec![
                (
                    "Ctrl+t",
                    t(
                        "次のタブへ (viewer → issues → pull requests)",
                        "go to the next tab (viewer → issues → pull requests)",
                    ),
                ),
                (
                    "Alt+1/2/3",
                    t(
                        "viewer / issues / pull requests へ直接切替",
                        "jump straight to viewer / issues / pull requests",
                    ),
                ),
                (
                    t("タブをクリック", "click a tab"),
                    t("そのタブへ切替", "switch to that tab"),
                ),
                (
                    "--github",
                    t(
                        "起動時だけ有効化 (config には保存しない)",
                        "enable for this run only (not saved to the config)",
                    ),
                ),
                (
                    t("設定画面の github tabs", "github tabs in settings"),
                    t(
                        "トグルで有効化・config に永続化",
                        "toggle to enable and persist it to the config",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Issues,
            title: t(
                "Issues (Ctrl+t / Alt+2、GitHub モード有効時)",
                "Issues (Ctrl+t / Alt+2, when GitHub mode is on)",
            ),
            entries: vec![
                ("j/k ↑/↓ gg/G", t("一覧を移動", "move through the list")),
                (
                    "Ctrl+d/u",
                    t(
                        "一覧を半ページ移動 / 詳細を半ページスクロール",
                        "move half a page in the list / scroll the detail half a page",
                    ),
                ),
                (
                    "Tab",
                    t(
                        "一覧 ⇄ 詳細のフォーカス切替",
                        "switch focus between list and detail",
                    ),
                ),
                (
                    t("Enter / l / クリック", "Enter / l / click"),
                    t(
                        "選択 issue の詳細を右に読み込む",
                        "load the selected issue's detail on the right",
                    ),
                ),
                (
                    "o",
                    t(
                        "ブラウザで開く (gh issue view --web)",
                        "open in the browser (gh issue view --web)",
                    ),
                ),
                (
                    "r",
                    t(
                        "一覧を再取得 (タブ往復では自動取得しない)",
                        "refetch the list (switching tabs does not refetch)",
                    ),
                ),
                (
                    "/",
                    t(
                        "一覧をファジー絞り込み (一覧側フォーカス時のみ)",
                        "fuzzy filter the list (only while the list has focus)",
                    ),
                ),
                (
                    "t",
                    t(
                        "state 絞り込みを循環 (open → closed → all)",
                        "cycle the state filter (open → closed → all)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Prs,
            title: t(
                "Pull Requests (Ctrl+t / Alt+3、GitHub モード有効時)",
                "Pull Requests (Ctrl+t / Alt+3, when GitHub mode is on)",
            ),
            entries: vec![
                ("j/k ↑/↓ gg/G", t("一覧を移動", "move through the list")),
                (
                    "Ctrl+d/u",
                    t(
                        "一覧を半ページ移動 / 右ペインを半ページスクロール",
                        "move half a page in the list / scroll the right pane half a page",
                    ),
                ),
                (
                    "Tab",
                    t(
                        "一覧 ⇄ 詳細のフォーカス切替",
                        "switch focus between list and detail",
                    ),
                ),
                (
                    t("Enter / l / クリック", "Enter / l / click"),
                    t(
                        "選択 PR を説明表示で開く",
                        "open the selected PR in the description view",
                    ),
                ),
                (
                    "d",
                    t(
                        "差分を表示 (GIT/LOG レーンと同じ見え方)",
                        "show the diff (rendered like the GIT/LOG lanes)",
                    ),
                ),
                (
                    "S",
                    t(
                        "CI ステータスを表示 (s は設定に割り当て済みのため大文字)",
                        "show CI status (uppercase because s is taken by settings)",
                    ),
                ),
                (
                    t("j/k ↑/↓ (diff 表示中)", "j/k ↑/↓ (diff)"),
                    t("行カーソルを移動", "move the line cursor"),
                ),
                (
                    t("]/[ (diff 表示中)", "]/[ (diff)"),
                    t("次 / 前の hunk へ", "go to the next / previous hunk"),
                ),
                (
                    t("w (diff 表示中)", "w (diff)"),
                    t(
                        "折り返し切替 (設定には保存しない)",
                        "toggle wrap (not saved to the config)",
                    ),
                ),
                (
                    t("h/l ←/→ (diff 表示中)", "h/l ←/→ (diff)"),
                    t("水平スクロール", "scroll horizontally"),
                ),
                (
                    "o",
                    t(
                        "ブラウザで開く (gh pr view --web)",
                        "open in the browser (gh pr view --web)",
                    ),
                ),
                (
                    "r",
                    t(
                        "一覧を再取得 (タブ往復では自動取得しない)",
                        "refetch the list (switching tabs does not refetch)",
                    ),
                ),
                (
                    "/",
                    t(
                        "一覧をファジー絞り込み (一覧側フォーカス時のみ)",
                        "fuzzy filter the list (only while the list has focus)",
                    ),
                ),
                (
                    "t",
                    t(
                        "state 絞り込みを循環 (open → closed → merged → all)",
                        "cycle the state filter (open → closed → merged → all)",
                    ),
                ),
                (
                    t("巨大な diff", "huge diffs"),
                    t(
                        "行数/バイト数の上限で打ち切り、notice で通知",
                        "truncated at the line/byte limit, reported with a notice",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Tree,
            title: "Tree",
            entries: vec![
                ("j/k ↑/↓", t("上下移動", "move up / down")),
                ("l →", t("展開 / 開く", "expand / open")),
                ("h ←", t("折りたたみ / 親へ", "collapse / go to parent")),
                (
                    "H",
                    t(
                        "親を選択して折りたたむ",
                        "select the parent and collapse it",
                    ),
                ),
                ("Enter", t("開く / 展開切替", "open / toggle expansion")),
                ("gg / G", t("先頭 / 末尾へ", "go to top / bottom")),
                ("r", t("再走査", "rescan")),
                (
                    "n",
                    t(
                        "新規ファイル (選択行のディレクトリ配下。a/b.rs で途中のディレクトリも作る)",
                        "new file (under the selected directory; a/b.rs creates intermediate dirs)",
                    ),
                ),
                ("N", t("新規ディレクトリ", "new directory")),
                (
                    "R",
                    t("リネーム (親は据え置き)", "rename (parent stays the same)"),
                ),
                (
                    "D",
                    t(
                        "削除 (確認あり。ディレクトリは配下ごと)",
                        "delete (with confirmation; a directory goes with its contents)",
                    ),
                ),
                (
                    "y",
                    t(
                        "相対パスをクリップボードへ",
                        "copy the relative path to the clipboard",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Viewer,
            title: "Viewer",
            entries: vec![
                (
                    "j/k ↑/↓",
                    t(
                        "行カーソルを移動 (画面はカーソルに追従する。v/e の起点もこの行)",
                        "move the line cursor (the view follows it; v/e start from this line)",
                    ),
                ),
                ("Ctrl+d/u", t("半ページスクロール", "scroll half a page")),
                ("gg / G", t("先頭 / 末尾へ", "go to top / bottom")),
                ("w", t("折り返し切替", "toggle wrap")),
                ("h/l ←/→", t("水平スクロール", "scroll horizontally")),
                (
                    "0",
                    t("水平スクロールをリセット", "reset horizontal scroll"),
                ),
                (
                    "Ctrl+o",
                    t(
                        "履歴を戻る (Backspace も同様)",
                        "go back in history (Backspace does the same)",
                    ),
                ),
                ("Ctrl+i", t("履歴を進む", "go forward in history")),
                (":N Enter", t("N 行目へジャンプ", "jump to line N")),
                ("/", t("検索", "search")),
                (
                    "n / N",
                    t("次 / 前のマッチへ", "go to the next / previous match"),
                ),
                ("e", t("編集モードに入る", "enter edit mode")),
                (
                    t("マウス長押し + 移動", "mouse drag"),
                    t(
                        "文字単位の範囲選択 (端まで引っ張ると 1 行ずつ送る)",
                        "character-wise selection (dragging past an edge scrolls a line at a time)",
                    ),
                ),
                (
                    "v",
                    t(
                        "行単位の選択を開始 / 解除 (j/k・Ctrl+d/u・gg/G で伸縮)",
                        "start / cancel a line-wise selection (grow it with j/k, Ctrl+d/u, gg/G)",
                    ),
                ),
                (
                    "y",
                    t(
                        "選択範囲をクリップボードへコピー",
                        "copy the selection to the clipboard",
                    ),
                ),
                (
                    "Y",
                    t("開いているファイル全体をコピー", "copy the whole open file"),
                ),
                ("Esc", t("選択を解除", "clear the selection")),
                (
                    t("コピー手段", "copy backend"),
                    t(
                        "pbcopy/wl-copy/xclip/xsel/clip.exe → 無ければ OSC 52 (ssh 越しも可)",
                        "pbcopy/wl-copy/xclip/xsel/clip.exe, falling back to OSC 52 (works over ssh)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Git,
            title: "Git (Shift+Tab)",
            entries: vec![
                (
                    t("左ペイン", "left pane"),
                    t(
                        "変更ファイルのみを階層付きで表示 (入った時点で全展開)",
                        "shows only changed files with their hierarchy (fully expanded on entry)",
                    ),
                ),
                (
                    "j/k ↑/↓",
                    t("変更ファイル間を移動", "move between changed files"),
                ),
                ("l →", t("展開 / diff を表示", "expand / show the diff")),
                ("h ←", t("折りたたみ / 親へ", "collapse / go to parent")),
                (
                    "H",
                    t(
                        "親を選択して折りたたむ",
                        "select the parent and collapse it",
                    ),
                ),
                (
                    "Enter",
                    t("diff を表示 / 展開切替", "show the diff / toggle expansion"),
                ),
                (
                    t("Space (左ペイン)", "Space (left pane)"),
                    t(
                        "選択中のファイル/ディレクトリを stage/unstage トグル",
                        "stage/unstage the selected file or directory",
                    ),
                ),
                (
                    t("j/k ↑/↓ (diff ペイン)", "j/k ↑/↓ (diff pane)"),
                    t(
                        "行カーソルを移動 (Space/Enter の対象は常にこのカーソル行)",
                        "move the line cursor (Space/Enter always act on this line)",
                    ),
                ),
                (
                    t("Space (diff ペイン)", "Space (diff pane)"),
                    t(
                        "カーソル行が属する hunk を stage (基準が staged のときは unstage)",
                        "stage the hunk the cursor is in (unstage when the base is staged)",
                    ),
                ),
                (
                    t("Enter (diff ペイン)", "Enter (diff pane)"),
                    t(
                        "カーソル行 (V の選択中はその範囲) の変更行だけを stage/unstage",
                        "stage/unstage only the changed lines under the cursor (or the V selection)",
                    ),
                ),
                (
                    t("V (diff ペイン)", "V (diff pane)"),
                    t(
                        "行単位選択の開始/解除 (j/k で伸縮・Esc で解除)",
                        "start / cancel a line-wise selection (grow it with j/k, cancel with Esc)",
                    ),
                ),
                (
                    t("クリック (diff ペイン)", "click (diff pane)"),
                    t("その行へカーソルを移動", "move the cursor to that line"),
                ),
                (
                    "X",
                    t(
                        "選択中のファイル/ディレクトリの変更を破棄 (確認あり・untracked は削除)",
                        "discard changes in the selected file or directory (confirmed; untracked files are deleted)",
                    ),
                ),
                (
                    "z",
                    t(
                        "変更を stash へ退避 (確認あり・untracked も含む)",
                        "stash the changes (confirmed; untracked files included)",
                    ),
                ),
                (
                    "Z",
                    t(
                        "直近の stash を pop (確認あり・GIT レーン以外からも可)",
                        "pop the latest stash (confirmed; works outside the GIT lane too)",
                    ),
                ),
                (
                    "/",
                    t(
                        "diff 内検索 (side-by-side 表示中は無効)",
                        "search within the diff (disabled in side-by-side view)",
                    ),
                ),
                (
                    "n / N",
                    t(
                        "次 / 前の検索マッチへ (検索確定後)",
                        "go to the next / previous match (after a search is committed)",
                    ),
                ),
                (
                    "] / [",
                    t("次 / 前の hunk へ", "go to the next / previous hunk"),
                ),
                (
                    "A",
                    t(
                        "全変更ファイルをまとめた diff を表示 (トグル)",
                        "show a combined diff of all changed files (toggle)",
                    ),
                ),
                (
                    "} / {",
                    t(
                        "まとめ diff 内で次 / 前のファイルへ",
                        "go to the next / previous file in the combined diff",
                    ),
                ),
                (
                    "t",
                    t(
                        "diff 基準を切替 (HEAD → staged → unstaged)",
                        "switch the diff base (HEAD → staged → unstaged)",
                    ),
                ),
                (
                    "c",
                    t(
                        "コミット (staged が空だと開かない)",
                        "commit (does not open when nothing is staged)",
                    ),
                ),
                (
                    "C",
                    t(
                        "amend コミット (既存メッセージをプリフィル・確認あり)",
                        "amend the last commit (prefills the existing message, confirmed)",
                    ),
                ),
                (
                    "v",
                    t(
                        "inline ⇔ side-by-side 切替 (設定には保存しない・まとめ diff 表示中は無効)",
                        "switch inline ⇔ side-by-side (not saved to the config; disabled in the combined diff)",
                    ),
                ),
                ("Ctrl+d/u", t("半ページスクロール", "scroll half a page")),
                ("gg / G", t("先頭 / 末尾へ", "go to top / bottom")),
                (
                    "w",
                    t(
                        "折り返し切替 (diff のみ・設定には保存しない)",
                        "toggle wrap (diff only, not saved to the config)",
                    ),
                ),
                (
                    "h/l ←/→",
                    t(
                        "水平スクロール (diff ペイン)",
                        "scroll horizontally (diff pane)",
                    ),
                ),
                (
                    "0",
                    t(
                        "水平スクロールをリセット (diff ペイン)",
                        "reset horizontal scroll (diff pane)",
                    ),
                ),
                (
                    "f / p / P",
                    t(
                        "fetch / pull / push (下の Remote セクション参照)",
                        "fetch / pull / push (see the Remote section below)",
                    ),
                ),
                (
                    "r",
                    t(
                        "再走査 (git status も取り直す)",
                        "rescan (also refetches git status)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::LogPanel,
            title: "Log panel (L)",
            entries: vec![
                (
                    "L",
                    t(
                        "表示切替 (VIEW の左ペイン下半分。ツリーと同時に見える)",
                        "toggle the panel (lower half of VIEW's left pane, shown alongside the tree)",
                    ),
                ),
                (
                    t("一覧の行", "list rows"),
                    t(
                        "コミット一覧 (短縮 SHA / 相対日時 / 作者 / 件名。狭い幅では右の列から落とす)",
                        "commit list (short SHA / relative date / author / subject; narrow widths drop the right columns first)",
                    ),
                ),
                (
                    "j/k ↑/↓",
                    t(
                        "コミット間を移動 (diff は追従しない)",
                        "move between commits (the diff does not follow)",
                    ),
                ),
                (
                    "Enter / l →",
                    t(
                        "選択コミットの diff を右ペインに表示 (フォーカスも移る)",
                        "show the selected commit's diff in the right pane (focus moves too)",
                    ),
                ),
                (
                    "gg / G",
                    t(
                        "先頭 / 読み込み済み末尾へ (末尾で追加取得)",
                        "go to the top / end of what is loaded (fetches more at the end)",
                    ),
                ),
                (
                    t("Esc (一覧)", "Esc (list)"),
                    t("パネルを閉じる (L と同じ)", "close the panel (same as L)"),
                ),
                (
                    t("j/k ↑/↓ (diff)", "j/k ↑/↓ (diff)"),
                    t("行カーソルを移動", "move the line cursor"),
                ),
                (
                    "n / N",
                    t(
                        "次 / 前の hunk へ (] / [ も同様)",
                        "go to the next / previous hunk (] / [ do the same)",
                    ),
                ),
                ("Ctrl+d/u", t("半ページスクロール", "scroll half a page")),
                (
                    "w",
                    t(
                        "折り返し切替 (diff のみ・設定には保存しない)",
                        "toggle wrap (diff only, not saved to the config)",
                    ),
                ),
                (
                    "h/l ←/→",
                    t(
                        "水平スクロール (diff ペイン)",
                        "scroll horizontally (diff pane)",
                    ),
                ),
                (
                    "0",
                    t(
                        "水平スクロールをリセット (diff ペイン)",
                        "reset horizontal scroll (diff pane)",
                    ),
                ),
                (
                    t("Esc (diff)", "Esc (diff)"),
                    t(
                        "diff を閉じてファイル表示へ戻す",
                        "close the diff and go back to the file view",
                    ),
                ),
                (
                    t("マージコミット", "merge commits"),
                    t(
                        "最初の親との diff を表示 (git show の既定は差分なし)",
                        "shows the diff against the first parent (git show shows none by default)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Edit,
            title: "Edit (e / Shift+Tab)",
            entries: vec![
                (
                    t("文字入力", "typing"),
                    t(
                        "挿入 (クリックでカーソル移動)",
                        "insert text (click to move the cursor)",
                    ),
                ),
                ("↑/↓/←/→", t("カーソル移動", "move the cursor")),
                (
                    "Alt+←/→",
                    t(
                        "単語単位で移動 (Option+←/→ / Ctrl+←/→ / Alt+b・f も可)",
                        "move by word (Option+←/→, Ctrl+←/→, Alt+b/f also work)",
                    ),
                ),
                (
                    "Home/End",
                    t(
                        "行頭 (インデント直後 ⇄ 桁 0) / 行末へ (Cmd+←/→・Ctrl+a/e も可)",
                        "go to line start (after the indent ⇄ column 0) / line end (Cmd+←/→, Ctrl+a/e also work)",
                    ),
                ),
                (
                    "Ctrl+Home/End",
                    t(
                        "文書の先頭 / 末尾へ (Cmd+↑/↓ も可)",
                        "go to the start / end of the document (Cmd+↑/↓ also works)",
                    ),
                ),
                (
                    "Alt+↑/↓",
                    t(
                        "カーソル行を上 / 下の行と入れ替える",
                        "swap the cursor line with the line above / below",
                    ),
                ),
                (
                    "Alt+Backspace",
                    t(
                        "手前の 1 単語を削除 (Ctrl+Backspace / Ctrl+w も可)",
                        "delete the previous word (Ctrl+Backspace, Ctrl+w also work)",
                    ),
                ),
                (
                    "Alt+Delete",
                    t(
                        "先の 1 単語を削除 (Ctrl+Delete も可)",
                        "delete the next word (Ctrl+Delete also works)",
                    ),
                ),
                (
                    "Cmd+Backspace",
                    t(
                        "行頭まで削除 (Ctrl+u も可) / Cmd+Delete: 行末まで削除",
                        "delete to the line start (Ctrl+u also works) / Cmd+Delete: delete to the line end",
                    ),
                ),
                ("Ctrl+s / Cmd+s", t("保存", "save")),
                (
                    "Ctrl+z / Ctrl+y",
                    t(
                        "undo / redo (Cmd+z / Cmd+Shift+z)",
                        "undo / redo (Cmd+z / Cmd+Shift+z)",
                    ),
                ),
                ("Ctrl+k", t("行削除", "delete the line")),
                (
                    "Esc",
                    t(
                        "終了 (未保存なら確認。確認中の s で保存して終了)",
                        "leave edit mode (confirms when unsaved; s in the confirmation saves and leaves)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Mouse,
            title: "Mouse",
            entries: vec![
                (
                    t("クリック", "click"),
                    t(
                        "ツリーの行を選択して開く / ペインをフォーカス",
                        "select and open a tree row / focus a pane",
                    ),
                ),
                (
                    t("ホイール", "wheel"),
                    t("ツリー移動 / スクロール", "move in the tree / scroll"),
                ),
                (
                    t("境界をドラッグ", "drag the divider"),
                    t(
                        "左右ペインの幅を変更 (離した時点で保存)",
                        "resize the left/right panes (saved on release)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Confirm,
            title: t(
                "Confirm (破壊的・書き込み系操作の確認)",
                "Confirm (for destructive and write operations)",
            ),
            entries: vec![
                ("y / Enter", t("実行", "run it")),
                (
                    t("n / Esc / それ以外", "n / Esc / other"),
                    t("中止", "cancel"),
                ),
            ],
        },
        Section {
            id: SectionId::Commit,
            title: t(
                "Commit (c / C、GIT レーンに限らず開ける)",
                "Commit (c / C, opens from any lane)",
            ),
            entries: vec![
                (t("文字入力", "typing"), t("挿入", "insert text")),
                ("Enter", t("改行", "new line")),
                ("↑/↓/←/→ Home/End", t("カーソル移動", "move the cursor")),
                (
                    "Ctrl+s",
                    t(
                        "確定 (amend は確認オーバーレイを経由)",
                        "commit (amend goes through the confirmation overlay)",
                    ),
                ),
                (
                    "Esc",
                    t(
                        "閉じる (書きかけは下書きとして残り、再度 c/C で復元)",
                        "close (what you typed is kept as a draft and restored on the next c/C)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Branch,
            title: t(
                "Branch (b、レーンを問わず開ける)",
                "Branch (b, opens from any lane)",
            ),
            entries: vec![
                (
                    t("文字入力", "typing"),
                    t("ブランチ名をファジー絞り込み", "fuzzy filter branch names"),
                ),
                (
                    "↑/↓ Ctrl+p",
                    t(
                        "候補選択 (Ctrl+n は新規作成に予約)",
                        "select a candidate (Ctrl+n is reserved for creating one)",
                    ),
                ),
                (
                    "Enter",
                    t(
                        "選択中のブランチへ切替 (リモートは追跡ブランチを作成)",
                        "switch to the selected branch (remotes create a tracking branch)",
                    ),
                ),
                (
                    "Ctrl+n",
                    t(
                        "入力文字列が既存ブランチと不一致なら新規作成して切替",
                        "create and switch to a branch when the query matches no existing one",
                    ),
                ),
                ("Esc", t("閉じる", "close")),
            ],
        },
        Section {
            id: SectionId::Remote,
            title: t(
                "Remote (f / p / P、レーンを問わず開ける)",
                "Remote (f / p / P, opens from any lane)",
            ),
            entries: vec![
                (
                    "f",
                    t(
                        "fetch --prune (確認不要)",
                        "fetch --prune (no confirmation)",
                    ),
                ),
                (
                    "p",
                    t(
                        "pull --ff-only (確認不要・ff できないと git のエラーを表示)",
                        "pull --ff-only (no confirmation; shows git's error when it cannot fast-forward)",
                    ),
                ),
                (
                    "P",
                    t(
                        "push (確認あり。upstream が無ければ --set-upstream origin <branch>)",
                        "push (confirmed; uses --set-upstream origin <branch> when there is no upstream)",
                    ),
                ),
                (
                    t("実行中", "while running"),
                    t(
                        "ステータスバーにジョブ名を表示。他の操作は継続可能・同じ/別ジョブの多重起動は不可",
                        "the status bar shows the job name; you can keep working, but no second job can start",
                    ),
                ),
                (
                    t("完了後", "on completion"),
                    t(
                        "status / ahead-behind / 表示中 diff を再取得",
                        "refetches status, ahead/behind and the diff on screen",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Finder,
            title: "Finder (Ctrl+p)",
            entries: vec![
                (
                    t("文字入力", "typing"),
                    t("クエリを絞り込み", "filter by query"),
                ),
                ("↑/↓ Ctrl+n/p", t("候補選択", "select a candidate")),
                ("Backspace", t("一文字削除", "delete one character")),
                ("Enter", t("開く", "open")),
                ("Esc", t("閉じる", "close")),
            ],
        },
        Section {
            id: SectionId::Grep,
            title: "Grep (Ctrl+f)",
            entries: vec![
                (
                    t("文字入力", "typing"),
                    t(
                        "クエリ (部分一致・smart-case・2 文字以上。打鍵が止まると repo 全体を歩き直す)",
                        "the query (substring, smart-case, 2+ characters; walks the repo again once you stop typing)",
                    ),
                ),
                ("↑/↓ Ctrl+n/p", t("ヒット選択", "select a hit")),
                (
                    "Backspace / Ctrl+u",
                    t("一文字削除 / 全消去", "delete one character / clear all"),
                ),
                (
                    "Enter",
                    t(
                        "開いてその行へ (同じクエリで / を立てるので n/N が続けて効く)",
                        "open at that line (the same query is set for /, so n/N keep working)",
                    ),
                ),
                (
                    "Esc",
                    t(
                        "閉じる (結果は残り、次に開いた時にそのまま見える)",
                        "close (the results stay and are still there next time you open it)",
                    ),
                ),
                (
                    t("タイトル", "title"),
                    t(
                        "searching... / N files scanned / truncated (5000 件で打ち切り) / stale (変更あり)",
                        "searching... / N files scanned / truncated (cut off at 5000 hits) / stale (files changed)",
                    ),
                ),
            ],
        },
        Section {
            id: SectionId::Input,
            title: t(
                "Search・Goto・ファイル操作の入力 (/ と :N と n/N/R)",
                "Search / Goto / file-op input (/ and :N and n/N/R)",
            ),
            entries: vec![
                (
                    t("文字入力", "typing"),
                    t(
                        "入力 (Goto は数字のみ)",
                        "type the input (Goto takes digits only)",
                    ),
                ),
                ("Backspace", t("一文字削除", "delete one character")),
                ("Enter", t("確定", "confirm")),
                ("Esc", t("キャンセル", "cancel")),
            ],
        },
    ]
}

// key 列を固定幅で左詰めし、"キー  説明" の2カラム風に整列させる
fn push_help_section(lines: &mut Vec<Line<'static>>, title: &str, entries: &[(&str, &str)]) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    for (key, desc) in entries {
        lines.push(Line::from(format!("  {key:<16}{desc}")));
    }
    lines.push(Line::from(""));
}
