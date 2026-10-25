## 2025-02-14 - Argument Injection in Git Commands
**Vulnerability:** Argument injection vulnerability in `git switch` and `git push` commands.
**Learning:** External command calls were passing branch names without using the end-of-options delimiter (`--`). A branch name like `-foo` would be misinterpreted as an option by git, leading to failures or potentially executing unintended options.
**Prevention:** Always use `--` before passing user-controlled or external string parameters to external processes when those parameters are meant to be arguments, unless the specific sub-command restricts the usage of `--` (e.g. `git switch -c`).
## 2025-02-14 - Sensitive Config File Permissions
**Vulnerability:** Weak file permissions on configuration files.
**Learning:** `std::fs::write` creates files with default permissions (often 0644), which can allow other users on a shared machine to read potentially sensitive configuration data (such as themes, layout states, and potentially in the future, tokens or other sensitive keys).
**Prevention:** For configuration or log files intended only for the application owner, use `std::fs::OpenOptions` and `std::os::unix::fs::OpenOptionsExt` to explicitly set secure file permissions (e.g., `0o600`).
