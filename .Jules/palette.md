## 2024-05-23 - Status Bar Notice Indicator

**Learning:** There is a notice message overlay in the application that is drawn over the normal keyboard hint line at the bottom when present (`app.notice`). Currently it just overwrites the hint bar text with `message`, and lacks visual distinction from the normal hints besides color.

**Action:** Update `notice_line` in `src/shell/status_bar.rs` to include a visual indicator (e.g., prefix with an info/warning symbol like `i` or `!`, or Nerd Font icon if supported, but simpler is safer if we want it to work without Nerd Fonts. Actually, let's use standard unicode characters like `✓` for success/info and `⚠` for error).

## $(date +%Y-%m-%d) - Edit Mode Notice Consistency
**Learning:** Edit mode specific notices (`EditState.notice`) previously bypassed the standard `notice_line` styling, resulting in warnings (unsaved changes) and errors (save failed) lacking visual distinction (color and icon) from normal hints.
**Action:** Always ensure nested or state-specific notice mechanisms reuse the top-level standard styling to provide a consistent visual language across all modes.

## 2026-09-11 - Confirmation Dialog UI & Status Bar UX
**Learning:** Destructive operations display a multi-line prompt that gets unreadably squished when condensed to a single-line status bar. Additionally, dialogs without visual center alignment or prominent localized warning titles fail to draw sufficient user attention to destructive operations.
**Action:** Always replace newlines with spaces when rendering multi-line dialog prompts in a single-line context like a status bar. Furthermore, align pop-up dialog prompts to the center and augment titles with warning indicators to emphasize their importance and maintain visual hierarchy.
