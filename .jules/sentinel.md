## 2025-02-14 - Argument Injection in Git Commands
**Vulnerability:** Argument injection vulnerability in `git switch` and `git push` commands.
**Learning:** External command calls were passing branch names without using the end-of-options delimiter (`--`). A branch name like `-foo` would be misinterpreted as an option by git, leading to failures or potentially executing unintended options.
**Prevention:** Always use `--` before passing user-controlled or external string parameters to external processes when those parameters are meant to be arguments, unless the specific sub-command restricts the usage of `--` (e.g. `git switch -c`).
## 2025-03-01 - Command/Option Injection in git switch -c
**Vulnerability:** A branch name starting with a hyphen (e.g., `-test`) passed to `git switch -c` can be interpreted as Git command-line options instead of a branch name.
**Learning:** Git commands executed via `std::process::Command` can suffer from option injection if user input starting with `-` is passed immediately after command-line flags. While `--` can be used to separate options from positional arguments in most Git commands, `git switch -c` incorrectly handles `--` before the branch name (it throws `error: option '--track' expects "direct" or "inherit"`).
**Prevention:** For `git switch -c`, since `--` cannot be used safely, validate user-provided branch names to ensure they do not start with a hyphen (`-`) before execution.
