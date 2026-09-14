## 2024-05-23 - Status Bar Notice Indicator

**Learning:** There is a notice message overlay in the application that is drawn over the normal keyboard hint line at the bottom when present (`app.notice`). Currently it just overwrites the hint bar text with `message`, and lacks visual distinction from the normal hints besides color.

**Action:** Update `notice_line` in `src/shell/status_bar.rs` to include a visual indicator (e.g., prefix with an info/warning symbol like `i` or `!`, or Nerd Font icon if supported, but simpler is safer if we want it to work without Nerd Fonts. Actually, let's use standard unicode characters like `✓` for success/info and `⚠` for error).

## $(date +%Y-%m-%d) - Edit Mode Notice Consistency
**Learning:** Edit mode specific notices (`EditState.notice`) previously bypassed the standard `notice_line` styling, resulting in warnings (unsaved changes) and errors (save failed) lacking visual distinction (color and icon) from normal hints.
**Action:** Always ensure nested or state-specific notice mechanisms reuse the top-level standard styling to provide a consistent visual language across all modes.

## 2024-09-14 - List Widget Selection Accessibility
**Learning:** For UI accessibility and consistent styling, Ratatui `List` widgets should use `.highlight_symbol("▎")` and `.highlight_spacing(ratatui::widgets::HighlightSpacing::Always)` rather than relying solely on background color for their selection state.
**Action:** When adding or updating `List::new` widgets, always configure a distinct highlight symbol and spacing to ensure selected items are accessible.
