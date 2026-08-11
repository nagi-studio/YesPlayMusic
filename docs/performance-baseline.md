# 性能迁移基线

## 目标

Electron 与 Tauri 必须使用同一套口径比较，不能只看 Activity Monitor 里某一个 helper。
本项目按根 PID 递归统计整个进程树：

- 内存：各进程 RSS 相加，记录 mean / P95 / max；
- CPU：各进程 `%CPU` 相加，记录 mean / P95 / max；
- 体积：比较 `.app` 目录总大小；
- 场景：冷启动 readiness/core/WebView 短采样、可见暂停稳态 5 分钟、窗口隐藏 5 分钟，以及连续真实播放 0-5/5-10 分钟。

可见暂停与窗口隐藏是两个独立场景，数据和门禁不得互相替代。

RSS 会重复计算进程间共享页，因此不是物理内存的绝对值；只有同一机器、同一场景、同一采样器的两版数据才能用于相对比较。macOS 另外记录后端进程 `phys_footprint`，用于隔离 Bun→Rust 的真实后端收益；不能把 WebKit 变化算给 Sidecar。

## 本 fork 迁移前本地静态基线（2026-08-02）

| 项目              | Electron 版 | 说明                                       |
| ----------------- | ----------: | ------------------------------------------ |
| `.app` 总大小     |   381.5 MiB | `dist_electron/mac-arm64/YesPlayMusic.app` |
| renderer 全部资源 |    4.13 MiB | `out/renderer` 所有文件                    |
| Bun arm64 sidecar |    63.9 MiB | 单文件，包含 Bun runtime 与 1,077 个模块   |

此前的探索性采样中，Electron 整棵进程树 RSS 约为 383-722 MiB，Electron Framework
自身约 273 MiB。这个范围只用于判断优化量级；正式验收必须用下面的固定场景重新采样。

## Tauri 后台核心中间结果（2026-08-02）

`bun run smoke:tauri:core` 在不创建 WebView、不显示窗口的条件下，验证 production bundle
里的真实 sidecar、静态页面、同源 API 和退出回收：

| 项目                  |                结果 |
| --------------------- | ------------------: |
| `.app` 总大小         |            73.4 MiB |
| Tauri 主进程 RSS      |            79.2 MiB |
| Bun sidecar RSS       |           90.25 MiB |
| 两进程 RSS mean / P95 | 169.54 / 169.56 MiB |
| 两进程 CPU mean / P95 |        0.38% / 1.3% |

包体积相对 Electron 下降约 80.8%。后台核心 RSS 相比此前 Electron 探索性范围低约
55.7%-76.5%，但这个数字**不含 WKWebView 的 WebContent / Networking 进程**，只能说明
Rust + Bun 后台的固定成本，不能当作最终播放器内存。完整结果要等隐藏 WebView 和正常播放
场景接入后再测。该段只保留为 2026-08-02 的 Bun 历史基线。

## 采样方法

先拿到被测版本的**精确根 PID**，再运行：

```bash
bun scripts/measure-process-tree.mjs \
  --pid 12345 \
  --include-pids 12351,12352,12353 \
  --duration 300 \
  --interval 1 \
  --label electron-hidden
```

工具只读取指定 PID、显式 include PID 及各自后代，不按应用名扫描，也不会启动、聚焦或关闭播放器。macOS WebKit XPC 通常会重挂到 PID 1，必须在启动前记录已有 WebKit PID、只把本次新建的 GPU/Networking/WebContent PID 传给 `--include-pids`；否则所谓“完整树”会漏掉最大的内存进程。

## 正常播放场景实测（2026-08-10，Apple Silicon / macOS 15）

首次按完整口径（含 WKWebView 各进程）测量正常播放场景，并发现、修复了一个
CPU 回归：

- **修复前**：Tauri 主进程恒定约 99% CPU（`MainEventsCleared` 每轮迭代刷新托盘
  标题，查询窗口状态唤醒 run loop 形成自持续空转），与负载无关；
- **修复后**（托盘标题改事件驱动 + 1s 对账线程，commit 64b680c）：播放态主进程
  6%-8%，空闲更低；60 秒进程树采样 CPU mean 8.5% / P95 16.7%。

