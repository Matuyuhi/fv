## 2024-05-18 - Avoid unnecessary .replace() allocations
**Learning:** `str::replace()` always allocates or at least performs slower search even if the string doesn't contain the pattern being replaced. While replacing `String` with `Cow` requires a larger refactor in ratatui `Span` usage, in hot loops pre-checking with `str::contains()` before calling `replace()` provides measurable speedups vs blind replacement.
**Action:** When performing `str::replace` in rendering paths (like `text::normalize` called on every visible string segment), check for presence with `.contains()` first if the pattern is rarely expected (e.g. tab characters in typical code lines).
## 2024-05-30 - Faster Tree Rendering
**Learning:** `format!("{}{}", "  ".repeat(n), marker)` is surprisingly slow in tight loops due to multiple allocations and `format!` overhead. A pre-allocated `String` with `push_str` is significantly faster. Also, initializing a vector with a single element using `vec![]` and then pushing to it later might cause reallocation; `Vec::with_capacity` is preferred.
**Action:** Use pre-allocated `String` and `push_str` instead of `format!` and `.repeat()` in hot paths like rendering loops.
