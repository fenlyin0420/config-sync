---
name: harvest-sync-warnings-verbose-only
description: harvest/sync CLI 默认不逐条打印 warning，只输出隐藏计数行，需 -v/--verbose 才原位显示
type: project
---

该 CLI 的 `warn!` 宏不再直接 `eprintln!`，而是走 `ui::warning`；`ui` 提供 `verbose()` / `warning()` / `hidden()`，默认只累积计数，退出前由 `main` 用 `ui::hidden()` 打一行 `[sync] warning: N warnings held back -- -v shows them where they happen`。`-v`/`--verbose`（在 `parse` 里）时每条 warning 在发生处原位打印，且不再有收尾计数行。`fail!` / `refuse!` 等号令性错误始终打印，warning 不影响退出码。保留计数行的理由：完全静默会让 unmanaged / 无 base 这类安全网 warning 彻底消失。改这里时注意：e2e 用例若断言 warning 正文必须带 `-v`，USAGE / README「Output」段 / AGENTS.md 需同步。
