---
name: config-sync-chezmoi
description: chezmoi 已弃用；dotfiles 归 cfgsync 中心仓库（每应用一目录 + path.rules 记各平台路径）
type: project
---

2026-11-12 用户明确「chezmoi 我不用了」，此前那条「chezmoi source state 是唯一权威」的记忆作废。`~/.local/share/chezmoi` 和私有 `dotfiles` 仓库只当历史归档，不再 `apply`/`add`/`forget`，也不要再建议走 chezmoi。

现状：dotfiles 归 **cfgsync**。中心仓库 `~/.local/share/cfgsync`（remote 私有 `cfgfiles`），每个应用一个目录，目录名是标签（v0.2.0 起），真实路径写在 `<name>/path.rules`（一行一个平台，`linux = ~/.config/nvim` 这种）。已迁移：fenlyin、ghostty、rime(=`~/.config/ibus/rime`)、nvim、pi(=`~/.pi`)。每棵树自己的 git 历史还在（`cfgsync render/harvest` 会提交）。

日常：`cfgsync status -a` 看漂移 → `harvest` 收本机改动 → `render` 写回 → `push`/`pull` 走 cfgfiles（需代理 `-c http.proxy=http://127.0.0.1:7897`）。`!--` 排除的键（密钥）两边都不落盘。

`~/.pi` 有 4 个 adopt.rules 认领但未 harvest 的文件，属历史漂移，未处理。
