## 2024-05-23 - Status Bar Notice Indicator

**Learning:** There is a notice message overlay in the application that is drawn over the normal keyboard hint line at the bottom when present (`app.notice`). Currently it just overwrites the hint bar text with `message`, and lacks visual distinction from the normal hints besides color.

**Action:** Update `notice_line` in `src/shell/status_bar.rs` to include a visual indicator (e.g., prefix with an info/warning symbol like `i` or `!`, or Nerd Font icon if supported, but simpler is safer if we want it to work without Nerd Fonts. Actually, let's use standard unicode characters like `✓` for success/info and `⚠` for error).

## $(date +%Y-%m-%d) - Edit Mode Notice Consistency
**Learning:** Edit mode specific notices (`EditState.notice`) previously bypassed the standard `notice_line` styling, resulting in warnings (unsaved changes) and errors (save failed) lacking visual distinction (color and icon) from normal hints.
**Action:** Always ensure nested or state-specific notice mechanisms reuse the top-level standard styling to provide a consistent visual language across all modes.

## 2024-09-13 - Ratatui List Accessibility
**Learning:** Ratatui `List` widgets by default rely solely on background color to indicate selection (`highlight_style`). This is poor accessibility for users with color vision deficiency. Also, adding a `highlight_symbol` shifts text if not configured with `HighlightSpacing::Always`.
**Action:** When adding or updating `List` widgets, consistently use `.highlight_symbol("▎")` and `.highlight_spacing(ratatui::widgets::HighlightSpacing::Always)` alongside `highlight_style` to ensure selection state is accessible and interactions are smooth without layout jank.

## 2024-11-20 - Ratatui Empty States for TUI Lists
**Learning:** Rendering an empty list in a TUI can lead to a confusing blank area (often making the user wonder if the app is frozen or if the search returned no results). The standard ratatui `List` doesn't provide a built-in fallback UI for zero items.
**Action:** When implementing TUI components with search or filter functionality that render lists, always short-circuit the list drawing logic to explicitly render a visual empty state (e.g. `Paragraph::new("no matches").style(Style::default().fg(Color::DarkGray))`) when the items array is empty.
