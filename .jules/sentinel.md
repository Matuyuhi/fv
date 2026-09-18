## 2025-02-14 - Argument Injection in Git Commands
**Vulnerability:** Argument injection vulnerability in `git switch` and `git push` commands.
**Learning:** External command calls were passing branch names without using the end-of-options delimiter (`--`). A branch name like `-foo` would be misinterpreted as an option by git, leading to failures or potentially executing unintended options.
**Prevention:** Always use `--` before passing user-controlled or external string parameters to external processes when those parameters are meant to be arguments, unless the specific sub-command restricts the usage of `--` (e.g. `git switch -c`).
