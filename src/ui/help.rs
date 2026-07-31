use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::centered_rect;

// キーバインド一覧のオーバーレイ。実装済みのハンドラ (app/keys.rs の on_*_key) と
// 一対一で対応させる。ここに書いた内容と実際の挙動がずれないよう追加時は両方直す
pub(super) fn draw_help(frame: &mut Frame, area: Rect) {
    let popup = centered_rect(70, 80, area);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title("help");

    let mut lines: Vec<Line> = Vec::new();
    push_help_section(
        &mut lines,
        "Global",
        &[
            ("Ctrl+c", "終了"),
            ("q", "終了"),
            ("Shift+Tab", "モード切替 (VIEW → EDIT → GIT → LOG)"),
            ("Tab", "フォーカス切替 (Tree/Viewer)"),
            ("Ctrl+p", "ファインダーを開く"),
            ("b", "ブランチ一覧オーバーレイを開く (git repo でのみ)"),
            ("?", "このヘルプを開く"),
            ("s", "設定画面を開く"),
            ("a", "隠し項目の表示を切替"),
            ("-a, --hidden", "起動時に隠し項目を表示"),
            (
                "ステータスバー",
                "現在ブランチ + ahead/behind を常時表示 (git repo のみ)",
            ),
        ],
    );
    push_help_section(
        &mut lines,
        "Workspace (GitHub モード、既定は無効)",
        &[
            ("Ctrl+t", "次のタブへ (viewer → issues → pull requests)"),
            ("Alt+1/2/3", "viewer / issues / pull requests へ直接切替"),
            ("タブをクリック", "そのタブへ切替"),
            ("--github", "起動時だけ有効化 (config には保存しない)"),
            ("設定画面の github tabs", "トグルで有効化・config に永続化"),
        ],
    );
    push_help_section(
        &mut lines,
        "Tree",
        &[
            ("j/k ↑/↓", "上下移動"),
            ("l →", "展開 / 開く"),
            ("h ←", "折りたたみ / 親へ"),
            ("H", "親を選択して折りたたむ"),
            ("Enter", "開く / 展開切替"),
            ("gg / G", "先頭 / 末尾へ"),
            ("r", "再走査"),
        ],
    );
    push_help_section(
        &mut lines,
        "Viewer",
        &[
            ("j/k ↑/↓", "スクロール"),
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
        ],
    );
    push_help_section(
        &mut lines,
        "Git (Shift+Tab)",
        &[
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
                "Space",
                "選択中のファイル/ディレクトリを stage/unstage トグル",
            ),
            ("n / N", "次 / 前の hunk へ (] / [ も同様)"),
            ("t", "diff 基準を切替 (HEAD → staged → unstaged)"),
            ("c", "コミット (staged が空だと開かない)"),
            ("C", "amend コミット (既存メッセージをプリフィル・確認あり)"),
            ("v", "inline ⇔ side-by-side 切替 (設定には保存しない)"),
            ("Ctrl+d/u", "半ページスクロール"),
            ("gg / G", "先頭 / 末尾へ"),
            ("w", "折り返し切替 (diff のみ・設定には保存しない)"),
            ("h/l ←/→", "水平スクロール (diff ペイン)"),
            ("r", "再走査 (git status も取り直す)"),
        ],
    );
    push_help_section(
        &mut lines,
        "Log (Shift+Tab)",
        &[
            (
                "左ペイン",
                "コミット一覧 (短縮 SHA / 相対日時 / 作者 / 件名)",
            ),
            ("j/k ↑/↓", "コミット間を移動 (diff は追従しない)"),
            ("Enter / l →", "選択コミットの diff を表示"),
            ("gg / G", "先頭 / 読み込み済み末尾へ (末尾で追加取得)"),
            ("n / N", "次 / 前の hunk へ (] / [ も同様)"),
            ("Ctrl+d/u", "半ページスクロール"),
            ("w", "折り返し切替 (diff のみ・設定には保存しない)"),
            ("h/l ←/→", "水平スクロール (diff ペイン)"),
            (
                "マージコミット",
                "最初の親との diff を表示 (git show の既定は差分なし)",
            ),
        ],
    );
    push_help_section(
        &mut lines,
        "Edit (e / Shift+Tab)",
        &[
            ("文字入力", "挿入 (クリックでカーソル移動)"),
            ("↑/↓/←/→", "カーソル移動"),
            ("Ctrl+←/→", "単語単位で移動"),
            ("Home/End", "行頭 / 行末へ (Cmd+←/→ も可)"),
            ("Ctrl+s / Cmd+s", "保存"),
            ("Ctrl+z / Ctrl+y", "undo / redo (Cmd+z / Cmd+Shift+z)"),
            ("Ctrl+k", "行削除"),
            ("Esc", "終了 (未保存なら確認。確認中の s で保存して終了)"),
        ],
    );
    push_help_section(
        &mut lines,
        "Mouse",
        &[
            ("クリック", "ツリーの行を選択して開く / ペインをフォーカス"),
            ("ホイール", "ツリー移動 / スクロール"),
            ("境界をドラッグ", "左右ペインの幅を変更 (離した時点で保存)"),
        ],
    );
    push_help_section(
        &mut lines,
        "Confirm (破壊的・書き込み系操作の確認)",
        &[("y / Enter", "実行"), ("n / Esc / それ以外", "中止")],
    );
    push_help_section(
        &mut lines,
        "Commit (c / C、GIT レーンに限らず開ける)",
        &[
            ("文字入力", "挿入"),
            ("Enter", "改行"),
            ("↑/↓/←/→ Home/End", "カーソル移動"),
            ("Ctrl+s", "確定 (amend は確認オーバーレイを経由)"),
            (
                "Esc",
                "閉じる (書きかけは下書きとして残り、再度 c/C で復元)",
            ),
        ],
    );
    push_help_section(
        &mut lines,
        "Branch (b、レーンを問わず開ける)",
        &[
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
    );
    push_help_section(
        &mut lines,
        "Finder (Ctrl+p)",
        &[
            ("文字入力", "クエリを絞り込み"),
            ("↑/↓ Ctrl+n/p", "候補選択"),
            ("Backspace", "一文字削除"),
            ("Enter", "開く"),
            ("Esc", "閉じる"),
        ],
    );
    push_help_section(
        &mut lines,
        "Search・Goto (/ と :N)",
        &[
            ("文字入力", "入力 (Goto は数字のみ)"),
            ("Backspace", "一文字削除"),
            ("Enter", "確定"),
            ("Esc", "キャンセル"),
        ],
    );

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, popup);
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
