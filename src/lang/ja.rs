//! 日本語の文言表。キーは msg.rs の `Msg`、英語は en.rs。
//! match を網羅させることで「片方の言語だけ書き忘れた文言」をコンパイルエラーにしている

use super::Msg;

pub(super) fn text(msg: Msg) -> &'static str {
    match msg {
        // Branch
        Msg::BranchUnsavedChangesSaveBeforeSwitching => {
            "未保存の変更があります。保存してから切り替えてください"
        }
        Msg::BranchFailedRunGit => "git の実行に失敗しました",
        Msg::BranchAlreadyExists => "ブランチ「{name}」は既に存在します (Enter で切替)",
        Msg::BranchSwitchedStale => {
            "{branch} に切り替えました (開いていたファイルが見つからないため閉じました)"
        }
        Msg::BranchSwitched => "{branch} に切り替えました",
        // Commit
        Msg::CommitUnsavedChangesSaveBeforeCommitting => {
            "未保存の変更があります。保存してからコミットしてください"
        }
        Msg::CommitNoStagedChangesSpaceStage => "ステージされた変更がありません (Space でステージ)",
        Msg::CommitTabEnterBodyCtrlCmd => {
            "Tab/Enter: 本文へ  Ctrl/Cmd+s: 確定  Esc: 閉じる (下書きを保持)"
        }
        Msg::CommitTabSubjectEnterNewlineCtrl => {
            "Tab: 件名へ  Enter: 改行  Ctrl/Cmd+s: 確定  Esc: 閉じる (下書きを保持)"
        }
        // Git
        Msg::GitFailedRunGit => "git の実行に失敗しました",
        Msg::GitCanTStageHunkWise => {
            "まとめ diff 表示中は hunk 単位でステージできません (A で解除)"
        }
        Msg::GitCanTStageUntrackedFiles => {
            "untracked は hunk 単位で stage できません (ツリー側の Space を使ってください)"
        }
        Msg::GitFailedApplyHunk => "hunk の適用に失敗しました",
        Msg::GitTrySwitchingUnstagedBaseWith => {
            " (t で unstaged 基準に切り替えると通ることがあります)"
        }
        Msg::GitCanTStageLineWise => "まとめ diff 表示中は行単位でステージできません (A で解除)",
        Msg::GitCanTStageLineWiseWhileSide => {
            "side-by-side 表示中は行単位でステージできません (v で inline に戻してください)"
        }
        Msg::GitCanTStageUntrackedFilesLineWise => {
            "untracked は行単位で stage できません (ツリー側の Space を使ってください)"
        }
        Msg::GitCanTStageRenamedFile => {
            "rename されたファイルは行単位で stage できません (Space でファイル単位に)"
        }
        Msg::GitCanTApplyPartNew => {
            "新規/削除ファイルの一部だけはこの向きでは反映できません (Space で hunk/ファイル単位に)"
        }
        Msg::GitCursorNotOnChangedLine => "カーソル行は変更行 (+/-) ではありません (V で範囲選択)",
        Msg::GitFailedApplyLines => "行の適用に失敗しました",
        Msg::GitUnsavedChangesSaveDiscardBefore => {
            "未保存の変更があります (保存または破棄してから実行してください)"
        }
        Msg::GitNUntrackedFilesWillBe => {
            "\n(untracked ファイルは削除されます。破棄すると復元できません)"
        }
        Msg::GitNothingDiscard => "破棄対象が見つかりませんでした",
        Msg::GitChangesDiscarded => "変更を破棄しました",
        Msg::GitFailedDiscard => "破棄に失敗しました",
        Msg::GitChangesStashed => "変更を stash に退避しました",
        Msg::GitFailedStashPush => "stash push に失敗しました",
        Msg::GitPopLatestStashNOn => {
            "直近の stash を pop しますか？\n(コンフリクト時は stash を残したままエラーを表示します)"
        }
        Msg::GitStashRestored => "stash を復元しました",
        Msg::GitFailedPopStashPossiblyConflict => {
            "stash pop に失敗しました (コンフリクトの可能性があります)"
        }
        Msg::GitNUnsavedEditsDonT => "\n(未保存の編集があります。保存を忘れずに)",
        Msg::GitTruncated => "  (打ち切り)",
        Msg::GitCannotRun => "git を実行できませんでした",
        Msg::GitStagedHunk => "hunk {ordinal}/{total} を {verb} しました",
        Msg::GitStagedLines => "{lines} 行を {verb} しました",
        Msg::GitDiscardPrompt => "{count} 件の変更を破棄しますか？\n{path}",
        Msg::GitStashPushPrompt => {
            "{count} 件の変更を stash に退避しますか？\n(untracked ファイルも含めて退避します)"
        }
        Msg::GitPushPrompt => "push を実行しますか？\n{target}",
        // App
        Msg::AppDiffTruncated => "diff が大きいため表示を打ち切りました (20000 行 / 2MB)",
        Msg::AppLineWiseSelectionIsnT => {
            "この表示では行単位選択を使えません (A の解除 / v で inline に戻してください)"
        }
        Msg::AppUnsavedChangesCtrlSSave => "未保存の変更があります (Ctrl+s: 保存 / Esc: 破棄)",
        Msg::AppNotGitRepository => "git リポジトリではありません",
        Msg::AppNoSelectionDragVSelect => "選択がありません (ドラッグ または v で選択)",
        Msg::AppNoTextCopy => "コピーできるテキストがありません",
        Msg::AppFetchDone => "fetch 完了",
        Msg::AppRemoteJobFailed => "{job} に失敗しました",
        Msg::AppFetchDoneWith => "fetch 完了: {message}",
        // Issues
        Msg::IssuesLoadingComments => "コメント読み込み中…",
        Msg::IssuesEnterLClickOpenDetail => "Enter / l / クリック: 詳細を開く",
        Msg::IssuesCommentsFetchFailed => "コメント取得に失敗しました: {err}",
        // Prs
        Msg::PrsDiffTruncated => "diff が大きいため表示を打ち切りました (20000 行 / 2MB)",
        Msg::PrsEnterLClickOpenDescription => "Enter / l / クリック: 説明を開く (d: diff  S: CI)",
        Msg::PrsTruncated => "  (打ち切り)",
        Msg::PrsLoading => "読み込み中…",
        Msg::PrsFetchFailed => "取得に失敗しました:\n{err}\n\n(d で再試行)",
        // Remote
        Msg::RemoteLoading => "読み込み中…",
        Msg::RemoteListFetchFailed => "取得に失敗しました:\n{err}\n\n(r で再取得)",
        Msg::RemoteDetailFetchFailed => "取得に失敗しました:\n{err}\n\n(再試行で開き直せます)",
        // Gh
        Msg::GhGitHubModeUnavailableGhNot => {
            "GitHub モードを有効化できません: gh が未認証です (gh auth login)"
        }
        Msg::GhGitHubModeUnavailableGhCommand => {
            "GitHub モードを有効化できません: gh コマンドが見つかりません"
        }
        Msg::GhGitHubModeUnavailableOriginNot => {
            "GitHub モードを有効化できません: origin が GitHub リポジトリではありません"
        }
        Msg::GhGhCommandNotFound => "gh コマンドが見つかりません",
        Msg::GhFailedRunGh => "gh の実行に失敗しました",
        // Confirm
        Msg::ConfirmRun => ": 実行    ",
        Msg::ConfirmCancel => ": 中止",
        // Help
        Msg::HelpQuit => "終了",
        Msg::HelpSwitchLaneVIEWEDITGIT => "モード切替 (VIEW → EDIT → GIT)",
        Msg::HelpSwitchFocusTreeViewerLog => {
            "フォーカス切替 (Tree → Viewer。コミット一覧を出している間だけ Log を挟む)"
        }
        Msg::HelpToggleCommitListPanelVIEW => {
            "コミット一覧パネルの表示切替 (VIEW のみ・左ペイン下半分)"
        }
        Msg::HelpOpenFinder => "ファインダーを開く",
        Msg::HelpOpenWorkspaceWideSearch => "ワークスペース横断検索を開く",
        Msg::HelpOpenBranchListOverlayGit => "ブランチ一覧オーバーレイを開く (git repo でのみ)",
        Msg::HelpOpenHelp => "このヘルプを開く",
        Msg::HelpOpenSettings => "設定画面を開く",
        Msg::HelpToggleHiddenItems => "隠し項目の表示を切替",
        Msg::HelpToggleIgnoredFilesGitignoreIgnore => {
            "無視ファイル (.gitignore/.ignore/exclude) の表示を切替"
        }
        Msg::HelpShowHiddenItemsOnStartup => "起動時に隠し項目を表示",
        Msg::HelpShowIgnoredFilesOnStartup => "起動時に無視ファイルも表示",
        Msg::HelpStatusBar => "ステータスバー",
        Msg::HelpAlwaysShowsCurrentBranchAhead => {
            "現在ブランチ + ahead/behind を常時表示 (git repo のみ)"
        }
        Msg::HelpScrollOneLine => "1 行スクロール",
        Msg::HelpScrollHalfPage => "半ページスクロール",
        Msg::HelpGoTopBottom => "先頭 / 末尾へ",
        Msg::HelpClose => "閉じる",
        Msg::HelpSectionOrder => "節の並び",
        Msg::HelpSectionForCurrentScreenComes => "今開いている画面の節が先頭に来る (残りは定義順)",
        Msg::HelpWorkspaceSection => "Workspace (GitHub モード、既定は無効)",
        Msg::HelpGoNextTabViewerIssues => "次のタブへ (viewer → issues → pull requests)",
        Msg::HelpJumpStraightViewerIssuesPull => "viewer / issues / pull requests へ直接切替",
        Msg::HelpClickTab => "タブをクリック",
        Msg::HelpSwitchTab => "そのタブへ切替",
        Msg::HelpEnableForRunOnlyNot => "起動時だけ有効化 (config には保存しない)",
        Msg::HelpGithubTabsInSettings => "設定画面の github tabs",
        Msg::HelpToggleEnablePersistConfig => "トグルで有効化・config に永続化",
        Msg::HelpIssuesSection => "Issues (Ctrl+t / Alt+2、GitHub モード有効時)",
        Msg::HelpMoveThroughList => "一覧を移動",
        Msg::HelpMoveHalfPageInList => "一覧を半ページ移動 / 詳細を半ページスクロール",
        Msg::HelpSwitchFocusBetweenListDetail => "一覧 ⇄ 詳細のフォーカス切替",
        Msg::HelpEnterLClick => "Enter / l / クリック",
        Msg::HelpLoadSelectedIssueSDetail => "選択 issue の詳細を右に読み込む",
        Msg::HelpOpenInBrowserGhIssue => "ブラウザで開く (gh issue view --web)",
        Msg::HelpRefetchListSwitchingTabsDoes => "一覧を再取得 (タブ往復では自動取得しない)",
        Msg::HelpFuzzyFilterListOnlyWhile => "一覧をファジー絞り込み (一覧側フォーカス時のみ)",
        Msg::HelpCycleStateFilterOpenClosed => "state 絞り込みを循環 (open → closed → all)",
        Msg::HelpPrsSection => "Pull Requests (Ctrl+t / Alt+3、GitHub モード有効時)",
        Msg::HelpMoveHalfPageInListScrollRight => {
            "一覧を半ページ移動 / 右ペインを半ページスクロール"
        }
        Msg::HelpOpenSelectedPRInDescription => "選択 PR を説明表示で開く",
        Msg::HelpShowDiffRenderedLikeGIT => "差分を表示 (GIT/LOG レーンと同じ見え方)",
        Msg::HelpShowCIStatusUppercaseBecause => {
            "CI ステータスを表示 (s は設定に割り当て済みのため大文字)"
        }
        Msg::HelpJKDiff => "j/k ↑/↓ (diff 表示中)",
        Msg::HelpMoveLineCursor => "行カーソルを移動",
        Msg::HelpDiff => "]/[ (diff 表示中)",
        Msg::HelpGoNextPreviousHunk => "次 / 前の hunk へ",
        Msg::HelpWDiff => "w (diff 表示中)",
        Msg::HelpToggleWrapNotSavedConfig => "折り返し切替 (設定には保存しない)",
        Msg::HelpHLDiff => "h/l ←/→ (diff 表示中)",
        Msg::HelpScrollHorizontally => "水平スクロール",
        Msg::HelpOpenInBrowserGhPr => "ブラウザで開く (gh pr view --web)",
        Msg::HelpCycleStateFilterOpenClosedMergedAll => {
            "state 絞り込みを循環 (open → closed → merged → all)"
        }
        Msg::HelpHugeDiffs => "巨大な diff",
        Msg::HelpTruncatedAtLineByteLimit => "行数/バイト数の上限で打ち切り、notice で通知",
        Msg::HelpMoveUpDown => "上下移動",
        Msg::HelpExpandOpen => "展開 / 開く",
        Msg::HelpCollapseGoParent => "折りたたみ / 親へ",
        Msg::HelpSelectParentCollapse => "親を選択して折りたたむ",
        Msg::HelpOpenToggleExpansion => "開く / 展開切替",
        Msg::HelpRescan => "再走査",
        Msg::HelpMoveLineCursorViewFollows => {
            "行カーソルを移動 (画面はカーソルに追従する。v/e の起点もこの行)"
        }
        Msg::HelpToggleWrap => "折り返し切替",
        Msg::HelpResetHorizontalScroll => "水平スクロールをリセット",
        Msg::HelpGoBackInHistoryBackspace => "履歴を戻る (Backspace も同様)",
        Msg::HelpGoForwardInHistory => "履歴を進む",
        Msg::HelpJumpLineN => "N 行目へジャンプ",
        Msg::HelpSearch => "検索",
        Msg::HelpGoNextPreviousMatch => "次 / 前のマッチへ",
        Msg::HelpEnterEditMode => "編集モードに入る",
        Msg::HelpMouseDrag => "マウス長押し + 移動",
        Msg::HelpCharacterWiseSelectionDraggingPast => {
            "文字単位の範囲選択 (端まで引っ張ると 1 行ずつ送る)"
        }
        Msg::HelpStartCancelLineWiseSelection => {
            "行単位の選択を開始 / 解除 (j/k・Ctrl+d/u・gg/G で伸縮)"
        }
        Msg::HelpCopySelectionClipboard => "選択範囲をクリップボードへコピー",
        Msg::HelpCopyWholeOpenFile => "開いているファイル全体をコピー",
        Msg::HelpClearSelection => "選択を解除",
        Msg::HelpCopyBackend => "コピー手段",
        Msg::HelpPbcopyWlCopyXclipXsel => {
            "pbcopy/wl-copy/xclip/xsel/clip.exe → 無ければ OSC 52 (ssh 越しも可)"
        }
        Msg::HelpLeftPane => "左ペイン",
        Msg::HelpShowsOnlyChangedFilesWith => {
            "変更ファイルのみを階層付きで表示 (入った時点で全展開)"
        }
        Msg::HelpMoveBetweenChangedFiles => "変更ファイル間を移動",
        Msg::HelpExpandShowDiff => "展開 / diff を表示",
        Msg::HelpShowDiffToggleExpansion => "diff を表示 / 展開切替",
        Msg::HelpSpaceLeftPane => "Space (左ペイン)",
        Msg::HelpStageUnstageSelectedFileDirectory => {
            "選択中のファイル/ディレクトリを stage/unstage トグル"
        }
        Msg::HelpJKDiffPane => "j/k ↑/↓ (diff ペイン)",
        Msg::HelpMoveLineCursorSpaceEnter => {
            "行カーソルを移動 (Space/Enter の対象は常にこのカーソル行)"
        }
        Msg::HelpSpaceDiffPane => "Space (diff ペイン)",
        Msg::HelpStageHunkCursorInUnstage => {
            "カーソル行が属する hunk を stage (基準が staged のときは unstage)"
        }
        Msg::HelpEnterDiffPane => "Enter (diff ペイン)",
        Msg::HelpStageUnstageOnlyChangedLines => {
            "カーソル行 (V の選択中はその範囲) の変更行だけを stage/unstage"
        }
        Msg::HelpVDiffPane => "V (diff ペイン)",
        Msg::HelpStartCancelLineWiseSelectionGrowWith => {
            "行単位選択の開始/解除 (j/k で伸縮・Esc で解除)"
        }
        Msg::HelpClickDiffPane => "クリック (diff ペイン)",
        Msg::HelpMoveCursorLine => "その行へカーソルを移動",
        Msg::HelpDiscardChangesInSelectedFile => {
            "選択中のファイル/ディレクトリの変更を破棄 (確認あり・untracked は削除)"
        }
        Msg::HelpStashChangesConfirmedUntrackedFiles => {
            "変更を stash へ退避 (確認あり・untracked も含む)"
        }
        Msg::HelpPopLatestStashConfirmedWorks => {
            "直近の stash を pop (確認あり・GIT レーン以外からも可)"
        }
        Msg::HelpSearchWithinDiffDisabledIn => "diff 内検索 (side-by-side 表示中は無効)",
        Msg::HelpGoNextPreviousMatchAfter => "次 / 前の検索マッチへ (検索確定後)",
        Msg::HelpShowCombinedDiffAllChanged => "全変更ファイルをまとめた diff を表示 (トグル)",
        Msg::HelpGoNextPreviousFileIn => "まとめ diff 内で次 / 前のファイルへ",
        Msg::HelpSwitchDiffBaseHEADStaged => "diff 基準を切替 (HEAD → staged → unstaged)",
        Msg::HelpCommitDoesNotOpenWhen => "コミット (staged が空だと開かない)",
        Msg::HelpAmendLastCommitPrefillsExisting => {
            "amend コミット (既存メッセージをプリフィル・確認あり)"
        }
        Msg::HelpSwitchInlineSideBySide => {
            "inline ⇔ side-by-side 切替 (設定には保存しない・まとめ diff 表示中は無効)"
        }
        Msg::HelpToggleWrapDiffOnlyNot => "折り返し切替 (diff のみ・設定には保存しない)",
        Msg::HelpScrollHorizontallyDiffPane => "水平スクロール (diff ペイン)",
        Msg::HelpResetHorizontalScrollDiffPane => "水平スクロールをリセット (diff ペイン)",
        Msg::HelpFetchPullPushSeeRemote => "fetch / pull / push (下の Remote セクション参照)",
        Msg::HelpRescanAlsoRefetchesGitStatus => "再走査 (git status も取り直す)",
        Msg::HelpTogglePanelLowerHalfVIEW => {
            "表示切替 (VIEW の左ペイン下半分。ツリーと同時に見える)"
        }
        Msg::HelpListRows => "一覧の行",
        Msg::HelpCommitListShortSHARelative => {
            "コミット一覧 (短縮 SHA / 相対日時 / 作者 / 件名。狭い幅では右の列から落とす)"
        }
        Msg::HelpMoveBetweenCommitsDiffDoes => "コミット間を移動 (diff は追従しない)",
        Msg::HelpShowSelectedCommitSDiff => {
            "選択コミットの diff を右ペインに表示 (フォーカスも移る)"
        }
        Msg::HelpGoTopEndWhatLoaded => "先頭 / 読み込み済み末尾へ (末尾で追加取得)",
        Msg::HelpEscList => "Esc (一覧)",
        Msg::HelpClosePanelSameAsL => "パネルを閉じる (L と同じ)",
        Msg::HelpLogJKDiff => "j/k ↑/↓ (diff)",
        Msg::HelpGoNextPreviousHunkDo => "次 / 前の hunk へ (] / [ も同様)",
        Msg::HelpEscDiff => "Esc (diff)",
        Msg::HelpCloseDiffGoBackFile => "diff を閉じてファイル表示へ戻す",
        Msg::HelpMergeCommits => "マージコミット",
        Msg::HelpShowsDiffAgainstFirstParent => {
            "最初の親との diff を表示 (git show の既定は差分なし)"
        }
        Msg::HelpTyping => "文字入力",
        Msg::HelpInsertTextClickMoveCursor => "挿入 (クリックでカーソル移動)",
        Msg::HelpMoveCursor => "カーソル移動",
        Msg::HelpMoveByWordOptionCtrl => "単語単位で移動 (Option+←/→ / Ctrl+←/→ / Alt+b・f も可)",
        Msg::HelpGoLineStartAfterIndent => {
            "行頭 (インデント直後 ⇄ 桁 0) / 行末へ (Cmd+←/→・Ctrl+a/e も可)"
        }
        Msg::HelpGoStartEndDocumentCmd => "文書の先頭 / 末尾へ (Cmd+↑/↓ も可)",
        Msg::HelpSwapCursorLineWithLine => "カーソル行を上 / 下の行と入れ替える",
        Msg::HelpDeletePreviousWordCtrlBackspace => {
            "手前の 1 単語を削除 (Ctrl+Backspace / Ctrl+w も可)"
        }
        Msg::HelpDeleteNextWordCtrlDelete => "先の 1 単語を削除 (Ctrl+Delete も可)",
        Msg::HelpDeleteLineStartCtrlU => "行頭まで削除 (Ctrl+u も可) / Cmd+Delete: 行末まで削除",
        Msg::HelpSave => "保存",
        Msg::HelpUndoRedoCmdZCmd => "undo / redo (Cmd+z / Cmd+Shift+z)",
        Msg::HelpDeleteLine => "行削除",
        Msg::HelpLeaveEditModeConfirmsWhen => "終了 (未保存なら確認。確認中の s で保存して終了)",
        Msg::HelpClick => "クリック",
        Msg::HelpSelectOpenTreeRowFocus => "ツリーの行を選択して開く / ペインをフォーカス",
        Msg::HelpWheel => "ホイール",
        Msg::HelpMoveInTreeScroll => "ツリー移動 / スクロール",
        Msg::HelpDragDivider => "境界をドラッグ",
        Msg::HelpResizeLeftRightPanesSaved => "左右ペインの幅を変更 (離した時点で保存)",
        Msg::HelpConfirmForDestructiveWriteOperations => "Confirm (破壊的・書き込み系操作の確認)",
        Msg::HelpRun => "実行",
        Msg::HelpNEscOther => "n / Esc / それ以外",
        Msg::HelpCancel => "中止",
        Msg::HelpCommitCCOpensFrom => "Commit (c / C、GIT レーンに限らず開ける)",
        Msg::HelpInsertText => "挿入",
        Msg::HelpNewLine => "改行",
        Msg::HelpCommitAmendGoesThroughConfirmation => "確定 (amend は確認オーバーレイを経由)",
        Msg::HelpCloseWhatYouTypedKept => "閉じる (書きかけは下書きとして残り、再度 c/C で復元)",
        Msg::HelpBranchBOpensFromAny => "Branch (b、レーンを問わず開ける)",
        Msg::HelpFuzzyFilterBranchNames => "ブランチ名をファジー絞り込み",
        Msg::HelpSelectCandidateCtrlNReserved => "候補選択 (Ctrl+n は新規作成に予約)",
        Msg::HelpSwitchSelectedBranchRemotesCreate => {
            "選択中のブランチへ切替 (リモートは追跡ブランチを作成)"
        }
        Msg::HelpCreateSwitchBranchWhenQuery => {
            "入力文字列が既存ブランチと不一致なら新規作成して切替"
        }
        Msg::HelpRemoteFPPOpens => "Remote (f / p / P、レーンを問わず開ける)",
        Msg::HelpFetchPruneNoConfirmation => "fetch --prune (確認不要)",
        Msg::HelpPullFfOnlyNoConfirmation => {
            "pull --ff-only (確認不要・ff できないと git のエラーを表示)"
        }
        Msg::HelpPushConfirmedUsesSetUpstream => {
            "push (確認あり。upstream が無ければ --set-upstream origin <branch>)"
        }
        Msg::HelpWhileRunning => "実行中",
        Msg::HelpStatusBarShowsJobName => {
            "ステータスバーにジョブ名を表示。他の操作は継続可能・同じ/別ジョブの多重起動は不可"
        }
        Msg::HelpOnCompletion => "完了後",
        Msg::HelpRefetchesStatusAheadBehindDiff => "status / ahead-behind / 表示中 diff を再取得",
        Msg::HelpFilterByQuery => "クエリを絞り込み",
        Msg::HelpSelectCandidate => "候補選択",
        Msg::HelpDeleteOneCharacter => "一文字削除",
        Msg::HelpOpen => "開く",
        Msg::HelpGrepQuery => {
            "クエリ (部分一致・smart-case・2 文字以上。打鍵が止まると repo 全体を歩き直す)"
        }
        Msg::HelpSelectHit => "ヒット選択",
        Msg::HelpDeleteOneCharacterClearAll => "一文字削除 / 全消去",
        Msg::HelpOpenAtLineSameQuery => {
            "開いてその行へ (同じクエリで / を立てるので n/N が続けて効く)"
        }
        Msg::HelpCloseResultsStayAreStill => "閉じる (結果は残り、次に開いた時にそのまま見える)",
        Msg::HelpTitle => "タイトル",
        Msg::HelpSearchingNFilesScannedTruncated => {
            "searching... / N files scanned / truncated (5000 件で打ち切り) / stale (変更あり)"
        }
        Msg::HelpSearchGotoN => "Search・Goto (/ と :N)",
        Msg::HelpTypeInputGotoTakesDigits => "入力 (Goto は数字のみ)",
        Msg::HelpConfirm => "確定",
        Msg::HelpCancelInput => "キャンセル",
        Msg::HelpTitleScroll => " help  {from}-{to}/{total}  j/k: スクロール  ?: 閉じる ",
        Msg::HelpCurrentScreen => "{title} ← 今の画面",
        // Settings
        Msg::SettingsJKSelectHL => "j/k 選択  h/l/Enter 変更  s/Esc 閉じる",
        // Status
        Msg::StatusWorkspaceHint => "Ctrl+t / Alt+1..3: タブ切替  s: 設定  q: 終了  ?: help",
        Msg::StatusLoadingIssues => "issues 取得中…",
        Msg::StatusLoadingPullRequests => "pull requests 取得中…",
        Msg::StatusIssuesFetchFailed => "issues 取得失敗: {err}  (r: 再取得)",
        Msg::StatusPrsFetchFailed => "pull requests 取得失敗: {err}  (r: 再取得)",
        Msg::StatusConfirm => "{prompt}  y/Enter: 実行  n/Esc: 中止",
        Msg::StatusCommit => "{title}  Enter: 改行  Ctrl+s: 確定  Esc: 閉じる",
        Msg::StatusRemoteJobRunning => "{job} 実行中… (他の操作は続けられます)",
        Msg::StatusEdit => {
            "{line}:{col}  Ctrl+s: save  Ctrl+z/y: undo/redo  Alt+←/→: 単語移動  Esc: exit"
        }
        Msg::StatusGitSearch => {
            "「{query}」 {current}/{total}  n: next  N: prev  Tab: focus  Shift+Tab: mode  ?: help"
        }
        Msg::StatusGitLinesSelected => {
            "{rows} lines selected  Enter: {verb} lines  j/k: 伸縮  Esc: 解除"
        }
        Msg::StatusViewSearch => {
            "「{query}」 {current}/{total}  n: next  N: prev  Tab: focus  q: quit  ?: help"
        }
    }
}
