## 2025-02-14 - Argument Injection in Git Commands
**Vulnerability:** Argument injection vulnerability in `git switch` and `git push` commands.
**Learning:** External command calls were passing branch names without using the end-of-options delimiter (`--`). A branch name like `-foo` would be misinterpreted as an option by git, leading to failures or potentially executing unintended options.
**Prevention:** Always use `--` before passing user-controlled or external string parameters to external processes when those parameters are meant to be arguments, unless the specific sub-command restricts the usage of `--` (e.g. `git switch -c`).
## 2025-02-14 - Argument Injection via git switch -c
**Vulnerability:** Argument injection vulnerability in `git switch -c` command.
**Learning:** For git commands where the end-of-options delimiter (`--`) cannot be used (like `git switch -c` which misinterprets it as the literal branch name), passing an unvalidated branch name directly allows argument injection if the name starts with a hyphen (e.g., `-foo`).
**Prevention:** If `--` cannot be used to separate options from arguments, strictly validate the user input before passing it to external commands, such as ensuring the string does not start with a hyphen.
