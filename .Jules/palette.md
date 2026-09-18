## 2024-05-23 - Status Bar Notice Indicator

**Learning:** There is a notice message overlay in the application that is drawn over the normal keyboard hint line at the bottom when present (`app.notice`). Currently it just overwrites the hint bar text with `message`, and lacks visual distinction from the normal hints besides color.

**Action:** Update `notice_line` in `src/shell/status_bar.rs` to include a visual indicator (e.g., prefix with an info/warning symbol like `i` or `!`, or Nerd Font icon if supported, but simpler is safer if we want it to work without Nerd Fonts. Actually, let's use standard unicode characters like `✓` for success/info and `⚠` for error).

## $(date +%Y-%m-%d) - Edit Mode Notice Consistency
**Learning:** Edit mode specific notices (`EditState.notice`) previously bypassed the standard `notice_line` styling, resulting in warnings (unsaved changes) and errors (save failed) lacking visual distinction (color and icon) from normal hints.
**Action:** Always ensure nested or state-specific notice mechanisms reuse the top-level standard styling to provide a consistent visual language across all modes.

## $(date +%Y-%m-%d) - TUI List Selection Accessibility
**Learning:** In terminal UIs, relying solely on background color `.bg()` for list selection state is an accessibility anti-pattern, especially on terminals with varying color palettes or low contrast.
**Action:** Always use `.highlight_symbol("\u{258e}")` combined with `.highlight_spacing(ratatui::widgets::HighlightSpacing::Always)` for Ratatui `List` widgets. This adds a solid vertical bar (Half Block) to the left of the selected item while keeping layout stable, providing a clear structural indicator alongside color changes.