内存（`footprint` 的 phys_footprint 口径，非 RSS）：

| 进程              |             phys_footprint |
| ----------------- | -------------------------: |
| Tauri 主进程      |                      58 MB |
| Bun sidecar       |                      82 MB |
| WebKit WebContent | 444 MB（观测峰值 1.76 GB） |
| 合计              |                  约 605 MB |

高强度交互（连续切歌、搜索、调窗口）时 WebContent 峰值约 49% CPU、GPU 进程约
67%，操作结束后均回落。RSS 含大量共享页，不作为对外数字；对外只引用
phys_footprint 与场景说明。用户缓存（`~/Library/WebKit/com.electron.yesplaymusic`
等）随使用增长，与安装体积分开陈述。

## Rust-only installed 实测（2026-08-11，`0.8.0-canary.1`）

环境：Apple M5 Pro / arm64、macOS 26.4.1（25E253）、Bun 1.3.12、Rust 1.89.0。
候选包冻结在 commit `1b891bc1059f1b92b090bf08d56a12f63b5ab5a9`。DMG 挂载后将
`.app` 复制到全新临时目录再启动，未运行 `target/release` 裸二进制。

| 分发物                                    | 结果                                                                                                                                |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `YesPlayMusic_0.8.0-canary.1_aarch64.dmg` | 12,551,495 bytes（11.970038 MiB）；`hdiutil verify` **PASS**                                                                        |
| DMG SHA-256                               | `372f921c199f4d8b29534c24d61bef5a49546582177a2b94668ee0e6ea0bd08e`                                                                  |
| 安装后 `.app`                             | 23,528 KiB（22.976563 MiB）                                                                                                         |
| 相对本 fork v0.7.0 Bun `.app` 82.555 MiB  | -72.168%；54.1 MiB hard gate **PASS**                                                                                               |
| 独立完整 Sidecar source asset             | 75,084,239 bytes（71.605910 MiB）；SHA-256 `bb1bc56c406c1316b24d240ef17dc9a5af1c5a1f89c17984e4ccb3f5722e46d9`；不进入 DMG 或 `.app` |
| `SOURCE-OFFER`                            | 905 bytes；SHA-256 `abd3ff34ae98d24acf9190150f2938c5834ca1cee08005bcffd76b760b55be29`；与 `.app` 内指引一致                         |
| Updater 签名资产                          | 本机未配置发布私钥，未生成可发布的 updater archive 与 `.sig`；tag CI / updater artifact / canary feed **PENDING**                   |

这份 pre-tag 本地候选包的 updater 公钥为空，endpoint 仍是 stable latest，因此不作为最终
canary 发布资产。Tag CI 需要注入 updater secrets 重新构建，并验收 canary endpoint、
updater archive 与签名；这些检查当前均为 **PENDING**。

体积比较分为本 fork 的迁移基线和上游官方发行物的外部历史参考：

| 比较对象                             |                           基线 |                              当前 |    降幅 | 用途           |
| ------------------------------------ | -----------------------------: | --------------------------------: | ------: | -------------- |
| 本 fork v0.7.0 Bun `.app`            |       84,536 KiB（82.555 MiB） |       23,528 KiB（22.976563 MiB） | 72.168% | 迁移 hard gate |
| 上游 qier222 v0.4.10 官方 arm64 DMG  | 93,085,284 bytes（88.773 MiB） | 12,551,495 bytes（11.970038 MiB） | 86.516% | 外部历史参考   |
| 上游 qier222 v0.4.10 挂载后的 `.app` |     217,020 KiB（211.934 MiB） |       23,528 KiB（22.976563 MiB） | 89.159% | 外部历史参考   |

上游 v0.4.10 DMG 的 SHA-256 是 `bf7564f451f0e25217015c0f2a83e1385f7a407a42daf0be8d8d992c471160d8`，
`hdiutil verify` 通过。其 `.app` 没有 Developer ID 或公证，bundle 不能通过
`codesign --verify --deep --strict`，主 Mach-O 只有 linker ad-hoc 签名。上游数据不用于本 fork 的 matched 性能比较。

