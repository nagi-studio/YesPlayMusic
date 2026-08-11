<p align="center">
  <img src="images/logo.png" alt="YesPlayMusic Logo" width="156" height="156">
  &nbsp;&nbsp;&nbsp;&nbsp;
  <a href="https://tauri.app"><img src="images/tauri-glyph.svg" alt="Built with Tauri 2" height="72"></a>
</p>

<h2 align="center" style="font-weight: 600">YesPlayMusic</h2>

<p align="center">高颜值的第三方网易云播放器</p>
<p align="center"><sub>Tauri 2 重构版 · macOS 正式发布 · Windows / Ubuntu 实验构建 · 由 <a href="https://github.com/nagi-studio">Nagi Studio</a> 维护</sub></p>

---

## 关于这个仓库

这是从 [qier222/YesPlayMusic](https://github.com/qier222/YesPlayMusic) 分出来独立维护的
macOS Tauri 重构版，不再跟随上游发版。原项目的界面和主要功能保留下来，桌面运行时、
本地服务、缓存和窗口交互已经针对 macOS 重新实现。

如果你是来找原版的，请直接去[上游仓库](https://github.com/qier222/YesPlayMusic)，
那边有完整的跨平台安装包和文档。

**欢迎提 Issue 和 PR。** Apple Silicon Mac 是正式支持平台；Windows x64 和 Ubuntu x64
由 CI 提供实验构建，等待更多真实设备反馈。

## Tauri 重构版改了什么

**v0.6.0 是一次完整的 Tauri 重构。** 渲染层升级到 Vue 3 和 Vite 7，桌面外壳改为
Tauri 2。应用不再捆绑 Chromium。窗口、菜单栏、媒体状态和 Sidecar 生命周期由 Rust
主进程负责；独立的 Rust Sidecar 托管页面，并提供网易云 API、同源 `/api` 代理和 UNM。
当前桌面包不再携带 Bun runtime。

当前代码已迁移到 TypeScript 6.0 严格模式与 Pinia 4。Vue 组件、API、播放器和前端测试
参加 TypeScript 类型检查；Rust 主进程与 Rust Sidecar 由 Cargo 测试和 Clippy 验证。
外部响应在运行时验证后才进入业务层。Electron 运行时已删除，
原有桌面能力的 Tauri 对应实现和验收状态见
[Electron → Tauri 功能迁移表](docs/feature-migration.md)。

Rust Sidecar 迁移后的 `0.7.1-canary.1` 本机验收包中，Apple Silicon `.app` 从本 fork
v0.7.0 的 82.555 MiB 降到 22.582 MiB，减少 72.6%；DMG 为 11.846 MiB。作为外部历史
参考，上游 qier222 v0.4.10 官方 arm64 DMG 为 88.773 MiB，挂载后的 `.app` 为
211.934 MiB，当前分别减少 86.7% 和 89.3%。

安装包实测的隐藏窗口完整进程树 CPU mean 为 0.57%，连续播放两个 5 分钟窗口的 Tauri
主进程 CPU mean 为 1.26% 和 1.20%；Rust Sidecar 稳态 `phys_footprint` 为 8.97 至
9.52 MiB，连续播放 10 分钟没有观察到 Sidecar 物理内存持续累积。相对历史 Bun Sidecar
约 82 MB 的粗略记录，后端占用量级约低 88% 至 89%；这不是 matched run。完整进程树内存
受 WebKit 和媒体缓存影响，尚无同场景 Bun 对照，因此不据此宣传整机内存降幅。逐样本
证据、测量口径和限制见[性能迁移基线](docs/performance-baseline.md)。

**迷你播放器。** 把窗口压矮（高度小于 340）就会自动变成一条紧凑的播放条，
左边小封面配歌名歌手，中间是当前这句歌词，右边是播放控制。只把窗口拖窄不会触发播放条，
而是切到完整播放器视图（封面加控制条的竖窗布局）。空间够的时候原文下面还会跟一行
中文翻译，纯音乐则显示「纯音乐，请欣赏」。拖回大窗口自动还原成完整歌词页。
原版最小窗口是 1080×720，这里放开到了 300×48。

**纯音乐**

![迷你播放器：纯音乐](images/mini-player-instrumental.png)

**双语歌词**

![迷你播放器：双语歌词（Venice Bitch）](images/mini-player-bilingual.png)

![迷你播放器：双语歌词（Red）](images/mini-player.png)

**窗口置顶。** 迷你条上有个图钉按钮，点一下把窗口钉在最上层，切到别的桌面或者全屏应用上面
它也会跟着走，适合一边干活一边扫一眼歌词。按钮平时藏着，鼠标移到播放条上才浮现，
开关状态会记住，下次启动还在。迷你模式下红绿灯也默认收起，悬浮时才出现。

**macOS 菜单栏歌词。** 菜单栏图标位置直接显示专辑封面，右边跟着当前歌词走。
文字按显示宽度截断而不是字符数，所以中日文和英文都能显示得比较满。迷你条开着的时候
菜单栏只留封面，不重复显示一遍歌词；窗口一收起，歌词立刻补回菜单栏。

![菜单栏歌词](images/menubar.png)

**Anon 进度条。** 在原有的彩虹猫之外多加了一种进度条皮肤，设置里可以切换，两者互斥。

**修了一个切歌的老问题。** 网络慢的时候快速切歌，前一首的歌词响应可能晚于后一首到达，
把新歌的歌词覆盖成上一首的。现在请求前会记住是哪首歌，回来发现已经切歌就直接丢弃。

## 安装

到 [Releases](https://github.com/nagi-studio/YesPlayMusic/releases) 下载 DMG。
macOS 正式支持包只提供 Apple Silicon（`arm64`）DMG，要求 macOS 14 或更高版本；
同一 Release 还可能包含下面说明的 Windows 和 Linux 实验包。
同一 Release 还会提供对应版本的
`YesPlayMusic_<version>_sidecar-source.tar.gz`、SHA-256 与醒目的
`SOURCE-OFFER` 指引；这是 Rust Sidecar 的完整对应源码和离线重链接包。转发或镜像 DMG
时，请同时保留源码资产和源码下载指引。

DMG 内的 `.app` 带完整的 ad-hoc Hardened Runtime seal；DMG 本身未签名，也没有
Developer ID 身份签名或 Apple 公证。上游公开版同样没有 Developer ID 和公证，首次打开时
macOS 会拦一道。放行方法二选一：

- 打开「系统设置 → 隐私与安全性」，往下翻到被拦截的提示，点「仍要打开」
- 或者在终端跑一句：`xattr -dr com.apple.quarantine /Applications/YesPlayMusic.app`

自己从源码构建的话没有这个问题，本地产物不带隔离属性。

从 tag 构建的包会在启动时静默检查更新，也可以在设置页手动检查、下载和安装。stable 只
接收 stable 更新，canary 只接收 canary 更新；更新包使用 Tauri Minisign 验签，这与
Apple Developer ID 无关。普通本地构建没有发布公钥，因此自动更新保持未配置状态。

## 自己构建

需要 [Bun 1.3.12](https://bun.sh)、Rust 1.89 以上，以及对应平台的 Tauri 系统依赖。
只有运行仓库里的 `npx` 辅助命令时才另需 Node 20 以上。

```bash
cp .env.example .env   # 推荐：启用 Last.fm 等完整本地配置
bun install
bun run dev:tauri      # 开发模式
bun run build:tauri    # 按当前系统构建 Tauri 应用
bun run package:tauri:dmg  # 生成 DMG、完整 Sidecar 源码包、下载指引与 SHA-256
```

上面这套 Tauri 命令会固定使用同源 `/api`，不复制 `.env` 也能正常加载主界面和网易云 API。
需要 Last.fm 等完整功能时，直接复制 `.env.example`；里面已经有可用配置，不需要另行申请
密钥。`.env` 不进版本库。

各平台 Tauri 产物在 `src-tauri/target/<target-triple>/release/bundle/`。macOS 的 DMG、
完整 Sidecar 源码包、醒目的源码下载指引和各自的校验文件在 `dist_tauri/`；
`package:tauri:dmg` 只用于 macOS。

版本 tag 走这套无 Developer ID 的发布流程，这就是当前正式发布政策；Developer ID 和公证
不是发布要求。仓库仍保留 `APPLE_SIGNING_ENABLED=true` 的可选 CI 能力，但默认不启用。

开发细节和踩过的坑都记在 [CLAUDE.md](CLAUDE.md) 里。

## Windows 和 Linux

只有 macOS 是正式支持平台。Windows x64（未签名 NSIS `.exe`）和 Ubuntu x64（AppImage 和
`.deb`）是同一套 Tauri 外壳的实验构建，未做实装验收：`master` push 只产生 Actions artifact，
推 `v*` tag 时会和 macOS 包一起进入同一个草稿 Release（未签名安装包会触发 SmartScreen）。
本机构建用 `bun run build:tauri:windows` / `bun run build:tauri:linux`。这两个平台没有
`afconvert`，精确 FLAC 拖动走播放器已有的回退路径。

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

## 截图

以下界面来自上游，这个分支没有改动。

![歌词页][lyrics-screenshot]

![音乐库（深色）][library-dark-screenshot]

![专辑][album-screenshot]

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

<!-- MARKDOWN LINKS & IMAGES -->

[album-screenshot]: images/album.png
[lyrics-screenshot]: images/lyrics.png
[library-dark-screenshot]: images/library-dark.png
