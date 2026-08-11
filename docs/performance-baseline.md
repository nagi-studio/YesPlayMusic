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

## Rust-only installed 实测（2026-08-11）

环境：Apple M5 Pro / arm64、macOS 26.4.1（25E253）、Bun 1.3.12、Rust 1.89.0。DMG 挂载后将 `.app` 复制到全新临时目录再启动；未运行 `target/release` 裸二进制。

| 分发物                                    |                                                                                                                             结果 |
| ----------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------: |
| `YesPlayMusic_0.7.1-canary.1_aarch64.dmg` |                                                                                                   12,421,069 bytes（11.846 MiB） |
| DMG SHA-256                               |                                                               `aef77b48eb649456a3a48f83805d82f350147f3a3d4be377f59557e2a94d080b` |
| 安装后 `.app`                             |                                                                                                         23,124 KiB（22.582 MiB） |
| 相对本 fork v0.7.0 Bun `.app` 82.555 MiB  |                                                                                           约 -72.6%；54.1 MiB hard gate **PASS** |
| 独立完整 Sidecar source asset             | 75,069,552 bytes（71.592 MiB），SHA-256 `06513642e393dcdb02068f3ab95855bc2dc25c887f219e65ddd02d0d8157294a`；不进入 DMG 或 `.app` |

体积比较分为本 fork 的迁移基线和上游官方发行物的外部历史参考：

| 比较对象                             |                           基线 |                           当前 |  降幅 | 用途           |
| ------------------------------------ | -----------------------------: | -----------------------------: | ----: | -------------- |
| 本 fork v0.7.0 Bun `.app`            |       84,536 KiB（82.555 MiB） |       23,124 KiB（22.582 MiB） | 72.6% | 迁移 hard gate |
| 上游 qier222 v0.4.10 官方 arm64 DMG  | 93,085,284 bytes（88.773 MiB） | 12,421,069 bytes（11.846 MiB） | 86.7% | 外部历史参考   |
| 上游 qier222 v0.4.10 挂载后的 `.app` |     217,020 KiB（211.934 MiB） |       23,124 KiB（22.582 MiB） | 89.3% | 外部历史参考   |

上游 v0.4.10 DMG 的 SHA-256 是 `bf7564f451f0e25217015c0f2a83e1385f7a407a42daf0be8d8d992c471160d8`，`hdiutil verify` 通过。其 `.app` 没有 Developer ID 或公证，bundle 不能通过 `codesign --verify --deep --strict`，主 Mach-O 只有 linker ad-hoc 签名。上游数据不用于本 fork 的 matched 性能比较。

本机 bundle 是 adhoc Hardened Runtime 签名：深度严格校验、arm64 host/Sidecar、空 entitlement、无 Bun/payload、Sidecar provenance/source 门禁，以及精确覆盖 328 个 host package 和 44 个 renderer package 的 app-compliance bundle 校验均通过。这是当前目标发布形态；项目不声称具备 Developer ID 身份认证或免 Gatekeeper 提示。

下面的 bundle 检查、smoke 与长时性能数据全部来自上述 checksum 的 `0.7.1-canary.1` DMG，以及它挂载后复制出的同一个临时安装副本；没有以旧 `0.7.0` 二进制的等价性推断替代重测。

冷启动 smoke：

- core：API/Renderer 2.426/2.427 s；5 个样本完整 core RSS mean 95.92 MiB、CPU mean 0%；Sidecar RSS mean 12.11 MiB；
- WebView：WebView event 0.681 s、API/Renderer 1.096/1.097 s；启动/网络阶段 8 个样本完整树 RSS mean 1,077.68 MiB、CPU mean 20.36%，Sidecar RSS mean 21.87 MiB；只作冷启动诊断，不冒充稳态数据；
- 可见暂停稳态结束时 Sidecar `phys_footprint` 8.969254 MiB，进程生命周期 peak 38.188026 MiB；独立隐藏窗口结束时为 9.234924 MiB，peak 39.453674 MiB；
- supervisor 真实强杀 Sidecar PID `1395 → 1404 → 1419 → 1450`，前三次本地 health/player 恢复，第四次按预算停止重启；
- core/WebView smoke 与最终 Cmd+Q 均记录 Sidecar `Some(0)`；最终 host、Sidecar、三个 WebKit PID 与四端口全部回收。

稳态均为 300 个样本、1 秒间隔：

| 场景                 |  完整树 RSS mean / P95 / max | 完整树 CPU mean / P95 / max | Tauri RSS / CPU mean | Sidecar RSS / CPU mean |                Sidecar phys_footprint |
| -------------------- | ---------------------------: | --------------------------: | -------------------: | ---------------------: | ------------------------------------: |
| 可见暂停稳态 0-5 min | 428.65 / 937.89 / 938.27 MiB |           2.35 / 3.8 / 4.8% |    97.89 MiB / 1.07% |      12.30 MiB / 0.01% | 结束 8.969254 MiB；peak 38.188026 MiB |
| 窗口隐藏 0-5 min     | 501.90 / 817.89 / 818.38 MiB |           0.57 / 1.5 / 5.6% |   104.48 MiB / 0.07% |      14.03 MiB / 0.00% | 结束 9.234924 MiB；peak 39.453674 MiB |
| 连续播放 0-5 min     | 399.66 / 496.27 / 508.38 MiB |          3.42 / 6.5 / 38.4% |    97.23 MiB / 1.26% |      12.31 MiB / 0.01% |                   5 分钟 9.516151 MiB |
| 连续播放 5-10 min    | 434.78 / 653.31 / 662.02 MiB |          4.18 / 8.9 / 72.6% |    97.70 MiB / 1.20% |      13.77 MiB / 0.01% |                  10 分钟 9.188049 MiB |

