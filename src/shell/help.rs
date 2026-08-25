use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Focus, Lane, Workspace};
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
        format!(
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
    Input,
}

struct Section {
    id: SectionId,
    title: &'static str,
    entries: &'static [(&'static str, &'static str)],
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
            format!("{} ← 今の画面", section.title)
        } else {
            section.title.to_string()
        };
        push_help_section(&mut lines, &title, section.entries);
    }
    lines
}

fn sections() -> Vec<Section> {
    vec![
        Section {
            id: SectionId::Global,
            title: "Global",
            entries: &[
                ("Ctrl+c", "終了"),
                ("q", "終了"),
                ("Shift+Tab", "モード切替 (VIEW → EDIT → GIT)"),
                (
                    "Tab",
                    "フォーカス切替 (Tree → Viewer。コミット一覧を出している間だけ Log を挟む)",
                ),
                (
                    "L",
                    "コミット一覧パネルの表示切替 (VIEW のみ・左ペイン下半分)",
                ),
                ("Ctrl+p", "ファインダーを開く"),
                ("b", "ブランチ一覧オーバーレイを開く (git repo でのみ)"),
                ("?", "このヘルプを開く"),
                ("s", "設定画面を開く"),
                ("a", "隠し項目の表示を切替"),
                (
                    "i",
                    "無視ファイル (.gitignore/.ignore/exclude) の表示を切替",
                ),
                ("-a, --hidden", "起動時に隠し項目を表示"),
                ("-i, --ignored", "起動時に無視ファイルも表示"),
                (
                    "ステータスバー",
                    "現在ブランチ + ahead/behind を常時表示 (git repo のみ)",
                ),
            ],
        },
        Section {
            id: SectionId::Help,
            title: "Help (?)",
            entries: &[
                ("j/k ↑/↓", "1 行スクロール"),
                ("Ctrl+d/u", "半ページスクロール"),
                ("gg / G", "先頭 / 末尾へ"),
                ("? / Esc / q", "閉じる"),
                (
                    "節の並び",
                    "今開いている画面の節が先頭に来る (残りは定義順)",
                ),
            ],
        },
        Section {
            id: SectionId::Workspace,
            title: "Workspace (GitHub モード、既定は無効)",
            entries: &[
                ("Ctrl+t", "次のタブへ (viewer → issues → pull requests)"),
                ("Alt+1/2/3", "viewer / issues / pull requests へ直接切替"),
                ("タブをクリック", "そのタブへ切替"),
                ("--github", "起動時だけ有効化 (config には保存しない)"),
                ("設定画面の github tabs", "トグルで有効化・config に永続化"),
            ],
        },
        Section {
            id: SectionId::Issues,
            title: "Issues (Ctrl+t / Alt+2、GitHub モード有効時)",
            entries: &[
                ("j/k ↑/↓ gg/G", "一覧を移動"),
                ("Ctrl+d/u", "一覧を半ページ移動 / 詳細を半ページスクロール"),
                ("Tab", "一覧 ⇄ 詳細のフォーカス切替"),
                ("Enter / l / クリック", "選択 issue の詳細を右に読み込む"),
                ("o", "ブラウザで開く (gh issue view --web)"),
                ("r", "一覧を再取得 (タブ往復では自動取得しない)"),
                ("/", "一覧をファジー絞り込み (一覧側フォーカス時のみ)"),
                ("t", "state 絞り込みを循環 (open → closed → all)"),
            ],
        },
        Section {
            id: SectionId::Prs,
            title: "Pull Requests (Ctrl+t / Alt+3、GitHub モード有効時)",
            entries: &[
                ("j/k ↑/↓ gg/G", "一覧を移動"),
                (
                    "Ctrl+d/u",
                    "一覧を半ページ移動 / 右ペインを半ページスクロール",
                ),
                ("Tab", "一覧 ⇄ 詳細のフォーカス切替"),
                ("Enter / l / クリック", "選択 PR を説明表示で開く"),
                ("d", "差分を表示 (GIT/LOG レーンと同じ見え方)"),
                (
                    "S",
                    "CI ステータスを表示 (s は設定に割り当て済みのため大文字)",
                ),
                ("j/k ↑/↓ (diff 表示中)", "行カーソルを移動"),
                ("]/[ (diff 表示中)", "次 / 前の hunk へ"),
                ("w (diff 表示中)", "折り返し切替 (設定には保存しない)"),
                ("h/l ←/→ (diff 表示中)", "水平スクロール"),
                ("o", "ブラウザで開く (gh pr view --web)"),
                ("r", "一覧を再取得 (タブ往復では自動取得しない)"),
                ("/", "一覧をファジー絞り込み (一覧側フォーカス時のみ)"),
                ("t", "state 絞り込みを循環 (open → closed → merged → all)"),
                (
                    "巨大な diff",
                    "行数/バイト数の上限で打ち切り、notice で通知",
                ),
            ],
        },
        Section {
            id: SectionId::Tree,
            title: "Tree",
            entries: &[
                ("j/k ↑/↓", "上下移動"),
                ("l →", "展開 / 開く"),
                ("h ←", "折りたたみ / 親へ"),
                ("H", "親を選択して折りたたむ"),
                ("Enter", "開く / 展開切替"),
                ("gg / G", "先頭 / 末尾へ"),
                ("r", "再走査"),
            ],
        },
        Section {
            id: SectionId::Viewer,
            title: "Viewer",
            entries: &[
                (
                    "j/k ↑/↓",
                    "行カーソルを移動 (画面はカーソルに追従する。v/e の起点もこの行)",
                ),
                ("Ctrl+d/u", "半ページスクロール"),
                ("gg / G", "先頭 / 末尾へ"),
                ("w", "折り返し切替"),
                ("h/l ←/→", "水平スクロール"),
                ("0", "水平スクロールをリセット"),
                ("Ctrl+o", "履歴を戻る (Backspace も同様)"),
                ("Ctrl+i", "履歴を進む"),
                (":N Enter", "N 行目へジャンプ"),
                ("/", "検索"),
                ("n / N", "次 / 前のマッチへ"),
                ("e", "編集モードに入る"),
                (
                    "マウス長押し + 移動",
                    "文字単位の範囲選択 (端まで引っ張ると 1 行ずつ送る)",
                ),
                (
                    "v",
                    "行単位の選択を開始 / 解除 (j/k・Ctrl+d/u・gg/G で伸縮)",
                ),
                ("y", "選択範囲をクリップボードへコピー"),
                ("Y", "開いているファイル全体をコピー"),
                ("Esc", "選択を解除"),
                (
                    "コピー手段",
                    "pbcopy/wl-copy/xclip/xsel/clip.exe → 無ければ OSC 52 (ssh 越しも可)",
                ),
            ],
        },
        Section {
            id: SectionId::Git,
            title: "Git (Shift+Tab)",
            entries: &[
                (
                    "左ペイン",
                    "変更ファイルのみを階層付きで表示 (入った時点で全展開)",
                ),
                ("j/k ↑/↓", "変更ファイル間を移動"),
                ("l →", "展開 / diff を表示"),
                ("h ←", "折りたたみ / 親へ"),
                ("H", "親を選択して折りたたむ"),
                ("Enter", "diff を表示 / 展開切替"),
                (
                    "Space (左ペイン)",
                    "選択中のファイル/ディレクトリを stage/unstage トグル",
                ),
                (
                    "j/k ↑/↓ (diff ペイン)",
                    "行カーソルを移動 (Space/Enter の対象は常にこのカーソル行)",
                ),
                (
                    "Space (diff ペイン)",
                    "カーソル行が属する hunk を stage (基準が staged のときは unstage)",
                ),
                (
                    "Enter (diff ペイン)",
                    "カーソル行 (V の選択中はその範囲) の変更行だけを stage/unstage",
                ),
                (
                    "V (diff ペイン)",
                    "行単位選択の開始/解除 (j/k で伸縮・Esc で解除)",
                ),
                ("クリック (diff ペイン)", "その行へカーソルを移動"),
                (
                    "X",
                    "選択中のファイル/ディレクトリの変更を破棄 (確認あり・untracked は削除)",
                ),
                ("z", "変更を stash へ退避 (確認あり・untracked も含む)"),
                (
                    "Z",
                    "直近の stash を pop (確認あり・GIT レーン以外からも可)",
                ),
                ("/", "diff 内検索 (side-by-side 表示中は無効)"),
                ("n / N", "次 / 前の検索マッチへ (検索確定後)"),
                ("] / [", "次 / 前の hunk へ"),
                ("A", "全変更ファイルをまとめた diff を表示 (トグル)"),
                ("} / {", "まとめ diff 内で次 / 前のファイルへ"),
                ("t", "diff 基準を切替 (HEAD → staged → unstaged)"),
                ("c", "コミット (staged が空だと開かない)"),
                ("C", "amend コミット (既存メッセージをプリフィル・確認あり)"),
                (
                    "v",
                    "inline ⇔ side-by-side 切替 (設定には保存しない・まとめ diff 表示中は無効)",
                ),
                ("Ctrl+d/u", "半ページスクロール"),
                ("gg / G", "先頭 / 末尾へ"),
                ("w", "折り返し切替 (diff のみ・設定には保存しない)"),
                ("h/l ←/→", "水平スクロール (diff ペイン)"),
                ("0", "水平スクロールをリセット (diff ペイン)"),
                (
                    "f / p / P",
                    "fetch / pull / push (下の Remote セクション参照)",
                ),
                ("r", "再走査 (git status も取り直す)"),
            ],
        },
        Section {
            id: SectionId::LogPanel,
            title: "Log panel (L)",
            entries: &[
                (
                    "L",
                    "表示切替 (VIEW の左ペイン下半分。ツリーと同時に見える)",
                ),
                (
                    "一覧の行",
                    "コミット一覧 (短縮 SHA / 相対日時 / 作者 / 件名。狭い幅では右の列から落とす)",
                ),
                ("j/k ↑/↓", "コミット間を移動 (diff は追従しない)"),
                (
                    "Enter / l →",
                    "選択コミットの diff を右ペインに表示 (フォーカスも移る)",
                ),
                ("gg / G", "先頭 / 読み込み済み末尾へ (末尾で追加取得)"),
                ("Esc (一覧)", "パネルを閉じる (L と同じ)"),
                ("j/k ↑/↓ (diff)", "行カーソルを移動"),
                ("n / N", "次 / 前の hunk へ (] / [ も同様)"),
                ("Ctrl+d/u", "半ページスクロール"),
                ("w", "折り返し切替 (diff のみ・設定には保存しない)"),
                ("h/l ←/→", "水平スクロール (diff ペイン)"),
                ("0", "水平スクロールをリセット (diff ペイン)"),
                ("Esc (diff)", "diff を閉じてファイル表示へ戻す"),
                (
                    "マージコミット",
                    "最初の親との diff を表示 (git show の既定は差分なし)",
                ),
            ],
        },
        Section {
            id: SectionId::Edit,
            title: "Edit (e / Shift+Tab)",
            entries: &[
                ("文字入力", "挿入 (クリックでカーソル移動)"),
                ("↑/↓/←/→", "カーソル移動"),
                ("Ctrl+←/→", "単語単位で移動"),
                ("Home/End", "行頭 / 行末へ (Cmd+←/→ も可)"),
                ("Ctrl+s / Cmd+s", "保存"),
                ("Ctrl+z / Ctrl+y", "undo / redo (Cmd+z / Cmd+Shift+z)"),
                ("Ctrl+k", "行削除"),
                ("Esc", "終了 (未保存なら確認。確認中の s で保存して終了)"),
            ],
        },
        Section {
            id: SectionId::Mouse,
            title: "Mouse",
            entries: &[
                ("クリック", "ツリーの行を選択して開く / ペインをフォーカス"),
                ("ホイール", "ツリー移動 / スクロール"),
                ("境界をドラッグ", "左右ペインの幅を変更 (離した時点で保存)"),
            ],
        },
        Section {
            id: SectionId::Confirm,
            title: "Confirm (破壊的・書き込み系操作の確認)",
            entries: &[("y / Enter", "実行"), ("n / Esc / それ以外", "中止")],
        },
        Section {
            id: SectionId::Commit,
            title: "Commit (c / C、GIT レーンに限らず開ける)",
            entries: &[
                ("文字入力", "挿入"),
                ("Enter", "改行"),
                ("↑/↓/←/→ Home/End", "カーソル移動"),
                ("Ctrl+s", "確定 (amend は確認オーバーレイを経由)"),
                (
                    "Esc",
                    "閉じる (書きかけは下書きとして残り、再度 c/C で復元)",
                ),
            ],
        },
        Section {
            id: SectionId::Branch,
            title: "Branch (b、レーンを問わず開ける)",
            entries: &[
                ("文字入力", "ブランチ名をファジー絞り込み"),
                ("↑/↓ Ctrl+p", "候補選択 (Ctrl+n は新規作成に予約)"),
                (
                    "Enter",
                    "選択中のブランチへ切替 (リモートは追跡ブランチを作成)",
                ),
                (
                    "Ctrl+n",
                    "入力文字列が既存ブランチと不一致なら新規作成して切替",
                ),
                ("Esc", "閉じる"),
            ],
        },
        Section {
            id: SectionId::Remote,
            title: "Remote (f / p / P、レーンを問わず開ける)",
            entries: &[
                ("f", "fetch --prune (確認不要)"),
                (
                    "p",
                    "pull --ff-only (確認不要・ff できないと git のエラーを表示)",
                ),
                (
                    "P",
                    "push (確認あり。upstream が無ければ --set-upstream origin <branch>)",
                ),
                (
                    "実行中",
                    "ステータスバーにジョブ名を表示。他の操作は継続可能・同じ/別ジョブの多重起動は不可",
                ),
                ("完了後", "status / ahead-behind / 表示中 diff を再取得"),
            ],
        },
        Section {
            id: SectionId::Finder,
            title: "Finder (Ctrl+p)",
            entries: &[
                ("文字入力", "クエリを絞り込み"),
                ("↑/↓ Ctrl+n/p", "候補選択"),
                ("Backspace", "一文字削除"),
                ("Enter", "開く"),
                ("Esc", "閉じる"),
            ],
        },
        Section {
            id: SectionId::Input,
            title: "Search・Goto (/ と :N)",
            entries: &[
                ("文字入力", "入力 (Goto は数字のみ)"),
                ("Backspace", "一文字削除"),
                ("Enter", "確定"),
                ("Esc", "キャンセル"),
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