本机 bundle 是 adhoc Hardened Runtime 签名。深度严格校验、arm64 host/Sidecar、空
entitlement、无 Bun/payload、Sidecar provenance/source 门禁，以及精确覆盖 328 个
host package 和 44 个 renderer package 的 app-compliance bundle 校验均通过。这是当前目标发布形态；
Developer ID 和公证按当前政策为 **N/A**。

下面的 bundle 检查、smoke 与长时性能数据全部来自上述 checksum 的
`0.8.0-canary.1` 本地 DMG，以及它挂载后复制出的同一个临时安装副本。本地 DMG SHA 不代表
未来 tag CI 构建的 artifact，也不计入 90 天 canary 观察期。

冷启动 smoke：

- core：API/Renderer 1.434/1.435 s；5 个样本完整 core RSS mean 95.11 MiB、CPU mean
  0.08%；Sidecar RSS mean 11.45 MiB；
- WebView：WebView event 0.396 s、API/Renderer 2.204/2.204 s；启动与网络阶段 8 个样本完整树
  RSS mean 1,216.46 MiB、CPU mean 14.88%，Sidecar RSS mean 32.25 MiB；只作冷启动诊断，不作稳态数据；
- supervisor primary host/Sidecar PID 为 `78260/78264`；端口冲突 host PID 为 `78302`；
  重启预算场景 host PID 为 `78317`，Sidecar PID 为 `78320 → 78329 → 78348 → 78380`，
  前三次本地 health/player 恢复，第四次按预算停止重启；
- 正常 Cmd+Q 退出记录 Sidecar `Some(0)`；host/Sidecar PID `78505/78514` 与 WebKit PID
  `78517/78518/78519` 均已消失。退出前 `12754/27232/28232` 在监听，`27233` 因未配置代理始终
  没有 listener；退出后四个端口均无 listener。

四个稳态窗口均为 300 个样本、1 秒间隔：

| 场景                 |  完整树 RSS mean / P95 / max | 完整树 CPU mean / P95 / max | Tauri RSS / CPU mean | Sidecar RSS / CPU mean |              Sidecar `phys_footprint` |
| -------------------- | ---------------------------: | --------------------------: | -------------------: | ---------------------: | ------------------------------------: |
| 可见暂停稳态 0-5 min | 255.35 / 256.89 / 256.98 MiB |           0.14 / 1.1 / 2.7% |    89.64 MiB / 0.01% |      10.15 MiB / 0.01% | 结束 8.203606 MiB；peak 38.391151 MiB |
| 连续播放 0-5 min     | 804.52 / 812.97 / 818.38 MiB |          1.58 / 4.8 / 19.2% |    93.72 MiB / 0.30% |      12.92 MiB / 0.00% |                   5 分钟 8.938004 MiB |
| 连续播放 5-10 min    | 834.59 / 843.91 / 857.64 MiB |          1.81 / 5.0 / 65.3% |    93.59 MiB / 0.32% |      13.28 MiB / 0.01% |                  10 分钟 8.609901 MiB |
| 窗口隐藏 0-5 min     | 553.43 / 560.03 / 592.05 MiB |           0.15 / 1.0 / 1.5% |   103.78 MiB / 0.01% |      13.36 MiB / 0.01% |                          8.703651 MiB |

四个窗口来自同一个 installed session：host PID `78505`、Sidecar PID `78514`，WebKit
GPU/Networking/WebContent PID 为 `78517/78518/78519`。每个时间窗口都是独立的 300 样本
区间，可见暂停与隐藏暂停不互相替代。

播放开始时为 `あいつら全員同窓会 · ずっと真夜中でいいのに。`，进度约 221.55 s；5 分钟点为
`真夜中のドア〜stay with me (シングルver.) · 松原みき` 4.30 s；10 分钟点为
`Notion · The Rare Occasions` 57.62 s。三个检查点均由用户确认处于播放态。播放后暂停并
隐藏窗口，进度保持静止，`lsappinfo` 同时报告 hidden 状态。