可见暂停稳态在 API、Renderer 与 WebView ready 后开始，采样开始时根进程已运行约 3 分 44 秒。窗口保持可见，播放已暂停；采样前曾误触播放并立即暂停，因此这段数据不能解释为从未触发播放的初始空闲，也不能替代窗口隐藏数据。末帧完整树 RSS 为 309.58 MiB。

窗口隐藏使用独立新启动的 host，PID 为 `9483`，Sidecar 为 `9492`，WebKit GPU/Networking/WebContent 为 `9493/9494/9495`。macOS 窗口隐藏状态经人工确认；末帧完整树 RSS 为 261.08 MiB。Cmd+Q 后 Sidecar 返回 `Some(0)`，5 个 PID 与四端口全部回收。

连续播放 5 分钟点为 `NIGHT DANCER · imase` 2:17，10 分钟点为 `IRIS OUT · 米津玄師` 0:53；两次界面均显示“暂停”按钮，表示仍在播放。第二窗口完整树 RSS mean 比第一窗口高 8.8%，末帧从 351.97 MiB 增至 396.59 MiB；波动主要来自 WebKit，不能归因于 Rust。Sidecar RSS mean 从 12.31 MiB 增至 13.77 MiB，但 `phys_footprint` 从 9.516151 MiB 降至 9.188049 MiB。10 分钟内没有 Sidecar 物理内存持续累积的证据，但这组数据不能证明长期运行不存在泄漏。

逐样本证据可直接复核：

- [可见暂停稳态 0-5 min](./evidence/sidecar-rust-migration/installed-idle-5m.json)
- [窗口隐藏 0-5 min](./evidence/sidecar-rust-migration/installed-hidden-idle-5m.json)
- [连续播放 0-5 min](./evidence/sidecar-rust-migration/installed-playback-0-5m.json)
- [连续播放 5-10 min](./evidence/sidecar-rust-migration/installed-playback-5-10m.json)

每份均为 schemaVersion 3，包含 300 个 `rawSamples`、`processSummaries`、artifact/executable SHA-256、`measurement.rootProcessStartedAt`、逐进程 `startedAt`/`execTokenHash` 和 1 秒间隔检查。JSON 保留采样机的绝对路径；换机器或清理临时安装目录后，必须把同 SHA DMG 复制出的 host 路径显式传给 verifier：

```bash
bun scripts/verify-performance-evidence.mjs \
  docs/evidence/sidecar-rust-migration/installed-idle-5m.json \
  --artifact dist_tauri/YesPlayMusic_0.7.1-canary.1_aarch64.dmg \
  --executable /path/to/YesPlayMusic.app/Contents/MacOS/yesplaymusic-tauri
```

窗口可见或隐藏、播放或暂停状态、曲目、进度与“暂停”按钮是通过 native UI 读取的人工 smoke 观察，不在性能 JSON 内；JSON 独立证明的是该时间窗口的进程、CPU/RSS、时间序列与分发物身份。

这些数据不能直接证明完整树相对 Bun/Electron 降低多少：历史 Electron/Bun 数据没有在同一机器、同一场景重跑。可以确认的是 Rust 后端自身远小于历史 Bun Sidecar 约 82 MB 的 `phys_footprint`；WebKit 仍是全应用内存大头。

## Tauri 验收线

- 隐藏窗口 CPU mean 不高于 2%，且明显低于修复前约 30% 的探索性结果；
- 播放态主进程 CPU mean 不高于 10%（2026-08-10 实测 6%-8%，回归门禁）；
- 完整树内存降幅只在同机同场景的 Bun/Electron 对照完成后判定；当前 **PENDING**，不得拿历史探索性范围计算宣传数字；
- 正常播放 10 分钟内不得出现可归因于 Sidecar 的持续内存累积；RSS 与 `phys_footprint` 联合判断，WebKit 波动不计作后端收益或回归；
- 随机播放、缓存、登录 cookie、托盘歌词不因降内存而回归。

2026-08-11 Rust-only 本机判定：包体积 hard gate **PASS**；隐藏窗口完整树 CPU **PASS**（0.57%），可见暂停稳态完整树 CPU 2.35% 与主进程 CPU 1.07% 只作为可见场景观察值；播放态主进程 CPU **PASS**（1.26% / 1.20%）；Sidecar 5→10 分钟 RSS mean 增加 1.46 MiB，`phys_footprint` 减少 0.328102 MiB，10 分钟物理内存趋势 **PASS**。相对历史 Bun Sidecar 粗略记录的约 82 MB `phys_footprint`，Rust 稳态 8.969254-9.516151 MiB 大致低 88%-89%；历史值没有 raw bytes，也不是同一时刻的 matched run，因此这里只说明后端量级，不用于宣称完整应用内存降幅。完整树 matched baseline 和真实登录 cookie 仍是 **PENDING**；Developer ID/公证按当前发布政策为 **N/A**。
