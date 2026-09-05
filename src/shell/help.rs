use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::{App, Focus, Lane, Workspace};
use crate::lang::{Msg, t};
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
            Msg::HelpTitleScroll,
            from = (scroll + 1).min(total),
            to = (scroll + height).min(total),
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
            tr!(Msg::HelpCurrentScreen, title = section.title)
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
                ("Ctrl+c", t(Msg::HelpQuit)),
                ("q", t(Msg::HelpQuit)),
                ("Shift+Tab", t(Msg::HelpSwitchLaneVIEWEDITGIT)),
                ("Tab", t(Msg::HelpSwitchFocusTreeViewerLog)),
                ("L", t(Msg::HelpToggleCommitListPanelVIEW)),
                ("Ctrl+p", t(Msg::HelpOpenFinder)),
                ("Ctrl+f", t(Msg::HelpOpenWorkspaceWideSearch)),
                ("b", t(Msg::HelpOpenBranchListOverlayGit)),
                ("?", t(Msg::HelpOpenHelp)),
                ("s", t(Msg::HelpOpenSettings)),
                ("a", t(Msg::HelpToggleHiddenItems)),
                ("i", t(Msg::HelpToggleIgnoredFilesGitignoreIgnore)),
                ("-a, --hidden", t(Msg::HelpShowHiddenItemsOnStartup)),
                ("-i, --ignored", t(Msg::HelpShowIgnoredFilesOnStartup)),
                (
                    t(Msg::HelpStatusBar),
                    t(Msg::HelpAlwaysShowsCurrentBranchAhead),
                ),
            ],
        },
        Section {
            id: SectionId::Help,
            title: "Help (?)",
            entries: vec![
                ("j/k ↑/↓", t(Msg::HelpScrollOneLine)),
                ("Ctrl+d/u", t(Msg::HelpScrollHalfPage)),
                ("gg / G", t(Msg::HelpGoTopBottom)),
                ("? / Esc / q", t(Msg::HelpClose)),
                (
                    t(Msg::HelpSectionOrder),
                    t(Msg::HelpSectionForCurrentScreenComes),
                ),
            ],
        },
        Section {
            id: SectionId::Workspace,
            title: t(Msg::HelpWorkspaceSection),
            entries: vec![
                ("Ctrl+t", t(Msg::HelpGoNextTabViewerIssues)),
                ("Alt+1/2/3", t(Msg::HelpJumpStraightViewerIssuesPull)),
                (t(Msg::HelpClickTab), t(Msg::HelpSwitchTab)),
                ("--github", t(Msg::HelpEnableForRunOnlyNot)),
                (
                    t(Msg::HelpGithubTabsInSettings),
                    t(Msg::HelpToggleEnablePersistConfig),
                ),
            ],
        },
        Section {
            id: SectionId::Issues,
            title: t(Msg::HelpIssuesSection),
            entries: vec![
                ("j/k ↑/↓ gg/G", t(Msg::HelpMoveThroughList)),
                ("Ctrl+d/u", t(Msg::HelpMoveHalfPageInList)),
                ("Tab", t(Msg::HelpSwitchFocusBetweenListDetail)),
                (
                    t(Msg::HelpEnterLClick),
                    t(Msg::HelpLoadSelectedIssueSDetail),
                ),
                ("o", t(Msg::HelpOpenInBrowserGhIssue)),
                ("r", t(Msg::HelpRefetchListSwitchingTabsDoes)),
                ("/", t(Msg::HelpFuzzyFilterListOnlyWhile)),
                ("t", t(Msg::HelpCycleStateFilterOpenClosed)),
            ],
        },
        Section {
            id: SectionId::Prs,
            title: t(Msg::HelpPrsSection),
            entries: vec![
                ("j/k ↑/↓ gg/G", t(Msg::HelpMoveThroughList)),
                ("Ctrl+d/u", t(Msg::HelpMoveHalfPageInListScrollRight)),
                ("Tab", t(Msg::HelpSwitchFocusBetweenListDetail)),
                (
                    t(Msg::HelpEnterLClick),
                    t(Msg::HelpOpenSelectedPRInDescription),
                ),
                ("d", t(Msg::HelpShowDiffRenderedLikeGIT)),
                ("S", t(Msg::HelpShowCIStatusUppercaseBecause)),
                (t(Msg::HelpJKDiff), t(Msg::HelpMoveLineCursor)),
                (t(Msg::HelpDiff), t(Msg::HelpGoNextPreviousHunk)),
                (t(Msg::HelpWDiff), t(Msg::HelpToggleWrapNotSavedConfig)),
                (t(Msg::HelpHLDiff), t(Msg::HelpScrollHorizontally)),
                ("o", t(Msg::HelpOpenInBrowserGhPr)),
                ("r", t(Msg::HelpRefetchListSwitchingTabsDoes)),
                ("/", t(Msg::HelpFuzzyFilterListOnlyWhile)),
                ("t", t(Msg::HelpCycleStateFilterOpenClosedMergedAll)),
                (t(Msg::HelpHugeDiffs), t(Msg::HelpTruncatedAtLineByteLimit)),
            ],
        },
        Section {
            id: SectionId::Tree,
            title: "Tree",
            entries: vec![
                ("j/k ↑/↓", t(Msg::HelpMoveUpDown)),
                ("l →", t(Msg::HelpExpandOpen)),
                ("h ←", t(Msg::HelpCollapseGoParent)),
                ("H", t(Msg::HelpSelectParentCollapse)),
                ("Enter", t(Msg::HelpOpenToggleExpansion)),
                ("gg / G", t(Msg::HelpGoTopBottom)),
                ("r", t(Msg::HelpRescan)),
            ],
        },
        Section {
            id: SectionId::Viewer,
            title: "Viewer",
            entries: vec![
                ("j/k ↑/↓", t(Msg::HelpMoveLineCursorViewFollows)),
                ("Ctrl+d/u", t(Msg::HelpScrollHalfPage)),
                ("gg / G", t(Msg::HelpGoTopBottom)),
                ("w", t(Msg::HelpToggleWrap)),
                ("h/l ←/→", t(Msg::HelpScrollHorizontally)),
                ("0", t(Msg::HelpResetHorizontalScroll)),
                ("Ctrl+o", t(Msg::HelpGoBackInHistoryBackspace)),
                ("Ctrl+i", t(Msg::HelpGoForwardInHistory)),
                (":N Enter", t(Msg::HelpJumpLineN)),
                ("/", t(Msg::HelpSearch)),
                ("n / N", t(Msg::HelpGoNextPreviousMatch)),
                ("e", t(Msg::HelpEnterEditMode)),
                (
                    t(Msg::HelpMouseDrag),
                    t(Msg::HelpCharacterWiseSelectionDraggingPast),
                ),
                ("v", t(Msg::HelpStartCancelLineWiseSelection)),
                ("y", t(Msg::HelpCopySelectionClipboard)),
                ("Y", t(Msg::HelpCopyWholeOpenFile)),
                ("Esc", t(Msg::HelpClearSelection)),
                (t(Msg::HelpCopyBackend), t(Msg::HelpPbcopyWlCopyXclipXsel)),
            ],
        },
        Section {
            id: SectionId::Git,
            title: "Git (Shift+Tab)",
            entries: vec![
                (t(Msg::HelpLeftPane), t(Msg::HelpShowsOnlyChangedFilesWith)),
                ("j/k ↑/↓", t(Msg::HelpMoveBetweenChangedFiles)),
                ("l →", t(Msg::HelpExpandShowDiff)),
                ("h ←", t(Msg::HelpCollapseGoParent)),
                ("H", t(Msg::HelpSelectParentCollapse)),
                ("Enter", t(Msg::HelpShowDiffToggleExpansion)),
                (
                    t(Msg::HelpSpaceLeftPane),
                    t(Msg::HelpStageUnstageSelectedFileDirectory),
                ),
                (t(Msg::HelpJKDiffPane), t(Msg::HelpMoveLineCursorSpaceEnter)),
                (
                    t(Msg::HelpSpaceDiffPane),
                    t(Msg::HelpStageHunkCursorInUnstage),
                ),
                (
                    t(Msg::HelpEnterDiffPane),
                    t(Msg::HelpStageUnstageOnlyChangedLines),
                ),
                (
                    t(Msg::HelpVDiffPane),
                    t(Msg::HelpStartCancelLineWiseSelectionGrowWith),
                ),
                (t(Msg::HelpClickDiffPane), t(Msg::HelpMoveCursorLine)),
                ("X", t(Msg::HelpDiscardChangesInSelectedFile)),
                ("z", t(Msg::HelpStashChangesConfirmedUntrackedFiles)),
                ("Z", t(Msg::HelpPopLatestStashConfirmedWorks)),
                ("/", t(Msg::HelpSearchWithinDiffDisabledIn)),
                ("n / N", t(Msg::HelpGoNextPreviousMatchAfter)),
                ("] / [", t(Msg::HelpGoNextPreviousHunk)),
                ("A", t(Msg::HelpShowCombinedDiffAllChanged)),
                ("} / {", t(Msg::HelpGoNextPreviousFileIn)),
                ("t", t(Msg::HelpSwitchDiffBaseHEADStaged)),
                ("c", t(Msg::HelpCommitDoesNotOpenWhen)),
                ("C", t(Msg::HelpAmendLastCommitPrefillsExisting)),
                ("v", t(Msg::HelpSwitchInlineSideBySide)),
                ("Ctrl+d/u", t(Msg::HelpScrollHalfPage)),
                ("gg / G", t(Msg::HelpGoTopBottom)),
                ("w", t(Msg::HelpToggleWrapDiffOnlyNot)),
                ("h/l ←/→", t(Msg::HelpScrollHorizontallyDiffPane)),
                ("0", t(Msg::HelpResetHorizontalScrollDiffPane)),
                ("f / p / P", t(Msg::HelpFetchPullPushSeeRemote)),
                ("r", t(Msg::HelpRescanAlsoRefetchesGitStatus)),
            ],
        },
        Section {
            id: SectionId::LogPanel,
            title: "Log panel (L)",
            entries: vec![
                ("L", t(Msg::HelpTogglePanelLowerHalfVIEW)),
                (t(Msg::HelpListRows), t(Msg::HelpCommitListShortSHARelative)),
                ("j/k ↑/↓", t(Msg::HelpMoveBetweenCommitsDiffDoes)),
                ("Enter / l →", t(Msg::HelpShowSelectedCommitSDiff)),
                ("gg / G", t(Msg::HelpGoTopEndWhatLoaded)),
                (t(Msg::HelpEscList), t(Msg::HelpClosePanelSameAsL)),
                (t(Msg::HelpLogJKDiff), t(Msg::HelpMoveLineCursor)),
                ("n / N", t(Msg::HelpGoNextPreviousHunkDo)),
                ("Ctrl+d/u", t(Msg::HelpScrollHalfPage)),
                ("w", t(Msg::HelpToggleWrapDiffOnlyNot)),
                ("h/l ←/→", t(Msg::HelpScrollHorizontallyDiffPane)),
                ("0", t(Msg::HelpResetHorizontalScrollDiffPane)),
                (t(Msg::HelpEscDiff), t(Msg::HelpCloseDiffGoBackFile)),
                (
                    t(Msg::HelpMergeCommits),
                    t(Msg::HelpShowsDiffAgainstFirstParent),
                ),
            ],
        },
        Section {
            id: SectionId::Edit,
            title: "Edit (e / Shift+Tab)",
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpInsertTextClickMoveCursor)),
                ("↑/↓/←/→", t(Msg::HelpMoveCursor)),
                ("Alt+←/→", t(Msg::HelpMoveByWordOptionCtrl)),
                ("Home/End", t(Msg::HelpGoLineStartAfterIndent)),
                ("Ctrl+Home/End", t(Msg::HelpGoStartEndDocumentCmd)),
                ("Alt+↑/↓", t(Msg::HelpSwapCursorLineWithLine)),
                ("Alt+Backspace", t(Msg::HelpDeletePreviousWordCtrlBackspace)),
                ("Alt+Delete", t(Msg::HelpDeleteNextWordCtrlDelete)),
                ("Cmd+Backspace", t(Msg::HelpDeleteLineStartCtrlU)),
                ("Ctrl+s / Cmd+s", t(Msg::HelpSave)),
                ("Ctrl+z / Ctrl+y", t(Msg::HelpUndoRedoCmdZCmd)),
                ("Ctrl+k", t(Msg::HelpDeleteLine)),
                ("Esc", t(Msg::HelpLeaveEditModeConfirmsWhen)),
            ],
        },
        Section {
            id: SectionId::Mouse,
            title: "Mouse",
            entries: vec![
                (t(Msg::HelpClick), t(Msg::HelpSelectOpenTreeRowFocus)),
                (t(Msg::HelpWheel), t(Msg::HelpMoveInTreeScroll)),
                (
                    t(Msg::HelpDragDivider),
                    t(Msg::HelpResizeLeftRightPanesSaved),
                ),
            ],
        },
        Section {
            id: SectionId::Confirm,
            title: t(Msg::HelpConfirmForDestructiveWriteOperations),
            entries: vec![
                ("y / Enter", t(Msg::HelpRun)),
                (t(Msg::HelpNEscOther), t(Msg::HelpCancel)),
            ],
        },
        Section {
            id: SectionId::Commit,
            title: t(Msg::HelpCommitCCOpensFrom),
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpInsertText)),
                ("Enter", t(Msg::HelpNewLine)),
                ("↑/↓/←/→ Home/End", t(Msg::HelpMoveCursor)),
                ("Ctrl+s", t(Msg::HelpCommitAmendGoesThroughConfirmation)),
                ("Esc", t(Msg::HelpCloseWhatYouTypedKept)),
            ],
        },
        Section {
            id: SectionId::Branch,
            title: t(Msg::HelpBranchBOpensFromAny),
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpFuzzyFilterBranchNames)),
                ("↑/↓ Ctrl+p", t(Msg::HelpSelectCandidateCtrlNReserved)),
                ("Enter", t(Msg::HelpSwitchSelectedBranchRemotesCreate)),
                ("Ctrl+n", t(Msg::HelpCreateSwitchBranchWhenQuery)),
                ("Esc", t(Msg::HelpClose)),
            ],
        },
        Section {
            id: SectionId::Remote,
            title: t(Msg::HelpRemoteFPPOpens),
            entries: vec![
                ("f", t(Msg::HelpFetchPruneNoConfirmation)),
                ("p", t(Msg::HelpPullFfOnlyNoConfirmation)),
                ("P", t(Msg::HelpPushConfirmedUsesSetUpstream)),
                (t(Msg::HelpWhileRunning), t(Msg::HelpStatusBarShowsJobName)),
                (
                    t(Msg::HelpOnCompletion),
                    t(Msg::HelpRefetchesStatusAheadBehindDiff),
                ),
            ],
        },
        Section {
            id: SectionId::Finder,
            title: "Finder (Ctrl+p)",
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpFilterByQuery)),
                ("↑/↓ Ctrl+n/p", t(Msg::HelpSelectCandidate)),
                ("Backspace", t(Msg::HelpDeleteOneCharacter)),
                ("Enter", t(Msg::HelpOpen)),
                ("Esc", t(Msg::HelpClose)),
            ],
        },
        Section {
            id: SectionId::Grep,
            title: "Grep (Ctrl+f)",
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpGrepQuery)),
                ("↑/↓ Ctrl+n/p", t(Msg::HelpSelectHit)),
                ("Backspace / Ctrl+u", t(Msg::HelpDeleteOneCharacterClearAll)),
                ("Enter", t(Msg::HelpOpenAtLineSameQuery)),
                ("Esc", t(Msg::HelpCloseResultsStayAreStill)),
                (
                    t(Msg::HelpTitle),
                    t(Msg::HelpSearchingNFilesScannedTruncated),
                ),
            ],
        },
        Section {
            id: SectionId::Input,
            title: t(Msg::HelpSearchGotoN),
            entries: vec![
                (t(Msg::HelpTyping), t(Msg::HelpTypeInputGotoTakesDigits)),
                ("Backspace", t(Msg::HelpDeleteOneCharacter)),
                ("Enter", t(Msg::HelpConfirm)),
                ("Esc", t(Msg::HelpCancelInput)),
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
