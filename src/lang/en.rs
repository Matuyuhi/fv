//! 英語の文言表。キーは msg.rs の `Msg`、日本語は ja.rs

use super::Msg;

pub(super) fn text(msg: Msg) -> &'static str {
    match msg {
        // Branch
        Msg::BranchUnsavedChangesSaveBeforeSwitching => "unsaved changes — save before switching",
        Msg::BranchFailedRunGit => "failed to run git",
        Msg::BranchAlreadyExists => "branch \"{name}\" already exists (Enter to switch)",
        Msg::BranchSwitchedStale => {
            "switched to {branch} (closed the open file — it no longer exists)"
        }
        Msg::BranchSwitched => "switched to {branch}",
        // Commit
        Msg::CommitUnsavedChangesSaveBeforeCommitting => "unsaved changes — save before committing",
        Msg::CommitNoStagedChangesSpaceStage => "no staged changes (Space to stage)",
        Msg::CommitTabEnterBodyCtrlCmd => {
            "Tab/Enter: to body  Ctrl/Cmd+s: confirm  Esc: close (keeps draft)"
        }
        Msg::CommitTabSubjectEnterNewlineCtrl => {
            "Tab: to subject  Enter: newline  Ctrl/Cmd+s: confirm  Esc: close (keeps draft)"
        }
        // Git
        Msg::GitFailedRunGit => "failed to run git",
        Msg::GitCanTStageHunkWise => {
            "can't stage hunk-wise while showing the combined diff (A to exit)"
        }
        Msg::GitCanTStageUntrackedFiles => {
            "can't stage untracked files hunk-wise (use Space on the tree instead)"
        }
        Msg::GitFailedApplyHunk => "failed to apply hunk",
        Msg::GitTrySwitchingUnstagedBaseWith => " (try switching to the unstaged base with t)",
        Msg::GitCanTStageLineWise => {
            "can't stage line-wise while showing the combined diff (A to exit)"
        }
        Msg::GitCanTStageLineWiseWhileSide => {
            "can't stage line-wise while side-by-side (v to switch back to inline)"
        }
        Msg::GitCanTStageUntrackedFilesLineWise => {
            "can't stage untracked files line-wise (use Space on the tree instead)"
        }
        Msg::GitCanTStageRenamedFile => {
            "can't stage a renamed file line-wise (use Space to stage the whole file)"
        }
        Msg::GitCanTApplyPartNew => {
            "can't apply part of a new/deleted file this way (use Space to stage by hunk/file)"
        }
        Msg::GitCursorNotOnChangedLine => {
            "cursor is not on a changed line (+/-) (V to select a range)"
        }
        Msg::GitFailedApplyLines => "failed to apply lines",
        Msg::GitUnsavedChangesSaveDiscardBefore => {
            "unsaved changes (save or discard before running this)"
        }
        Msg::GitNUntrackedFilesWillBe => {
            "\n(untracked files will be deleted. this cannot be undone)"
        }
        Msg::GitNothingDiscard => "nothing to discard",
        Msg::GitChangesDiscarded => "changes discarded",
        Msg::GitFailedDiscard => "failed to discard",
        Msg::GitChangesStashed => "changes stashed",
        Msg::GitFailedStashPush => "failed to stash push",
        Msg::GitPopLatestStashNOn => {
            "pop the latest stash?\n(on conflict, the stash is kept and an error is shown)"
        }
        Msg::GitStashRestored => "stash restored",
        Msg::GitFailedPopStashPossiblyConflict => "failed to pop stash (possibly a conflict)",
        Msg::GitNUnsavedEditsDonT => "\n(unsaved edits — don't forget to save)",
        Msg::GitTruncated => "  (truncated)",
        Msg::GitCannotRun => "failed to run git",
        Msg::GitStagedHunk => "{verb}d hunk {ordinal}/{total}",
        Msg::GitStagedLines => "{verb}d {lines} lines",
        Msg::GitDiscardPrompt => "discard {count} change(s)?\n{path}",
        Msg::GitStashPushPrompt => "stash {count} change(s)?\n(untracked files are included)",
        Msg::GitPushPrompt => "push to {target}?",
        // App
        Msg::AppDiffTruncated => "diff too large — truncated (20000 lines / 2MB)",
        Msg::AppLineWiseSelectionIsnT => {
            "line-wise selection isn't available here (exit A / v to switch to inline)"
        }
        Msg::AppUnsavedChangesCtrlSSave => "unsaved changes (Ctrl+s: save / Esc: discard)",
        Msg::AppNotGitRepository => "not a git repository",
        Msg::AppNoSelectionDragVSelect => "no selection (drag or v to select)",
        Msg::AppNoTextCopy => "no text to copy",
        Msg::AppFetchDone => "fetch done",
        Msg::AppRemoteJobFailed => "failed to {job}",
        Msg::AppFetchDoneWith => "fetch done: {message}",
        // Issues
        Msg::IssuesLoadingComments => "loading comments…",
        Msg::IssuesEnterLClickOpenDetail => "Enter / l / click: open detail",
        Msg::IssuesCommentsFetchFailed => "failed to fetch comments: {err}",
        // Prs
        Msg::PrsDiffTruncated => "diff too large — truncated (20000 lines / 2MB)",
        Msg::PrsEnterLClickOpenDescription => {
            "Enter / l / click: open description (d: diff  S: CI)"
        }
        Msg::PrsTruncated => "  (truncated)",
        Msg::PrsLoading => "loading…",
        Msg::PrsFetchFailed => "failed to fetch:\n{err}\n\n(d to retry)",
        // Remote
        Msg::RemoteLoading => "loading…",
        Msg::RemoteListFetchFailed => "failed to fetch:\n{err}\n\n(r to retry)",
        Msg::RemoteDetailFetchFailed => "failed to fetch:\n{err}\n\n(reopen to retry)",
        // Gh
        Msg::GhGitHubModeUnavailableGhNot => {
            "GitHub mode unavailable: gh is not authenticated (gh auth login)"
        }
        Msg::GhGitHubModeUnavailableGhCommand => "GitHub mode unavailable: gh command not found",
        Msg::GhGitHubModeUnavailableOriginNot => {
            "GitHub mode unavailable: origin is not a GitHub repository"
        }
        Msg::GhGhCommandNotFound => "gh command not found",
        Msg::GhFailedRunGh => "failed to run gh",
        // Confirm
        Msg::ConfirmRun => ": run    ",
        Msg::ConfirmCancel => ": cancel",
        // Help
        Msg::HelpQuit => "quit",
        Msg::HelpSwitchLaneVIEWEDITGIT => "switch lane (VIEW → EDIT → GIT)",
        Msg::HelpSwitchFocusTreeViewerLog => {
            "switch focus (Tree → Viewer; Log is inserted only while the commit list is shown)"
        }
        Msg::HelpToggleCommitListPanelVIEW => {
            "toggle the commit list panel (VIEW only, lower half of the left pane)"
        }
        Msg::HelpOpenFinder => "open the finder",
        Msg::HelpOpenWorkspaceWideSearch => "open workspace-wide search",
        Msg::HelpOpenBranchListOverlayGit => "open the branch list overlay (git repos only)",
        Msg::HelpOpenHelp => "open this help",
        Msg::HelpOpenSettings => "open settings",
        Msg::HelpToggleHiddenItems => "toggle hidden items",
        Msg::HelpToggleIgnoredFilesGitignoreIgnore => {
            "toggle ignored files (.gitignore/.ignore/exclude)"
        }
        Msg::HelpShowHiddenItemsOnStartup => "show hidden items on startup",
        Msg::HelpShowIgnoredFilesOnStartup => "show ignored files on startup",
        Msg::HelpStatusBar => "status bar",
        Msg::HelpAlwaysShowsCurrentBranchAhead => {
            "always shows the current branch + ahead/behind (git repos only)"
        }
        Msg::HelpScrollOneLine => "scroll one line",
        Msg::HelpScrollHalfPage => "scroll half a page",
        Msg::HelpGoTopBottom => "go to top / bottom",
        Msg::HelpClose => "close",
        Msg::HelpSectionOrder => "section order",
        Msg::HelpSectionForCurrentScreenComes => {
            "the section for the current screen comes first (the rest keep their defined order)"
        }
        Msg::HelpWorkspaceSection => "Workspace (GitHub mode, off by default)",
        Msg::HelpGoNextTabViewerIssues => "go to the next tab (viewer → issues → pull requests)",
        Msg::HelpJumpStraightViewerIssuesPull => "jump straight to viewer / issues / pull requests",
        Msg::HelpClickTab => "click a tab",
        Msg::HelpSwitchTab => "switch to that tab",
        Msg::HelpEnableForRunOnlyNot => "enable for this run only (not saved to the config)",
        Msg::HelpGithubTabsInSettings => "github tabs in settings",
        Msg::HelpToggleEnablePersistConfig => "toggle to enable and persist it to the config",
        Msg::HelpIssuesSection => "Issues (Ctrl+t / Alt+2, when GitHub mode is on)",
        Msg::HelpMoveThroughList => "move through the list",
        Msg::HelpMoveHalfPageInList => {
            "move half a page in the list / scroll the detail half a page"
        }
        Msg::HelpSwitchFocusBetweenListDetail => "switch focus between list and detail",
        Msg::HelpEnterLClick => "Enter / l / click",
        Msg::HelpLoadSelectedIssueSDetail => "load the selected issue's detail on the right",
        Msg::HelpOpenInBrowserGhIssue => "open in the browser (gh issue view --web)",
        Msg::HelpRefetchListSwitchingTabsDoes => {
            "refetch the list (switching tabs does not refetch)"
        }
        Msg::HelpFuzzyFilterListOnlyWhile => {
            "fuzzy filter the list (only while the list has focus)"
        }
        Msg::HelpCycleStateFilterOpenClosed => "cycle the state filter (open → closed → all)",
        Msg::HelpPrsSection => "Pull Requests (Ctrl+t / Alt+3, when GitHub mode is on)",
        Msg::HelpMoveHalfPageInListScrollRight => {
            "move half a page in the list / scroll the right pane half a page"
        }
        Msg::HelpOpenSelectedPRInDescription => "open the selected PR in the description view",
        Msg::HelpShowDiffRenderedLikeGIT => "show the diff (rendered like the GIT/LOG lanes)",
        Msg::HelpShowCIStatusUppercaseBecause => {
            "show CI status (uppercase because s is taken by settings)"
        }
        Msg::HelpJKDiff => "j/k ↑/↓ (diff)",
        Msg::HelpMoveLineCursor => "move the line cursor",
        Msg::HelpDiff => "]/[ (diff)",
        Msg::HelpGoNextPreviousHunk => "go to the next / previous hunk",
        Msg::HelpWDiff => "w (diff)",
        Msg::HelpToggleWrapNotSavedConfig => "toggle wrap (not saved to the config)",
        Msg::HelpHLDiff => "h/l ←/→ (diff)",
        Msg::HelpScrollHorizontally => "scroll horizontally",
        Msg::HelpOpenInBrowserGhPr => "open in the browser (gh pr view --web)",
        Msg::HelpCycleStateFilterOpenClosedMergedAll => {
            "cycle the state filter (open → closed → merged → all)"
        }
        Msg::HelpHugeDiffs => "huge diffs",
        Msg::HelpTruncatedAtLineByteLimit => {
            "truncated at the line/byte limit, reported with a notice"
        }
        Msg::HelpMoveUpDown => "move up / down",
        Msg::HelpExpandOpen => "expand / open",
        Msg::HelpCollapseGoParent => "collapse / go to parent",
        Msg::HelpSelectParentCollapse => "select the parent and collapse it",
        Msg::HelpOpenToggleExpansion => "open / toggle expansion",
        Msg::HelpRescan => "rescan",
        Msg::HelpMoveLineCursorViewFollows => {
            "move the line cursor (the view follows it; v/e start from this line)"
        }
        Msg::HelpToggleWrap => "toggle wrap",
        Msg::HelpResetHorizontalScroll => "reset horizontal scroll",
        Msg::HelpGoBackInHistoryBackspace => "go back in history (Backspace does the same)",
        Msg::HelpGoForwardInHistory => "go forward in history",
        Msg::HelpJumpLineN => "jump to line N",
        Msg::HelpSearch => "search",
        Msg::HelpGoNextPreviousMatch => "go to the next / previous match",
        Msg::HelpEnterEditMode => "enter edit mode",
        Msg::HelpMouseDrag => "mouse drag",
        Msg::HelpCharacterWiseSelectionDraggingPast => {
            "character-wise selection (dragging past an edge scrolls a line at a time)"
        }
        Msg::HelpStartCancelLineWiseSelection => {
            "start / cancel a line-wise selection (grow it with j/k, Ctrl+d/u, gg/G)"
        }
        Msg::HelpCopySelectionClipboard => "copy the selection to the clipboard",
        Msg::HelpCopyWholeOpenFile => "copy the whole open file",
        Msg::HelpClearSelection => "clear the selection",
        Msg::HelpCopyBackend => "copy backend",
        Msg::HelpPbcopyWlCopyXclipXsel => {
            "pbcopy/wl-copy/xclip/xsel/clip.exe, falling back to OSC 52 (works over ssh)"
        }
        Msg::HelpLeftPane => "left pane",
        Msg::HelpShowsOnlyChangedFilesWith => {
            "shows only changed files with their hierarchy (fully expanded on entry)"
        }
        Msg::HelpMoveBetweenChangedFiles => "move between changed files",
        Msg::HelpExpandShowDiff => "expand / show the diff",
        Msg::HelpShowDiffToggleExpansion => "show the diff / toggle expansion",
        Msg::HelpSpaceLeftPane => "Space (left pane)",
        Msg::HelpStageUnstageSelectedFileDirectory => {
            "stage/unstage the selected file or directory"
        }
        Msg::HelpJKDiffPane => "j/k ↑/↓ (diff pane)",
        Msg::HelpMoveLineCursorSpaceEnter => {
            "move the line cursor (Space/Enter always act on this line)"
        }
        Msg::HelpSpaceDiffPane => "Space (diff pane)",
        Msg::HelpStageHunkCursorInUnstage => {
            "stage the hunk the cursor is in (unstage when the base is staged)"
        }
        Msg::HelpEnterDiffPane => "Enter (diff pane)",
        Msg::HelpStageUnstageOnlyChangedLines => {
            "stage/unstage only the changed lines under the cursor (or the V selection)"
        }
        Msg::HelpVDiffPane => "V (diff pane)",
        Msg::HelpStartCancelLineWiseSelectionGrowWith => {
            "start / cancel a line-wise selection (grow it with j/k, cancel with Esc)"
        }
        Msg::HelpClickDiffPane => "click (diff pane)",
        Msg::HelpMoveCursorLine => "move the cursor to that line",
        Msg::HelpDiscardChangesInSelectedFile => {
            "discard changes in the selected file or directory (confirmed; untracked files are deleted)"
        }
        Msg::HelpStashChangesConfirmedUntrackedFiles => {
            "stash the changes (confirmed; untracked files included)"
        }
        Msg::HelpPopLatestStashConfirmedWorks => {
            "pop the latest stash (confirmed; works outside the GIT lane too)"
        }
        Msg::HelpSearchWithinDiffDisabledIn => {
            "search within the diff (disabled in side-by-side view)"
        }
        Msg::HelpGoNextPreviousMatchAfter => {
            "go to the next / previous match (after a search is committed)"
        }
        Msg::HelpShowCombinedDiffAllChanged => "show a combined diff of all changed files (toggle)",
        Msg::HelpGoNextPreviousFileIn => "go to the next / previous file in the combined diff",
        Msg::HelpSwitchDiffBaseHEADStaged => "switch the diff base (HEAD → staged → unstaged)",
        Msg::HelpCommitDoesNotOpenWhen => "commit (does not open when nothing is staged)",
        Msg::HelpAmendLastCommitPrefillsExisting => {
            "amend the last commit (prefills the existing message, confirmed)"
        }
        Msg::HelpSwitchInlineSideBySide => {
            "switch inline ⇔ side-by-side (not saved to the config; disabled in the combined diff)"
        }
        Msg::HelpToggleWrapDiffOnlyNot => "toggle wrap (diff only, not saved to the config)",
        Msg::HelpScrollHorizontallyDiffPane => "scroll horizontally (diff pane)",
        Msg::HelpResetHorizontalScrollDiffPane => "reset horizontal scroll (diff pane)",
        Msg::HelpFetchPullPushSeeRemote => "fetch / pull / push (see the Remote section below)",
        Msg::HelpRescanAlsoRefetchesGitStatus => "rescan (also refetches git status)",
        Msg::HelpTogglePanelLowerHalfVIEW => {
            "toggle the panel (lower half of VIEW's left pane, shown alongside the tree)"
        }
        Msg::HelpListRows => "list rows",
        Msg::HelpCommitListShortSHARelative => {
            "commit list (short SHA / relative date / author / subject; narrow widths drop the right columns first)"
        }
        Msg::HelpMoveBetweenCommitsDiffDoes => "move between commits (the diff does not follow)",
        Msg::HelpShowSelectedCommitSDiff => {
            "show the selected commit's diff in the right pane (focus moves too)"
        }
        Msg::HelpGoTopEndWhatLoaded => {
            "go to the top / end of what is loaded (fetches more at the end)"
        }
        Msg::HelpEscList => "Esc (list)",
        Msg::HelpClosePanelSameAsL => "close the panel (same as L)",
        Msg::HelpLogJKDiff => "j/k ↑/↓ (diff)",
        Msg::HelpGoNextPreviousHunkDo => "go to the next / previous hunk (] / [ do the same)",
        Msg::HelpEscDiff => "Esc (diff)",
        Msg::HelpCloseDiffGoBackFile => "close the diff and go back to the file view",
        Msg::HelpMergeCommits => "merge commits",
        Msg::HelpShowsDiffAgainstFirstParent => {
            "shows the diff against the first parent (git show shows none by default)"
        }
        Msg::HelpTyping => "typing",
        Msg::HelpInsertTextClickMoveCursor => "insert text (click to move the cursor)",
        Msg::HelpMoveCursor => "move the cursor",
        Msg::HelpMoveByWordOptionCtrl => "move by word (Option+←/→, Ctrl+←/→, Alt+b/f also work)",
        Msg::HelpGoLineStartAfterIndent => {
            "go to line start (after the indent ⇄ column 0) / line end (Cmd+←/→, Ctrl+a/e also work)"
        }
        Msg::HelpGoStartEndDocumentCmd => {
            "go to the start / end of the document (Cmd+↑/↓ also works)"
        }
        Msg::HelpSwapCursorLineWithLine => "swap the cursor line with the line above / below",
        Msg::HelpDeletePreviousWordCtrlBackspace => {
            "delete the previous word (Ctrl+Backspace, Ctrl+w also work)"
        }
        Msg::HelpDeleteNextWordCtrlDelete => "delete the next word (Ctrl+Delete also works)",
        Msg::HelpDeleteLineStartCtrlU => {
            "delete to the line start (Ctrl+u also works) / Cmd+Delete: delete to the line end"
        }
        Msg::HelpSave => "save",
        Msg::HelpUndoRedoCmdZCmd => "undo / redo (Cmd+z / Cmd+Shift+z)",
        Msg::HelpDeleteLine => "delete the line",
        Msg::HelpLeaveEditModeConfirmsWhen => {
            "leave edit mode (confirms when unsaved; s in the confirmation saves and leaves)"
        }
        Msg::HelpClick => "click",
        Msg::HelpSelectOpenTreeRowFocus => "select and open a tree row / focus a pane",
        Msg::HelpWheel => "wheel",
        Msg::HelpMoveInTreeScroll => "move in the tree / scroll",
        Msg::HelpDragDivider => "drag the divider",
        Msg::HelpResizeLeftRightPanesSaved => "resize the left/right panes (saved on release)",
        Msg::HelpConfirmForDestructiveWriteOperations => {
            "Confirm (for destructive and write operations)"
        }
        Msg::HelpRun => "run it",
        Msg::HelpNEscOther => "n / Esc / other",
        Msg::HelpCancel => "cancel",
        Msg::HelpCommitCCOpensFrom => "Commit (c / C, opens from any lane)",
        Msg::HelpInsertText => "insert text",
        Msg::HelpNewLine => "new line",
        Msg::HelpCommitAmendGoesThroughConfirmation => {
            "commit (amend goes through the confirmation overlay)"
        }
        Msg::HelpCloseWhatYouTypedKept => {
            "close (what you typed is kept as a draft and restored on the next c/C)"
        }
        Msg::HelpBranchBOpensFromAny => "Branch (b, opens from any lane)",
        Msg::HelpFuzzyFilterBranchNames => "fuzzy filter branch names",
        Msg::HelpSelectCandidateCtrlNReserved => {
            "select a candidate (Ctrl+n is reserved for creating one)"
        }
        Msg::HelpSwitchSelectedBranchRemotesCreate => {
            "switch to the selected branch (remotes create a tracking branch)"
        }
        Msg::HelpCreateSwitchBranchWhenQuery => {
            "create and switch to a branch when the query matches no existing one"
        }
        Msg::HelpRemoteFPPOpens => "Remote (f / p / P, opens from any lane)",
        Msg::HelpFetchPruneNoConfirmation => "fetch --prune (no confirmation)",
        Msg::HelpPullFfOnlyNoConfirmation => {
            "pull --ff-only (no confirmation; shows git's error when it cannot fast-forward)"
        }
        Msg::HelpPushConfirmedUsesSetUpstream => {
            "push (confirmed; uses --set-upstream origin <branch> when there is no upstream)"
        }
        Msg::HelpWhileRunning => "while running",
        Msg::HelpStatusBarShowsJobName => {
            "the status bar shows the job name; you can keep working, but no second job can start"
        }
        Msg::HelpOnCompletion => "on completion",
        Msg::HelpRefetchesStatusAheadBehindDiff => {
            "refetches status, ahead/behind and the diff on screen"
        }
        Msg::HelpFilterByQuery => "filter by query",
        Msg::HelpSelectCandidate => "select a candidate",
        Msg::HelpDeleteOneCharacter => "delete one character",
        Msg::HelpOpen => "open",
        Msg::HelpGrepQuery => {
            "the query (substring, smart-case, 2+ characters; walks the repo again once you stop typing)"
        }
        Msg::HelpSelectHit => "select a hit",
        Msg::HelpDeleteOneCharacterClearAll => "delete one character / clear all",
        Msg::HelpOpenAtLineSameQuery => {
            "open at that line (the same query is set for /, so n/N keep working)"
        }
        Msg::HelpCloseResultsStayAreStill => {
            "close (the results stay and are still there next time you open it)"
        }
        Msg::HelpTitle => "title",
        Msg::HelpSearchingNFilesScannedTruncated => {
            "searching... / N files scanned / truncated (cut off at 5000 hits) / stale (files changed)"
        }
        Msg::HelpTypeInputGotoTakesDigits => "type the input (Goto takes digits only)",
        Msg::HelpConfirm => "confirm",
        Msg::HelpCancelInput => "cancel",
        Msg::HelpTitleScroll => " help  {from}-{to}/{total}  j/k: scroll  ?: close ",
        Msg::HelpCurrentScreen => "{title} ← current screen",
        // Settings
        Msg::SettingsJKSelectHL => "j/k select  h/l/Enter change  s/Esc close",
        // Status
        Msg::StatusWorkspaceHint => "Ctrl+t / Alt+1..3: switch tab  s: settings  q: quit  ?: help",
        Msg::StatusLoadingIssues => "loading issues…",
        Msg::StatusLoadingPullRequests => "loading pull requests…",
        Msg::StatusIssuesFetchFailed => "failed to fetch issues: {err}  (r: retry)",
        Msg::StatusPrsFetchFailed => "failed to fetch pull requests: {err}  (r: retry)",
        Msg::StatusConfirm => "{prompt}  y/Enter: run  n/Esc: cancel",
        Msg::StatusCommit => "{title}  Enter: newline  Ctrl+s: confirm  Esc: close",
        Msg::StatusRemoteJobRunning => "{job} running… (other operations still work)",
        Msg::StatusEdit => {
            "{line}:{col}  Ctrl+s: save  Ctrl+z/y: undo/redo  Alt+←/→: word move  Esc: exit"
        }
        Msg::StatusGitSearch => {
            "\"{query}\" {current}/{total}  n: next  N: prev  Tab: focus  Shift+Tab: mode  ?: help"
        }
        Msg::StatusGitLinesSelected => {
            "{rows} lines selected  Enter: {verb} lines  j/k: resize  Esc: clear"
        }
        Msg::StatusViewSearch => {
            "\"{query}\" {current}/{total}  n: next  N: prev  Tab: focus  q: quit  ?: help"
        }
        // File ops
        Msg::FileCannotRenameNonUTF8 => "cannot rename a non-UTF-8 file name",
        Msg::FileRename => "rename: ",
        Msg::FileRefusingSymlinkEscape => "refusing to write outside the tree through a symlink",
        Msg::FileEmptyName => "empty name",
        Msg::FileOnlyRelativeName => "only a relative name is allowed (no .. or absolute paths)",
        Msg::FileRenameBareNameOnly => {
            "rename takes a bare name (it cannot move to another directory)"
        }
        Msg::FileDeleteDirPrompt => {
            "delete this directory? (everything inside is removed)\n{shown}\n(this cannot be undone)"
        }
        Msg::FileDeleteFilePrompt => "delete this file?\n{shown}\n(this cannot be undone)",
        Msg::FileDeleted => "deleted: {shown}",
        Msg::FileDeleteFailed => "delete failed: {shown}: {e}",
        Msg::FileNewFilePrompt => "new file {dir}",
        Msg::FileNewDirPrompt => "new dir {dir}",
        Msg::FileAlreadyExists => "already exists: {shown}",
        Msg::FileCreated => "created: {shown}",
        Msg::FileCreateFailed => "create failed: {shown}: {e}",
        Msg::FileAlreadyExistsTo => "already exists: {shown_to}",
        Msg::FileRenamed => "renamed: {shown_from} → {shown_to}",
        Msg::FileRenameFailed => "rename failed: {shown_from}: {e}",
        Msg::HelpNewFileUnderSelectedDirectory => {
            "new file (under the selected directory; a/b.rs creates intermediate dirs)"
        }
        Msg::HelpNewDirectory => "new directory",
        Msg::HelpRenameParentStaysSame => "rename (parent stays the same)",
        Msg::HelpDeleteWithConfirmationDirectoryGoes => {
            "delete (with confirmation; a directory goes with its contents)"
        }
        Msg::HelpCopyRelativePathClipboard => "copy the relative path to the clipboard",
        Msg::HelpSearchGotoFileOpInput => "Search / Goto / file-op input (/ and :N and n/N/R)",
    }
}
