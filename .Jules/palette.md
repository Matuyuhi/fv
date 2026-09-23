## 2024-05-23 - Status Bar Notice Indicator

**Learning:** There is a notice message overlay in the application that is drawn over the normal keyboard hint line at the bottom when present (`app.notice`). Currently it just overwrites the hint bar text with `message`, and lacks visual distinction from the normal hints besides color.

**Action:** Update `notice_line` in `src/shell/status_bar.rs` to include a visual indicator (e.g., prefix with an info/warning symbol like `i` or `!`, or Nerd Font icon if supported, but simpler is safer if we want it to work without Nerd Fonts. Actually, let's use standard unicode characters like `✓` for success/info and `⚠` for error).

## $(date +%Y-%m-%d) - Edit Mode Notice Consistency
**Learning:** Edit mode specific notices (`EditState.notice`) previously bypassed the standard `notice_line` styling, resulting in warnings (unsaved changes) and errors (save failed) lacking visual distinction (color and icon) from normal hints.
**Action:** Always ensure nested or state-specific notice mechanisms reuse the top-level standard styling to provide a consistent visual language across all modes.

## 2024-09-13 - Ratatui List Accessibility
**Learning:** Ratatui `List` widgets by default rely solely on background color to indicate selection (`highlight_style`). This is poor accessibility for users with color vision deficiency. Also, adding a `highlight_symbol` shifts text if not configured with `HighlightSpacing::Always`.
**Action:** When adding or updating `List` widgets, consistently use `.highlight_symbol("▎")` and `.highlight_spacing(ratatui::widgets::HighlightSpacing::Always)` alongside `highlight_style` to ensure selection state is accessible and interactions are smooth without layout jank.

## 2024-11-20 - Empty States in Search/Filter Components
**Learning:** TUI components with search or filter functionalities (like branch search or file finder) without a distinct visual empty state (e.g., rendering a blank list area) can leave the user confused as to whether the component is still searching or if there are truly no matches.
**Action:** When implementing TUI components with search or filter functionality, always provide a clear visual empty state (e.g., rendering a dark gray `Paragraph` displaying 'no matches') when there are zero results, instead of rendering a completely blank list area. This matches existing conventions found in the `grep` and `remotelist` components.
