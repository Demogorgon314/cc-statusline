# cc-statusline

基于 [kimi-statusline](https://github.com/Demogorgon314/kimi-statusline) 的 Rust 实现，参考 [CCometixLine](https://github.com/Haleclipse/CCometixLine)，为 Claude Code 提供可配置状态栏。

[English / 完整配置参考](README.md)

## 安装

在当前仓库执行：

```sh
cargo install --path . --locked
cc-statusline install
```

重启 Claude Code。安装器只修改 `~/.claude/settings.json` 的 `statusLine`，备份原文件到 `settings.json.bak`；已有其他状态栏时，需要显式使用 `install --force`。支持 `CLAUDE_CONFIG_DIR`，不会读取或修改 Kimi 的配置。

安装器使用带 shell 引号的绝对程序路径。Windows 通过 Claude Code 的 Git Bash 执行。`install.sh`、`install.ps1` 和自动更新依赖本仓库已发布的 GitHub Release；首次发布前请从源码安装。

## 支持内容

- 模型、实时推理强度、上下文占比与 token 数。
- Claude 提供的会话费用、累计运行时长、输出样式、代码增删行数。
- Vim 模式、Agent 名称、快速模式。
- Git 分支、detached HEAD、工作区变更、冲突、领先/落后数量及 PR 链接。
- 当前会话及子代理累计输入/输出 token、缓存命中率、用量最多的子代理模型。
- 估算 API 输出吞吐：`≈42 tok/s`，按输出 token 和 API 耗时增量计算。
- 原生五小时/七天额度及网关消费额度、重置倒计时。
- 十套主题、普通/Nerd Font/Powerline 样式、中英文、窄终端自动收缩。
- 支持键盘和鼠标的交互配置器：启停、排序、配色、图标、实时预览。

```sh
cc-statusline                       # 交互菜单
cc-statusline config                # 配置器
cc-statusline init                  # 生成默认配置
cc-statusline themes                # 列出主题
cc-statusline -t nord preview       # 主题预览
cc-statusline preview --width 100
cc-statusline quota                 # 主动查询 OAuth 额度
cc-statusline update --check
cc-statusline uninstall
```

可直接用示例输入体验：`cargo run -- --theme claude --width 160 < examples/claude.json`。

配置文件是 `~/.claude/cc-statusline/config.toml`。可用段：
`mode`、`cost`、`model`、`output_style`、`directory`、`git`、`context`、`usage`、`subagent`、`session`、`quota`、`changes`、`tps`。其中输出样式、会话时长、增删行数默认关闭，可在配置器启用。

连续两次 Ctrl+C（1.5 秒内）退出配置器并放弃未保存的修改。预览使用该目录最近一次实际收到的 Claude payload；首次使用时显示通用 Claude 标签，配置器则补充演示数据。

## 数据口径与兼容性

上下文占用优先使用 Claude 传入的百分比；回退计算使用输入 token 与缓存读写之和，不包含输出。会话累计统计单独解析 transcript，避免旧版/新版 `total_*_tokens` 含义不同而产生误算。

同一次回复拆出的多条记录按 `message.id` 去重，并保留完整用量。日志增量读取，末尾未写完的记录留到下次，截断/替换会重置游标。首次读取大日志会分多次刷新追上。只读取当前 `transcript_path` 及其子代理目录，不会拿其他会话填补新会话数据。

TPS 显示为 `≈42 tok/s`，计算公式为「去重后的累计输出 token 增量 × 1000 ÷ `cost.total_api_duration_ms` 增量」。包含日志中可见的子代理输出；分母包含首 token 等待和重试，并行请求按各自 API 耗时累计。日志和计时更新不同步、部分辅助 API 调用未出现在 transcript 中，因此这是估算的 API 输出吞吐，不是纯解码速度或并行墙钟吞吐。

首次完整采样只建立基线，两项计数均增长后显示结果。数据缺失、日志未读完、模型变化、计数回退、日志替换、子代理日志集合变化或两次采样相隔超过 30 分钟时会重新建立基线。空闲时保留上次结果，默认 5 分钟后变暗；预览不会推进实时采样状态。

新配置默认开启 TPS；已有配置请在 `cc-statusline config` 中开启 **TPS**，或添加：

```toml
[[segments]]
id = "tps"
enabled = true
options = { stale_secs = 300, hide_when_stale = false }
```

`hide_when_stale = true` 表示过期后隐藏，`stale_secs = 0` 表示不变暗。窄终端会按优先级隐藏 TPS。

额度默认使用原生 `rate_limits`，无需额外联网或读取登录凭据。旧版可在 quota 段的 options 中启用 `oauth_fallback = true`：后台使用 Claude 已有 OAuth 登录查询 Anthropic 额度接口，失败时保留缓存，不刷新或改写登录 token。API Key 和第三方供应商用户可保持关闭。

未收到原生 PR 数据时，会后台调用 `gh`；git 段设置 `pr = false` 可关闭。费用为 Claude 的估算值；不以 API 等待时间冒充解码速度，也不修改 Claude 可执行程序。

主题、自定义配色和模型别名请见 [完整配置参考](README.md#configuration)。设置 `NO_COLOR=1` 或 `CC_STATUSLINE_NO_COLOR=1` 输出纯文本。缓存位于 `~/.claude/cc-statusline-cache/`，启用 `CC_STATUSLINE_DEBUG=1` 后日志写入 `~/.claude/cc-statusline-debug.log`。

## 开发验证

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

协议以 [Claude Code 官方文档](https://code.claude.com/docs/en/statusline) 为准，并参考旧版源码核对 transcript、子代理布局与凭据存储。CI 覆盖 Linux、macOS、Windows；发布流程构建六个平台的二进制归档。

MIT；保留原项目 [LICENSE](LICENSE)。
