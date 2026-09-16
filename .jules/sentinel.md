## 2024-05-24 - Command Argument Injection in git switch
**Vulnerability:** External command execution via `Command::new` in `git switch` did not properly separate options from arguments using `--`. A maliciously crafted branch name (e.g., `-foo`) could inject options into the git command.
**Learning:** This repo builds git commands using user-provided names (like branch names). Without the POSIX end-of-options delimiter (`--`), an argument beginning with `-` is treated as a switch, causing unexpected behavior or security issues.
**Prevention:** To prevent argument injection in external command executions (like `git switch`), always use the POSIX end-of-options delimiter (`--`) before user-provided arguments (e.g., branch names) in `Command::new`.
