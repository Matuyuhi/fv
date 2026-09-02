## 2024-05-17 - TUI Vertical Centering of Empty States
**Learning:** Ratatui's `Paragraph` widget with `Alignment::Center` only centers horizontally. To achieve vertical centering (e.g., for empty state messages like "No files found"), manual padding with newlines must be calculated based on the available block area's height.
**Action:** When implementing empty states in TUI apps using ratatui, always calculate and inject vertical padding to center the message both horizontally and vertically, making the interface feel more polished.
