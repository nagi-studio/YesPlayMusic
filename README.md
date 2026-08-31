<p align="center">
  <img src="images/logo.png" alt="YesPlayMusic Logo" width="156" height="156">
  &nbsp;&nbsp;&nbsp;&nbsp;
  <a href="https://tauri.app"><img src="images/tauri-glyph.svg" alt="Built with Tauri 2" height="72"></a>
</p>

<h2 align="center" style="font-weight: 600">YesPlayMusic</h2>

<p align="center">高颜值的第三方网易云播放器 · 桌面版与 TUI 同一个仓库、共用一套 Rust 核心</p>
<p align="center"><sub>Tauri 2 桌面应用 + <code>ypm</code> TUI · macOS 正式发布 · Windows / Ubuntu 实验构建 · 由 <a href="https://github.com/nagi-studio">Nagi Studio</a> 维护</sub></p>

<p align="center">
  <img src="images/tui-now-playing.png" alt="ypm TUI：正在播放" width="423">
  <img src="images/tui-spectrum.png" alt="ypm TUI：频谱与双语歌词" width="375">
</p>

---

这是从 [qier222/YesPlayMusic](https://github.com/qier222/YesPlayMusic) 分出来独立维护的
macOS Tauri 重构版，不再跟随上游发版。界面和主要功能保留，桌面运行时、本地服务、缓存和
窗口交互全部重新实现。想要原版跨平台安装包请去[上游仓库](https://github.com/qier222/YesPlayMusic)。

仓库里是两个前端：Tauri 2 桌面应用，和终端里跑的 `ypm`。网易云的账号、搜索、
歌单、播放源、歌词这些业务逻辑收在同一份 Rust 核心（`core::ncm`）里，两边共用，
所以两个界面的行为一致，修一次两边都好。

Apple Silicon Mac 是正式支持平台，Windows x64 和 Ubuntu x64 由 CI 提供实验构建。欢迎提 Issue 和 PR。

## 迷你播放器

窗口压矮（高度小于 340）自动变成紧凑播放条，最小可到 300×48。空间够时原文下面跟一行中文翻译，
纯音乐显示「纯音乐，请欣赏」。拖回大窗口自动还原。

![迷你播放器：双语歌词](images/mini-player-bilingual.png)

![迷你播放器：双语歌词](images/mini-player.png)

![迷你播放器：纯音乐](images/mini-player-instrumental.png)

播放条上有图钉按钮，把窗口钉在最上层，跟着切桌面和全屏应用走。按钮和红绿灯平时藏着，鼠标移上去才出现。

## 菜单栏歌词

菜单栏图标位置直接显示专辑封面，右边跟着当前歌词走，按显示宽度截断，中日文和英文都能显示得比较满。
迷你条开着时菜单栏只留封面，窗口一收起歌词立刻补回来。

![菜单栏歌词](images/menubar.png)

另外多了 Anon 和 Creeper（Minecraft 苦力怕）两种进度条皮肤（和彩虹猫三者互斥，设置里
切换），并修掉了网络慢时快速切歌会把新歌歌词覆盖成上一首的老问题。

## ypm TUI

![ypm TUI：曲库](images/tui-library.png)

封面直接画在终端里，歌词双语滚动，带频谱。8 套内置主题，支持 Nerd Font 图标和自定义配色。

macOS（Apple Silicon）和 Linux（x86_64）用 Homebrew 安装：

```bash
brew tap nagi-studio/ypm && brew install ypm
```

较新版本的 Homebrew 会要求先 `brew trust nagi-studio/ypm` 信任第三方 tap。
formula 模板在 [`Formula/`](Formula/)，发版后同步到 [`nagi-studio/homebrew-ypm`](https://github.com/nagi-studio/homebrew-ypm)。

也可以从同一 Release 下载 `ypm-macos-aarch64`、`ypm-linux-x64` 或 `ypm-windows-x64.exe`：

```bash
chmod +x ypm-macos-aarch64 # Linux 对应 ypm-linux-x64
xattr -d com.apple.quarantine ypm-macos-aarch64 # 仅浏览器下载需要
./ypm-macos-aarch64
```

Linux x64 以 Ubuntu 22.04（glibc 2.35）为兼容基线，需要 `libasound2`；Windows 建议用 Windows Terminal。

<details>
<summary>主题、图标和配置文件</summary>

<br>

按 `5` 或 `,` 打开设置页；`j/k` 选项、`h/l` 或左右键调整，主题即时预览。
`Enter` 原子保存，`Esc` 取消并恢复原值。语言和封面模式在下次启动后生效。

内置主题：`db16`、`pico8`、`gameboy`、`everforest`、`tokyo-night`、`tokyo-night-storm`、
`one-dark`、`transparent`，其中 `transparent` 继承终端自己的前景色与背景色。

Nerd Font 图标在设置页把「图标」切到 `nerd`：

- macOS：`brew install font-symbols-only-nerd-font`
- Linux：安装任一 Nerd Font 后 fontconfig 会自动 fallback
- Windows Terminal：直接把终端字体换成 Nerd Font 变体

首次启动生成 `~/.config/ypm/config.toml`，也可以直接编辑：

```toml
quality = "exhigh"       # 128 | 192 | 320/exhigh | lossless | hires
cover_mode = "original"  # 终端不支持原图协议时自动回退到 pixel
pixel_scale = 1.0        # pixel 模式采样细节；不会放大封面占用区域
cover_size = "auto"      # compact | auto | large，封面占用区域大小
intro_animation = true   # 启动时播放像素 logo 动画，任意键跳过
# cache_limit_mib = 8192 # 仅显式设置时更新 ypm 进程共享的缓存上限
```

配置文件里出现无法识别的字段、非法的值或语法错误时，ypm 会直接报错退出并指出问题所在，
不会带着默认值启动——否则下一次在设置页保存就会把你原来的配置覆盖掉。

不设置 `cache_limit_mib` 时沿用缓存数据库现有值，新数据库默认 8 GiB。

自定义主题放在 `~/.config/ypm/themes/<name>.toml`，配置里写 `theme = "<name>"`。
色板需要 2–64 个 RGB 十六进制颜色，`roles` 的值是色板下标：

```toml
palette = ["#1a1b26", "#565f89", "#c0caf5", "#7aa2f7", "#bb9af7"]

[roles]
bg = 0
fg = 2
dim = 1
faint = 1
accent = 3
accent2 = 4
sel = 3
```

</details>

## 给 AI agent 的 skill

仓库自带一个 [Agent Skill](skills/ypm/SKILL.md)，让任何支持 SKILL.md 标准的
agent（Claude Code、Cursor、Codex、dsh 等）通过 `ypm` CLI 控制播放：
查在放的歌、暂停/继续、切歌、跳到指定时间（`ypm seek <秒>`）。

程序需要读取 TUI 状态时可运行 `ypm --json --tui status`。返回值包含
`playing`、`title`、`artist`、`album`、`positionMs`、`durationMs`、`coverUrl`、
`seekable`、`iconStyle` 和 `source`；`coverUrl` 是可选的 64×64 网易云 HTTPS CDN 封面地址，
没有封面时为 `null`。`seekable` 表示当前歌曲可接受绝对 seek，`iconStyle` 是 YPM 已选的
`unicode` / `nerd` 图标模式。这三个字段都是向后兼容的增量，调用方应接受旧版缺省。

需要实时可视化时，可从正在运行的 TUI 订阅版本化的 NDJSON 频谱流：

```sh
ypm --json --tui spectrum --fps 12
```

`fps` 可设为 1–20。每行是一个 `version: 1` 帧，包含当前 `style`、`playing` 和固定
32 个 0–255 的归一化 `bins`。接口只投影频谱，不暴露 PCM；首个订阅连接时才启用
分析，最后一个连接断开后自动停用。该流目前和其他 TUI 远程控制一样，只支持 macOS
与 Linux。

```sh
mkdir -p ~/.agents/skills/ypm && curl -fsSL \
  https://raw.githubusercontent.com/nagi-studio/YesPlayMusic/master/skills/ypm/SKILL.md \
  -o ~/.agents/skills/ypm/SKILL.md
```

`~/.agents/skills/` 是各家 agent 通用的位置；dsh 另外还认 `~/.dsh/skills/`
和项目内 `.dsh/skills/`，Claude Code 认 `~/.claude/skills/`，换个目标目录即可。

用 `npx skills add nagi-studio/YesPlayMusic -s ypm -g -a universal` 也能装，
但那会为这一个文件克隆整个仓库。

## 安装

> **[尝鲜版（canary）](https://github.com/nagi-studio/YesPlayMusic/releases?q=prerelease%3Atrue&expanded=true)** 带上面那个 TUI。
> canary 走独立更新通道，和 stable 互不干扰。稳定版请用下面的 Releases 页面。

到 [Releases](https://github.com/nagi-studio/YesPlayMusic/releases) 下载 DMG，
只提供 Apple Silicon（`arm64`），要求 macOS 14 或更高版本。

DMG 经 Developer ID 签名与 Apple 公证，下载后双击即可，不需要额外放行。

v0.9.1 及更早的包没有签名，首次打开时 macOS 会拦一道，放行方法二选一：

- 打开「系统设置 → 隐私与安全性」，往下翻到被拦截的提示，点「仍要打开」
- 或者在终端跑一句：`xattr -dr com.apple.quarantine /Applications/YesPlayMusic.app`

从源码构建的产物不带隔离属性，没有这个问题。

<details>
<summary>签名、自动更新与 Sidecar 源码提供</summary>

<br>

DMG 内的 `.app` 由 `Developer ID Application` 身份签名、启用 Hardened Runtime，并经 Apple
公证，DMG 带 stapler ticket，因此离线也能通过 Gatekeeper 校验。签名与公证是发版门禁：
`codesign --verify`、`spctl --assess` 和 `xcrun stapler validate` 任一失败就不产出安装包。
v0.9.1 及更早的版本走的是 ad-hoc Hardened Runtime seal，没有 Developer ID 身份。

从 tag 构建的包会在启动时静默检查更新，也可以在设置页手动检查、下载和安装。stable 只接收
stable 更新，canary 只接收 canary 更新；更新包使用 Tauri Minisign 验签，与 Apple Developer ID
无关。普通本地构建没有发布公钥，自动更新保持未配置状态。

同一 Release 会提供对应版本的 `YesPlayMusic_<version>_sidecar-source.tar.gz`、SHA-256 与醒目的
`SOURCE-OFFER` 指引，这是 Rust Sidecar 的完整对应源码和离线重链接包。转发或镜像 DMG 时，
请同时保留源码资产和源码下载指引。

</details>

## 三次桌面重构

| 阶段                        | 改动                                                                                                            |
| --------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `v0.6.0`：桌面外壳          | Electron → Tauri 2；升级 Vue 3、Vite 7、TypeScript 6 和 Pinia 4，改用系统 WebView                               |
| `v0.8.0`：后台服务          | Bun Sidecar → Rust Sidecar；页面托管、网易云 API、同源 `/api` 和 UNM 全部改写为 Rust，桌面包不再携带 Bun runtime |
| `v0.9.0`：业务核心          | 网易云客户端逻辑下沉 `core::ncm`；桌面端与 TUI 共用同一份 Rust 实现，通用转发路由从 57 条减到 45 条              |

`.app` 从 82.6 MiB 降到 23.0 MiB，DMG 12.0 MiB，Sidecar 常驻内存约 9 MiB。
详见[功能迁移表](docs/feature-migration.md)和[性能迁移基线](docs/performance-baseline.md)。

## 自己构建

需要 [Bun 1.3.12](https://bun.sh)、Rust 1.89 以上，以及对应平台的 Tauri 系统依赖。

```bash
cp .env.example .env   # 推荐：启用 Last.fm 等完整本地配置
bun install
bun run dev:tauri      # 开发模式
bun run build:tauri    # 按当前系统构建 Tauri 应用
bun run package:tauri:dmg  # 生成 DMG、完整 Sidecar 源码包、下载指引与 SHA-256
```

不复制 `.env` 也能正常加载主界面和网易云 API。`.env.example` 里已经有可用配置，不需要另行申请密钥；
`.env` 不进版本库。运行仓库里的 `npx` 辅助命令时另需 Node 20 以上。

Tauri 产物在 `src-tauri/target/<target-triple>/release/bundle/`，macOS 的 DMG 和源码包在
`dist_tauri/`。TUI 单独构建：

```bash
cargo build --locked --release --manifest-path src-tauri/Cargo.toml -p yesplaymusic-tui
```

开发细节和踩过的坑都记在 [CLAUDE.md](CLAUDE.md) 里。

## Windows 和 Linux

只有 macOS 是正式支持平台。Windows x64（未签名 NSIS `.exe`）和 Ubuntu x64（AppImage 和 `.deb`）
是同一套 Tauri 外壳的实验构建，未做实装验收：`master` push 只产生 Actions artifact，推 `v*` tag
时和 macOS 包一起进入同一个草稿 Release（未签名安装包会触发 SmartScreen）。本机构建用
`bun run build:tauri:windows` / `bun run build:tauri:linux`。这两个平台没有 `afconvert`，
精确 FLAC 拖动走播放器已有的回退路径。

## 更多界面

以下界面来自上游，这个分支没有改动。

![歌词页](images/lyrics.png)

![音乐库（深色）](images/library-dark.png)

![专辑](images/album.png)

## 致谢

这个项目的一切都建立在 [qier222](https://github.com/qier222) 和
[YesPlayMusic 所有贡献者](https://github.com/qier222/YesPlayMusic/graphs/contributors)
的工作之上。播放器内核、界面设计、歌词、音乐库、网易云 API 的对接，这些真正困难的部分
都是他们写好的，这个分支只是在上面加了几个自己想要的功能。

同样感谢这些被项目依赖的开源工作：

- [NeteaseCloudMusicApi](https://github.com/Binaryify/NeteaseCloudMusicApi) 及其
  [维护分支](https://github.com/neteasecloudmusicapienhanced/api)：网易云 API 的实现
- [UnblockNeteaseMusic](https://github.com/UnblockNeteaseMusic/server)：灰色歌曲解锁
- [Vue](https://vuejs.org)、[Vite](https://vite.dev)、[Tauri](https://tauri.app)

界面设计的灵感来自 [Apple Music](https://music.apple.com)、
[YouTube Music](https://music.youtube.com) 和 [Spotify](https://www.spotify.com)。

## 开源许可

本项目自有的前端与 Tauri 主程序代码沿用上游的 [MIT license](LICENSE)。Rust Sidecar
静态链接了 `GPL-3.0-only` 依赖，因此 Sidecar 组合程序及其源码按
[GPL-3.0-only](legal/GPL-3.0.txt) 分发。每个包含 Rust Sidecar 的新 Release 会在同一
下载页提供完整对应源码、第三方 notice、校验和与离线重链接说明。

MIT 与 GPL-3.0 均允许商业使用，本项目不附加“仅限个人或非商业用途”的代码许可限制。
使用者仍须自行遵守网易云音乐服务条款、适用法律和音乐版权要求；本项目不提供音乐内容，
也不授予任何音乐作品的商业使用权。

TAURI is a trademark of The Tauri Programme within the Commons Conservancy. README 中使用的
Tauri 标识来自官方 Logopack，仅作技术栈说明。
