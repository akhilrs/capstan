What `cstan recent --json` prints, with the project id, epoch and times replaced by fixed values (`tests/recent.rs`
checks them and rewrites them when `CSTAN_WRITE_FIXTURES` is set). `unknown_command.json` and `old_usage.json` come from
a fake controller. CapstanKit's `RecentCallTests` read these files.
