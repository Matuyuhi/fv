## 2024-05-19 - Added Empty State for File Tree
**Learning:** `ratatui` UI elements (like `List`) lack built-in empty states out-of-the-box. When filtering yields 0 files or a directory is completely empty, it shows nothing, appearing broken.
**Action:** Always check the item count before rendering iterative components in `ratatui`. Provide an explicit fallback `Paragraph` UI (e.g. `(empty)`) to give explicit feedback to the user.
