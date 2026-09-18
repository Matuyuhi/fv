## 2024-05-19 - [Command Injection via Git Branch Names]
**Vulnerability:** Argument injection via branch names passed to `git switch`, `git switch --track`, and `git push` which could be treated as command-line options if they started with `-` (e.g. `--force`).
**Learning:** External processes executed with `Command::new` can misinterpret dynamic string parameters as options instead of positional arguments if the arguments aren't explicitly delimited by `--`.
**Prevention:** Always use the POSIX end-of-options delimiter (`--`) before user-provided arguments in `Command::new`. Exception: `git switch -c <name>` implicitly consumes the next argument, so no `--` is needed.
