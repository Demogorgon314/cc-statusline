<div align="center">

# cc-statusline

**在 Claude Code 底栏直接看到这次会话花了多少：token、缓存命中率、套餐额度。**

[![Release](https://img.shields.io/github/v/release/Demogorgon314/cc-statusline?style=flat-square)](https://github.com/Demogorgon314/cc-statusline/releases)
[![CI](https://img.shields.io/github/actions/workflow/status/Demogorgon314/cc-statusline/ci.yml?branch=main&style=flat-square&label=ci)](https://github.com/Demogorgon314/cc-statusline/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)
![Rust](https://img.shields.io/badge/rust-%E2%9C%93-orange?style=flat-square&logo=rust)

[English](README.md) | 中文

![cc-statusline 的 Claude Code 演示会话](assets/hero.png)

</div>

## 安装

```bash
curl -fsSL https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.sh | sh
```

重启 Claude Code 即可加载状态栏。

<details>
<summary>Windows、手动配置或源码安装</summary>

**Windows（PowerShell）**

```powershell
irm https://raw.githubusercontent.com/Demogorgon314/cc-statusline/main/install.ps1 | iex
```

**从源码安装**

```bash
cargo install --git https://github.com/Demogorgon314/cc-statusline --locked
cc-statusline install
```

已有本地仓库时，运行 `cargo install --path . --locked`，再运行 `cc-statusline install`。

**手动配置** `~/.claude/settings.json`：

```json
{
  "statusLine": {
    "type": "command",
    "command": "cc-statusline",
    "padding": 0
  }
}
```

将这个对象合并到已有配置。自动安装器会使用带引号的可执行文件绝对路径，不依赖 PATH；保留无关设置，备份为 `settings.json.bak`，且不会覆盖别人的状态栏命令（需要的话加 `install --force`）。`CLAUDE_CONFIG_DIR` 可以覆盖默认的 `~/.claude` 路径。

需要支持命令式状态栏的 Claude Code。Windows 下 Claude 通过 Git Bash 执行命令。Nerd Font 和 powerline 样式需要终端使用 Nerd Font；默认 plain 样式不需要。

</details>

## 为什么用它

不用打断工作，就能随时看到 Claude Code 会话的这些信息：

- 📊 **整个会话一共用了多少 token**（主 agent 加可见的子 agent），以及**缓存命中率**，低红高绿
- ⏳ **5 小时和 7 天额度用了多少**、什么时候重置（例如 `5h 42% ↻1h20m`）
- 🧩 **哪个子 agent 模型输入用量最多**，以及 Claude 估算的会话费用
- 🌿 **git 状态一眼可见**：改动行数、ahead/behind、可点击的 `[PR#42]`

模型和实时思考强度、上下文占用、Vim / agent / fast 模式徽标、估算输出速度也都能显示。模型名称中的长上下文说明会简写成 `Opus 5.5 [1M]`。

## 随心定制

![全部 10 套内置主题](assets/themes.png)

10 套内置主题，从 Claude 配色的默认样式，到完整的 powerline。运行 `cc-statusline` 后选择配置器，或用 `cc-statusline config` 直接打开：

![TUI 配置器](assets/configurator.png)

开关和排序各个段、选颜色和图标、切换主题，用你自己会话的数据实时预览。键盘和鼠标都能操作：单击选中，再点一次切换或编辑，拖动段来调整顺序，滚轮上下移动。设计参考 [CCometixLine](https://github.com/Haleclipse/CCometixLine)。

在任意 TUI 界面，**1.5 秒内连按两次 Ctrl+C** 即可退出。第一次会显示确认提示，退出时丢弃未保存的修改。

## 为速度而生

cc-statusline 是单个 Rust 二进制。会话变长时，每次刷新仍限制处理量：

- **增量读取**会话日志：从缓存游标继续，每个文件最多读取 4 MiB，每次刷新解析预算为 100 ms；历史记录分多次刷新追上
- 可选的 OAuth 额度刷新、`gh pr view` 都在**后台**做；Claude 原生额度无需额外请求
- 终端变窄时**自动精简**，压缩显示并逐步去掉优先级较低的段

![四种终端宽度下的自适应效果](assets/adaptive.png)

## 参考

<details>
<summary>所有段</summary>

| id | 内容 |
| --- | --- |
| `mode` | Vim 模式、agent 名称、fast 模式徽标 |
| `cost` | Claude 估算的会话费用（美元） |
| `model` | 模型和实时思考强度；简写的 `[1M]` 后缀 |
| `output_style` | 输出风格（默认关闭） |
| `directory` | 工作目录 |
| `git` | 分支、改动统计、冲突、ahead/behind、打开的 PR（回退查询需要 `gh`） |
| `context` | 上下文占用百分比和输入 token / 容量 |
| `usage` | 整个会话的输入 ↑ / 输出 ↓ / 缓存命中率，包含子 agent |
| `subagent` | 同上，针对输入用量最多的子 agent 模型 |
| `session` | 累计会话时长（默认关闭） |
| `quota` | 5h / 7d 额度、可选的网关消费上限和重置时间 |
| `changes` | 会话新增 / 删除行数（默认关闭） |
| `tps` | 估算 API 输出速度：`≈42 tok/s`（紧凑模式为 `≈42 t/s`） |

空间不够时按这个顺序去掉：session → tps → changes → git → directory → subagent → output_style → cost → context → quota → mode。

上下文 token 统计输入、缓存读取和写入，不含输出；优先使用 Claude 原生百分比。会话累计用量只读取明确传入的 transcript 及其相邻子 agent 日志，按消息 ID 去重。缺失会话不会继承其他会话的统计。额度属于账号，可以跨会话保留。

预览和配置器使用当前目录最近收到的状态栏数据。`preview --session ID` 只接受匹配的缓存会话。还没有数据时，预览显示通用 Claude 标签，配置器会为缺失字段补上演示值。

**TPS 怎么计算**

```text
去重后的 transcript 输出 token 增量 × 1000 / cost.total_api_duration_ms 增量
```

包含可见子 agent 的输出。API 时间包含首 token 等待和重试；并行请求各自的耗时会累加。因此它是按 API 请求时间折算的吞吐估算，不是纯解码速度或并行时的实际墙钟吞吐。日志和计时可能分别更新，部分 API 调用也可能不在 transcript 中。

第一次完整采样建立基线，两项计数都增长后才显示速度。空闲刷新保留上一次估算。计时缺失、日志不完整或历史记录尚未追完、模型变化、计数回退、日志被替换、子 agent 文件集合变化，或采样间隔超过 30 分钟时，都需要重新建立基线。预览不会推进采样器。

新配置默认开启 TPS；已有配置请在 `cc-statusline config` 中开启 **TPS**。默认五分钟没有新采样后变暗，相关选项见下方。

</details>

<details>
<summary>配置文件</summary>

`~/.claude/cc-statusline/config.toml`（自定义主题放在 `themes/<name>.toml`），沿用 CCometixLine 按段配置的形式：

```toml
theme = "claude"

[style]
mode = "plain"          # plain | nerd_font | powerline
separator = "  "        # "" 为 powerline 箭头
lang = "zh"             # 或 "en"
palette = "dark"        # dark | light | 自定义配色名
width = 0              # 自动检测；--width 优先

[[segments]]
id = "quota"
enabled = true
colors = { text = "text_dim" }
options = { show_5h = true, show_7d = true, show_spend = true, show_reset = true, bar = false, oauth_fallback = false, refresh_secs = 120 }

[[segments]]
id = "tps"
enabled = true
options = { stale_secs = 300, hide_when_stale = false }
```

段按配置顺序显示，未列出的段会追加但保持关闭。用 `cc-statusline init` 生成完整的初始配置。TPS 设置 `hide_when_stale = true` 可隐藏过时估算；`stale_secs = 0` 可关闭变暗。

颜色支持配色名（`primary`、`accent`、`text_dim`、`success`、`warning`、`error` 等）、`"#rrggbb"`、`{ c16 = 14 }`、`{ c256 = 208 }` 或 `{ r = 1, g = 2, b = 3 }`。自定义配色放在 `palettes/<name>.json`，包含 `base`（`dark` / `light`）和 `colors` 对象，颜色键使用 `textDim` 等 camelCase 名称。

内置主题：`claude`、`cometix`、`default`、`minimal`、`gruvbox`、`nord`、`powerline-dark`、`powerline-light`、`powerline-rose-pine`、`powerline-tokyo-night`。

可选的 `~/.claude/cc-statusline/models.toml` 按收到的完整模型显示名或 ID 设置别名：

```toml
"claude-sonnet-custom" = "Team Sonnet"
```

</details>

<details>
<summary>额度是怎么拿到的</summary>

优先使用 Claude 原生的 `rate_limits`，不需要额外请求或读取登录凭据。旧版客户端可以开启 `oauth_fallback = true`；后台刷新最多每 `refresh_secs` 秒一次（最短 30 秒）。`cc-statusline quota` 会主动请求 OAuth 用量接口。

回退模式读取 Claude 已有的 macOS Keychain 或 `.credentials.json` 登录凭据，请求 `https://api.anthropic.com/api/oauth/usage`。不会刷新或写入登录 token。请求失败时保留最后的缓存值；API key 和第三方账号可以保持关闭回退。

Claude 没有提供 PR 时，也会在后台查询 PR。将 git 段的 `pr = false` 即可关闭查询。缓存位于 `~/.claude/cc-statusline-cache/`。

</details>

<details>
<summary>命令、调试、卸载</summary>

```bash
cc-statusline                     # 菜单：配置、安装、测试额度、检查更新……
cc-statusline config              # 配置器
cc-statusline init                # 写入默认配置；--force 可覆盖
cc-statusline themes              # 列出主题
cc-statusline -t nord preview     # 在终端里预览某个主题
cc-statusline preview --cwd /path/to/project --width 100
cc-statusline quota               # 立即拉取额度
cc-statusline update              # 最新发布版本；--check 只检查
cc-statusline uninstall           # 移除自己的 statusLine 设置，然后重启 Claude
```

- 调试日志：`touch ~/.claude/cc-statusline-debug`，然后看 `~/.claude/cc-statusline-debug.log`。也可以设置 `CC_STATUSLINE_DEBUG=1`。
- 设置 `NO_COLOR=1` 或 `CC_STATUSLINE_NO_COLOR=1` 关闭颜色。
- 试用示例数据：`cargo run -- --theme claude --width 160 < examples/claude.json`。
- 更新会校验 SHA256。卸载保留无关配置和其他应用的状态栏。

</details>

## 参与开发

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
uv run --with pyte python scripts/screenshots.py
```

截图脚本需要 Chrome、ImageMagick 和 Nerd Font，通过 PTY 在 macOS / Linux 上运行。`SHOT_CHROME` 可指定 Chrome 路径，`SHOT_FONT` 可指定字体。已安装 `pyte` 时也可直接运行 `python3 scripts/screenshots.py`。图片使用合成演示数据，由实际二进制和配置器渲染，再放入 HTML 终端框架。

发布新版本：同步更新 `Cargo.toml` 和 `Cargo.lock`，推送对应的 `vX.Y.Z` tag。CI 检查 Linux、macOS 和 Windows；发布工作流构建六种平台归档和校验文件。

致谢：[kimi-statusline](https://github.com/Demogorgon314/kimi-statusline)（项目基础、README 和截图设计）、[CCometixLine](https://github.com/Haleclipse/CCometixLine)（配置器和主题设计）。协议参考：[Claude Code 状态栏文档](https://code.claude.com/docs/en/statusline)。

## License

MIT