第二个播放窗口的完整树 RSS mean 比第一个高 30.07 MiB（3.7%），增量主要来自
WebKit。Sidecar RSS mean 从 12.92 MiB 增至 13.28 MiB，`phys_footprint` 从 8.938004 MiB
降至 8.609901 MiB。10 分钟内没有 Sidecar 物理内存持续累积的证据，这组数据不能证明
长期运行不存在泄漏。

逐样本证据可直接复核：

- [可见暂停稳态 0-5 min](./evidence/sidecar-rust-migration/installed-idle-5m.json)
- [连续播放 0-5 min](./evidence/sidecar-rust-migration/installed-playback-0-5m.json)
- [连续播放 5-10 min](./evidence/sidecar-rust-migration/installed-playback-5-10m.json)
- [窗口隐藏 0-5 min](./evidence/sidecar-rust-migration/installed-hidden-idle-5m.json)
- [人工 UI、`phys_footprint` 与退出回收记录](./evidence/sidecar-rust-migration/installed-footprints.json)

四份性能证据均为 schemaVersion 3，包含 300 个 `rawSamples`、`processSummaries`、
artifact/executable SHA-256、`measurement.rootProcessStartedAt`、逐进程
`startedAt`/`execTokenHash` 和 1 秒间隔检查。四份均已由 `verify-performance-evidence.mjs`
独立重算通过。JSON 保留采样机的绝对路径；换机器或清理临时安装目录后，必须把同 SHA
DMG 复制出的 host 路径显式传给 verifier：

```bash
bun scripts/verify-performance-evidence.mjs \
  docs/evidence/sidecar-rust-migration/installed-idle-5m.json \
  --artifact dist_tauri/YesPlayMusic_0.8.0-canary.1_aarch64.dmg \
  --executable /path/to/YesPlayMusic.app/Contents/MacOS/yesplaymusic-tauri
```

窗口可见或隐藏、播放或暂停状态、曲目和进度属于人工 UI 观察，不在性能 JSON 内。
`installed-footprints.json` 记录这些观察、Sidecar `phys_footprint` 和退出回收；四份 schema v3
JSON 独立证明各时间窗口的进程身份、CPU/RSS 与时间序列。

这些数据不能直接证明完整树相对 Bun/Electron 降低多少：历史 Electron/Bun 数据没有在同一机器、
同一场景重跑。Rust Sidecar 自身远小于历史 Bun Sidecar 约 82 MB 的 `phys_footprint`，
WebKit 仍是全应用内存的主要部分。

## Tauri 验收线

- 隐藏窗口 CPU mean 不高于 2%，且明显低于修复前约 30% 的探索性结果；
- 播放态主进程 CPU mean 不高于 10%（2026-08-10 实测 6%-8%，回归门禁）；
- 完整树内存降幅只在同机同场景的 Bun/Electron 对照完成后判定；当前 **PENDING**，不得拿历史探索性范围计算宣传数字；
- 正常播放 10 分钟内不得出现可归因于 Sidecar 的持续内存累积；RSS 与 `phys_footprint` 联合判断，WebKit 波动不计作后端收益或回归；
- 随机播放、缓存、登录 cookie、托盘歌词不因降内存而回归。

2026-08-11 Rust-only 本机判定：包体积 hard gate **PASS**；隐藏窗口完整树 CPU
**PASS**（0.15%）；可见暂停稳态完整树 CPU 0.14% 与主进程 CPU 0.01% 为可见场景观察值；
播放态主进程 CPU **PASS**（0.30% / 0.32%）。Sidecar 5→10 分钟 RSS mean 增加
0.36 MiB，`phys_footprint` 减少 0.328103 MiB，10 分钟物理内存趋势 **PASS**。相对历史
Bun Sidecar 粗略记录的约 82 MB `phys_footprint`，Rust 四个窗口的结束值为
8.203606-8.938004 MiB，后端量级约低 89%-90%。历史值没有 raw bytes，也不是同时刻的
matched run，该比较不用于宣称完整应用内存降幅。完整树 matched baseline、真实登录 cookie
与 tag CI updater 资产仍是 **PENDING**；Developer ID/公证按当前发布政策为 **N/A**。
