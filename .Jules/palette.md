## 2024-10-27 - Added keyboard shortcut hints to tab bar
**Learning:** Adding numeric prefixes to tab labels (e.g., "1: Viewer") is a simple but highly effective way to teach users keyboard shortcuts (Alt+1, Alt+2, etc.) in a TUI environment where tooltips aren't natively available.
**Action:** Look for other areas where keyboard shortcuts exist but aren't visually discoverable, and consider adding inline hints to labels or status bars.

## 2024-10-31 - Improved empty states
**Learning:** Generic text like `(empty)` or `no file selected` is unhelpful in a TUI. Providing an actionable hint, and centering it within the pane, makes the UI much more approachable and gives users clear next steps without taking up extra space.
**Action:** When finding generic empty states, update them to include a brief, actionable instruction and center them visually.