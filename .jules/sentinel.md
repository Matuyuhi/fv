## 2025-02-14 - Argument Injection in Git Commands
**Vulnerability:** Argument injection vulnerability in `git switch` and `git push` commands.
**Learning:** External command calls were passing branch names without using the end-of-options delimiter (`--`). A branch name like `-foo` would be misinterpreted as an option by git, leading to failures or potentially executing unintended options.
**Prevention:** Always use `--` before passing user-controlled or external string parameters to external processes when those parameters are meant to be arguments, unless the specific sub-command restricts the usage of `--` (e.g. `git switch -c`).
## 2025-02-28 - Secure Configuration File Permissions
**Vulnerability:** Application configuration files created with `std::fs::write` were assigned overly permissive default permissions. This could potentially expose sensitive settings or tokens stored in the configuration file to other unauthorized users on the same system.
**Learning:** Functions like `std::fs::write` lack mechanisms for setting explicit file permissions upon creation, leading to environments applying their default broad permissions (usually influenced by `umask`).
**Prevention:** For sensitive configuration files, prefer using `std::fs::OpenOptions` along with `std::os::unix::fs::OpenOptionsExt` to explicitly set file modes (e.g., `options.mode(0o600)`) on Unix platforms, ensuring the file is exclusively accessible by the owner immediately upon creation.
