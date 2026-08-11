# Bun Sidecar → Rust Sidecar 迁移与验收记录

> 状态（2026-08-11）：57-route 静态合同门禁、Rust Sidecar 与 Rust-only 分发接线已在工作树实现；逐路 Node↔Rust differential 尚未实现。冻结在 `1b891bc1059f1b92b090bf08d56a12f63b5ab5a9` 的 `0.8.0-canary.1` pre-tag 本地 adhoc Hardened Runtime DMG 已完成 installed/core/WebView/supervisor、可见暂停、连续播放与窗口隐藏验收。Tag CI、updater 签名资产、canary feed 与真实发布仍为 **PENDING**；Developer ID/公证按当前政策为 **N/A**。Windows/Ubuntu 实验安装包验收仍待完成，90 天观察期尚未开始。
>
> 迁移基线：`b6efe0b850bca17bfd3d9c0ffc9061e6348c4f07`（v0.7.0 的 Bun 发行实现）。90 天起点必须是首个实际发布并投入使用的 Rust-only prerelease 的 tag、发布时间和 artifact SHA；当前为 `TBD`。上述 freeze commit、本地 DMG 及其 SHA 都不计入观察期，也不代表未来 tag CI artifact。

## 一句话结论

**最终发布物只包含一个后端：独立 Rust Sidecar。Bun 不进入 Rust 版安装包，也不存在用户运行时的双后端 fallback。**

迁移完成后的应用仍有两个进程，但只有一个后端实现：

```text
YesPlayMusic Tauri 主程序
  ├─ WebView：Vue / Pinia UI
  └─ spawn + supervise
       └─ YesPlayMusic Rust Sidecar
            ├─ 127.0.0.1:28232：Renderer 与同源 /api
            ├─ 127.0.0.1:12754：API（dev 与 release 均使用）
            ├─ 127.0.0.1:27232：/player 兼容 API
            └─ 127.0.0.1:27233：配置上游代理时才启用的 relay
```

这里保留的是 **Sidecar 进程边界**，不是 Bun：当前 workspace release profile 的 `panic = "abort"` 同时适用于 host 与 Sidecar；边界的价值不是把 Sidecar panic 变成单请求 500，而是 Sidecar 即使 abort，Tauri UI 仍存活、显示错误并按有限预算重启后端。不会把网络协议、UNM、转码和代理代码塞进主进程。现有 first-party handler 审计未找到用户输入可达的 panic；若后续确认依赖存在可复现 panic 输入，应把 Sidecar 拆成独立 unwind profile，而不是依赖 supervisor 掩盖。

## 当前阶段状态

| 范围                                 | 状态                     | 可复核证据                                                                       | 尚缺门禁                                                                             |
| ------------------------------------ | ------------------------ | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------ |
| 57-route inventory / static contract | **PASS**                 | `sidecarRouteManifest.test.ts`、manifest-driven Rust router                      | 无逐路 Node↔Rust request/decoder/comparator differential；不能称为 differential 完成 |
| Rust Sidecar 行为与接入              | **PASS**                 | Rust router/upstream/proxy/cloud/UNM/lifecycle 行为测试，Rust `externalBin` 构建 | live 账号写入只允许作为手工 smoke，不进入 required CI                                |
| macOS Rust-only 本地候选包           | **PASS（目标发布形态）** | 本文“macOS installed artifact 实测”及性能基线                                    | tag CI 重建与 updater/canary endpoint 验收、真实 canary 发布                         |
| Windows x64 / Ubuntu x64             | **PENDING（实验构建）**  | cross-build/配置与模块测试                                                       | NSIS 实装/卸载、AppImage 全新 XDG、deb 实装与 release WebView smoke                  |
| 90 天 canary 与删除 Bun              | **NOT STARTED**          | 无                                                                               | 首个 prerelease、artifact SHA、每周记录、协议变化演练和连续 90 天                    |

这里的“阶段 1/2/3”是迁移检查点，不是已经存在的三个 Git PR。当前改动仍在同一工作树；真正拆 PR 时必须补 PR URL、commit range 与各自独立门禁结果。

### 支持与阻塞语义

- Apple Silicon macOS 是正式支持平台：DMG 的 installed/WebView/performance/provenance 门禁必须通过。当前目标发布形态是 adhoc Hardened Runtime，不要求 Developer ID 或公证。
- Windows x64、Ubuntu x64 是实验构建：不暗示正式支持，但实际分发的实验包仍要完成基本安装、启动、退出和卸载检查。
- adhoc seal、Hardened Runtime、空 entitlement 和 provenance 是当前发布门禁；它们不提供 Developer ID 身份认证。macOS 首次打开的 Gatekeeper 手工放行步骤已写入 README，属于已接受的发布体验。

## 安装包唯一后端规则

任何面向用户或 canary 的单个安装包都只能包含一种后端：

| 阶段                                                   | 仓库中的实现        | 安装包中的后端 | 包体积收益                                                                   |
| ------------------------------------------------------ | ------------------- | -------------- | ---------------------------------------------------------------------------- |
| 迁移基线                                               | Bun                 | Bun            | 基线                                                                         |
| 阶段 1                                                 | Bun + Rust 合同实现 | Bun            | 无；Rust 只作为测试实现                                                      |
| 阶段 2 canary candidate（当前工作树 `0.8.0-canary.1`） | Bun 参考源码 + Rust | **仅 Rust**    | macOS 本机 `.app` 已从 v0.7.0 的 82.555 MiB 降至 22.976563 MiB；跨平台待验收 |
| 阶段 3 正式收口                                        | Rust                | **仅 Rust**    | 需先满足 90 天门禁                                                           |

禁止以下形态：

- 同一个安装包同时携带 Bun 和 Rust 后端；
- 请求失败后从 Rust 自动重放到 Bun；
- Rust 后端健康检查失败时，在用户机器上临时下载或启动 Bun；
- 按路由混用 Rust 与 Bun，例如云盘走 Bun、其余走 Rust。

回滚依靠上一个已发布版本、Git revert 和重新发布，不依靠把第二套运行时塞进安装包。

## 重要链接

- [Tauri：嵌入外部二进制文件（Sidecar）](https://v2.tauri.app/zh-cn/develop/sidecar/)
- [Tauri：Node.js 作为 Sidecar](https://v2.tauri.app/zh-cn/learn/sidecar-nodejs/)
- [`SPlayer-Dev/ncm-api-rs`](https://github.com/SPlayer-Dev/ncm-api-rs)：Rust 网易云 API SDK/服务器
- [`ncm-api-rs` crates.io](https://crates.io/crates/ncm-api-rs)
- [`UnblockNeteaseMusic/server-rust`](https://github.com/UnblockNeteaseMusic/server-rust)：当前 UNM N-API 的 Rust 源头
- [`unm_engine` crates.io](https://crates.io/crates/unm_engine)
- [`unm_api_utils` crates.io](https://crates.io/crates/unm_api_utils)
- [Apple：Hardened Runtime](https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime/)
- [Apple：Entitlements](https://developer.apple.com/documentation/bundleresources/entitlements)
- [qier222/YesPlayMusic](https://github.com/qier222/YesPlayMusic)：Electron 功能对照基线
- [Bun 参考 Sidecar 入口](../src/sidecar.ts)
- [Bun 参考构建脚本](../scripts/build-sidecar.mjs)
- [Rust Sidecar](../src-tauri/sidecar/src/main.rs)
- [Rust Sidecar 构建脚本](../scripts/build-rust-sidecar.mjs)
- [可执行路由 manifest](../src/sidecar-route-manifest.json)
- [Tauri/Rust 主进程](../src-tauri/src/main.rs)
- [Electron → Tauri 功能迁移表](./feature-migration.md)
- [性能基线](./performance-baseline.md)

## 基线架构与迁移边界

```text
WebKit / WebView2 / WebKitGTK
          │ HTTP: 127.0.0.1:28232
          ▼
      Bun Sidecar
          ├─ Renderer 静态资源
          ├─ /api 同源代理
          ├─ 网易云 API
          ├─ UNM
          ├─ 无损精确 seek
          ├─ WebView 代理 relay
          └─ /player 兼容 API
          ▲
          │ spawn / health / restart / shutdown
      Tauri/Rust 主进程
```

迁移只替换中间的 Bun 可执行文件。Tauri 主进程继续拥有 Sidecar 生命周期；WebView origin、端口、数据目录和前端请求合同不变。仓库暂留 Bun 源码作为 differential oracle 和可回退的 Git 历史，但默认构建、签名和安装包资源均不引用它。

## 必须完整迁移的功能

| 职责          | 当前行为                                                                                            | Rust 最终要求                                                                                                                                                                                                                                    |
| ------------- | --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| 网易云 API    | `ncmModDef.cjs` 注册 124 条；产品实际调用恰好 57 条（55 条静态路径 + `/login`、`/login/cellphone`） | 显式 manifest 精确覆盖产品 57 条；另保留 smoke 所需内部 `/login/status`，不把未使用的 67 条伪装成产品合同                                                                                                                                        |
| 云盘上传      | multipart、metadata、MD5、NOS、info、publish                                                        | 完整 Rust 状态机；不能留给 Bun，也不能删除功能                                                                                                                                                                                                   |
| UNM           | ytdl、Bilibili、pyncm、酷狗等替代音源                                                               | 直接使用经过锁版本与许可证审查的 Rust crates；行为保持一致                                                                                                                                                                                       |
| Renderer 服务 | `127.0.0.1:28232` 托管 Vue 产物；桌面端使用 `createWebHashHistory()`                                | Rust 托管同一 origin、目录 index、缓存头和安全头；未知真实路径保持 404，不新增无用的 SPA history fallback                                                                                                                                        |
| 同源 API      | `/api` 反向代理，保留 SameSite cookie                                                               | 完整保留多 Set-Cookie、刷新、登出和跳转语义                                                                                                                                                                                                      |
| 无损精确 seek | macOS 流式 FLAC → WAV，支持 Range；Windows/Linux 返回 501 后由 renderer fallback                    | macOS 保留原 bit depth、不重采样及 Range；Windows/Linux 明确保持 501/fallback 合同                                                                                                                                                               |
| WebView 代理  | HTTP、HTTPS CONNECT、连接回收                                                                       | Rust 实现同等代理语义和资源上限                                                                                                                                                                                                                  |
| 播放器 API    | `127.0.0.1:27232/player` 与 `28232/player`                                                          | 地址、响应结构、缓存头和退出回收一致                                                                                                                                                                                                             |
| 安全边界      | Origin、native token、cookie 加固、logout                                                           | 外部输入严格解析，token 和 origin 校验不弱化                                                                                                                                                                                                     |
| 生命周期      | 健康检查、父进程监控、崩溃重启、端口回收                                                            | supervisor 保留健康握手与有限重启；主程序主动退出时通过 stdin NUL sentinel 请求 graceful shutdown，意外断开仍以 EOF 兜底；按 PID+generation 最多等待 7 秒，Sidecar 最多排空 listener 5 秒，超时后通过仍持有的 `CommandChild` 强制终止并再等 2 秒 |

## 不可破坏的产品契约

1. 正式版 origin 保持 `http://127.0.0.1:28232`，不切换到 `app://`。
2. dev 保持 `127.0.0.1:1420` 与同源 `/api` 语义。
3. 保留 `127.0.0.1:27232/player` 和 `28232/player` alias。
4. 同一时刻只有一个进程拥有 `12754/27232/27233/28232`；正式版唯一 owner 是 Rust Sidecar。
5. 保留 WebView 代理的 HTTP/HTTPS CONNECT 语义。
6. 不改变登录 cookie、localStorage、IndexedDB 和旧版数据迁移边界。
7. 五档音质 wire value 全部保留：128000、192000、320000、350000（FLAC）、999000（Hi-Res）。
8. FLAC → WAV 只用于精确 seek，必须保留原 bit depth，不允许隐式重采样。
9. UNM 音源、顺序、excludedSources、proxy、enableFlac、searchMode 和 Bilibili 特殊处理保持一致。
10. 外部响应仍以 `unknown` 接收，并通过现有 TypeScript decoder 缩窄。
11. 非幂等请求不得跨后端自动 retry；正式 Rust 版没有 Bun 后端可重放。

## 目标代码结构

Rust Sidecar 建成独立 Cargo package，而不是 `main.rs` 内的服务模块：

```text
src-tauri/
  sidecar/
    Cargo.toml
    src/
      main.rs
      server.rs
      ncm.rs
      cloud.rs
      unm.rs
      session.rs
      renderer.rs
      precise_wav.rs
      proxy_relay.rs
      player_api.rs
      health.rs
      config.rs
      lib.rs
  src/main.rs                 # 只负责 spawn、supervise、窗口与原生功能
  binaries/                   # 构建产物，不手工提交平台二进制
scripts/
  build-rust-sidecar.mjs      # 按 target triple 构建并放入 Tauri externalBin 位置
  build-sidecar-compliance.mjs # 生成轻量随包 notice 与独立完整源码/relink kit
  build-app-compliance.mjs    # 生成 target-specific host/renderer 精确许可证闭包
  verify-packaged-app-compliance.mjs # 解包并校验最终 NSIS/AppImage/deb 合规资源
  sidecar-route-manifest.mjs  # 从前端与 Bun adapter AST 验证产品路由集合
test/
  appCompliance.test.ts
  sidecarRouteManifest.test.ts
  sidecarCompliance.test.ts
```

HTTP、cookie、云盘、UNM、音频和 proxy 的行为测试与对应 Rust 模块同目录，直接驱动 router、stream 和 mock upstream；不另建只检查源码字符串或文件存在性的“迁移完成”墓碑测试。Rust Sidecar 与 Tauri 主进程沿用 stdin identity token、父 PID 和健康检查合同。后端异常达到重启预算后，Tauri 显示明确错误，不静默降级到另一实现。

## 实施步骤

这项工作超过 8 个文件并新增一个服务进程，按三个可独立验收的阶段推进。当前工作树同时包含阶段 1 与阶段 2 的实现，尚未形成三个独立 Git PR；以下状态只按可运行证据判断。

### 阶段 1：合同门禁 + 测试端口上的 Rust Sidecar（部分完成）

产品行为不切换，正式安装包仍只包含 Bun。

- 新建独立 Rust Sidecar package。HTTP/router、上游和 proxy 行为测试使用操作系统分配的临时端口，不争抢生产端口；只验证固定兼容端口的 packaged smoke 留到阶段 2。
- 锁定 `ncm-api-rs`、UNM crates 及所有直接协议依赖的确切版本。
- 建立可执行 route manifest：当前 55 条静态路径，加 `/login`、`/login/cellphone`，恰好 57 条；CI 断言生产请求路径集合与 manifest 完全相等。`/login/status` 是分发 smoke 的内部路由，不混入产品 manifest。
- manifest 每项明确 HTTP method、path、request builder、Node adapter、Rust adapter、生产 decoder 和 comparator 名称；当前静态门会校验路径、method、builder、Node adapter、decoder 与 Rust adapter allowlist。
- **PENDING：**逐路启动 Node 与 Rust、执行同一个生产 TypeScript decoder 并调用 comparator 比较稳定字段的 differential harness 尚未实现。现有 Rust 代表性行为测试不能替代 57-route differential。当前仓库没有保留可执行 spike；要完成该门禁，仍需为 41 种 decoder 建立真实脱敏响应 corpus，并为多数 WEAPI 请求补出 SDK transport 前可观察的明文边界。这里没有用 comparator 自动拼出万能 JSON 来制造假绿色。
- 云盘使用 mock upstream 验证 multipart → MD5 → NOS → info → publish，避免对真实账号重复写入造成假差异。
- UNM 使用固定 provider fixture 验证 source order、代理、FLAC、搜索模式和 Bilibili headers/base64；执行 case 数为零或依赖缺失时必须失败，不能显示绿色 skip。
- 修复当前发行物的 UNM 许可证/source/build-instruction 缺口：生成器从锁定依赖闭包构建 notice、校验和、对应源码和 relink kit，并在闭包变化时失败。
- 阶段 1 的概念检查点仍使用 Bun 正式安装包；当前工作树已继续进入阶段 2，默认构建不会发布 Bun Sidecar。

阶段 1 当前独立价值是锁定 57 条产品路由清单、构建 Rust 行为测试和合规闭包。完整 cookie jar 链与五档共享 case table 已补齐；由于逐路 differential 仍缺真实脱敏 response corpus，不能称为“完整 API differential 门禁”。

阶段 1 的 cookie 行为链“ephemeral frontend → `/api` → ephemeral Rust”已通过：测试真实启动两个临时 listener，使用 cookie jar 验证多 `Set-Cookie`、下一请求重放、refresh 轮换、native logout 过期和清空后不再携带。renderer 的 301 expiry interceptor 另有行为测试，确认清本地会话并返回账号登录页。真实 `28232 → /api → 12754` 已由阶段 2 packaged smoke 验证基础 API 路径；真实账号登录仍只作为 canary 手工 smoke，不写入 fixture 或 required CI。

### 阶段 2：Rust-only canary candidate（接线完成，发布门禁未清零）

这一阶段开始获得真实体积和运行内存收益。每个 canary 安装包只包含 Rust Sidecar。

- Rust Sidecar 接管 Renderer、`/api`、NCM、云盘、UNM、precise WAV、proxy relay、logout、安全边界和 `/player`。
- Tauri `externalBin` 从 Bun wrapper 改为按 target triple 构建的 Rust Sidecar。
- Tauri supervisor 继续负责 spawn、健康检查、有限重启和端口回收。主动退出通过 stdin NUL sentinel 请求 Sidecar drain，EOF 仍处理父进程异常消失；host 保留 `CommandChild` 到收到 matching termination，超时才通过该句柄强制终止，避免对可能复用的裸 PID 发信号。
- Bun 源码暂时留在仓库，作为未来 differential oracle 与快速 Git revert 的参考；当前 CI 尚未执行 Node↔Rust differential。默认构建、签名和安装包都不复制 Bun。
- `bun run dev:tauri` 默认启动 Rust Sidecar；`bun run build:sidecar:bun-reference` 只在显式调用时构建 Node 参考实现，当前 CI/test 不会自动启动它做 differential。
- 正式运行模式只有 `rust-strict`。不存在 `rust-with-fallback`，启动失败时显示错误并退出。
- macOS 主程序与 Rust Sidecar 分开签名，并断言以下五项均不存在：
  - `com.apple.security.cs.allow-jit`
  - `com.apple.security.cs.allow-unsigned-executable-memory`
  - `com.apple.security.cs.disable-executable-page-protection`
  - `com.apple.security.cs.disable-library-validation`
  - `com.apple.security.cs.allow-dyld-environment-variables`
- 清完 differential、真实账号手工 smoke 等 canary 发布门禁后，下一步才是发布 rust-only prerelease，而不是覆盖 current latest；旧 Bun 正式版继续作为可下载回滚版本。Developer ID/公证按当前政策不属于门禁；当前本地 adhoc DMG 仍不是已发布 canary。

阶段 2 的回滚方式：revert 切换提交或让用户安装上一个已发布 Bun 版本。用户数据不迁移、不删除，因此回滚不触碰 localStorage、IndexedDB 或 cookie。

### 阶段 3：正式发布与删除 Bun 实现（未开始）

满足下面的观察门禁后，才将 Rust-only 版本发布为 latest：

当前工作树不满足这组时间门禁。即使所有自动测试和本地安装包 smoke 都通过，也只能进入 canary，不能删除 Bun 参考实现或发布为 latest。

- 连续 90 天使用 rust-strict canary；无无法解释的账号丢失、音质降级、数据破坏或后端崩溃。
- Apple Silicon macOS 每周 canary smoke 全绿；Windows x64、Ubuntu x64 每次相关提交 CI 全绿。
- 至少处理一次真实网易协议变化；若 90 天内没有变化，则用固定旧版 JS 基线与主动更新的 JS reference 做一次协议漂移演练。
- 协议变化从确认 JS reference 修复到 Rust canary 恢复的目标时间不超过 72 小时。
- 任何回退到旧 Bun 发行版都会重新开始 90 天观察期。

#### canary observation log

| 字段                            | 当前值                                                                                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------------------ |
| pre-tag 本地候选包              | `0.8.0-canary.1` @ `1b891bc1059f1b92b090bf08d56a12f63b5ab5a9`；DMG SHA `372f921c…0bd08e`；不计入 90 天 |
| 首个 Rust-only prerelease/tag   | `TBD`                                                                                                  |
| 发布 artifact SHA / release URL | `TBD`；不以本地 DMG SHA 预填                                                                           |
| tag CI / updater / canary feed  | **PENDING**；需用 updater secrets 重建并验收 canary endpoint、archive 和签名                           |
| 观察起止时间                    | `NOT STARTED`                                                                                          |
| 每周 installed smoke            | `NOT STARTED`；当前 workflow 无定时任务                                                                |
| 真实 incident / 协议变化        | 无记录；本地工作树测试不计入 90 天                                                                     |

后续每条观察必须记录构建 SHA、安装方式、脱敏步骤与结果。只写“本周正常”或依赖 required CI 之外的真实账号请求，均不能作为删除 Bun 的证据。

收口内容：

- 删除 `src/sidecar.ts` 及只服务于 Bun 后端的 TypeScript 服务、构建脚本、N-API 和 Node server/proxy 依赖。
- 删除 Bun external binary、wrapper、payload 和相关签名逻辑。
- 保留 Rust Sidecar、Tauri supervisor、health/restart/shutdown 协议。
- 将合同 oracle 从仓库内 Bun 实现改为：固定脱敏 fixtures + 锁定版本的活跃 JS reference canary；正式测试不依赖远程服务才能通过。
- 更新架构文档、许可证清单、性能基线和 release 正文。

## 验收门禁

下表区分“代码/模块测试”和“真实分发物”。`PARTIAL` 与 `PENDING` 不能计入 canary 发布 green：

| 门禁                                               | 状态（2026-08-11）           | 证据 / 缺口                                                                                                                                                                                               |
| -------------------------------------------------- | ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 57-route inventory 与 Rust router                  | **PASS**                     | manifest CLI/tests + Rust manifest-driven router                                                                                                                                                          |
| 57-route Node↔Rust decoder/comparator differential | **PENDING**                  | 没有逐路执行 harness；comparator 当前只是 manifest 字段                                                                                                                                                   |
| Cookie/安全边界                                    | **PASS（hermetic）**         | 双临时 listener + cookie jar 覆盖登录、重放、refresh、301 expiry 与 logout；真实账号只做 canary 手工 smoke                                                                                                |
| 云盘                                               | **PASS（hermetic）**         | multipart/MD5/去重/阶段失败/auth/timeout、真实 MP3 ID3 与 FLAC Vorbis Comment 均覆盖；live 写入只做手工 smoke                                                                                             |
| UNM/proxy/precise WAV/player                       | **PASS（hermetic）**         | Rust 行为测试；UNM CLI 以本地 fixture engine 真实执行 production `Executor` 的 search→retrieve；官方 provider transport 与 live codec/bit-depth/source smoke 仍属手工发布检查                             |
| Supervisor                                         | **PASS**                     | Rust 行为测试覆盖 generation/health/shutdown race；macOS 真实进程覆盖端口占用、四代 SIGKILL、三次恢复与 restart-budget exhaustion                                                                         |
| 应用 host/renderer 第三方合规                      | **PASS（macOS 本地候选包）** | `aarch64-apple-darwin` manifest 精确覆盖 328 个 host normal package 与 44 个 renderer final-chunk package；bundle 文件和 `SHA256SUMS` exact match；Windows/Linux artifact gate 已接线，真实实验包仍待构建 |
| macOS adhoc DMG installed artifact                 | **PASS（pre-tag 本机）**     | SHA、挂载后复制、arm64、bundle seal、无 Bun、Sidecar source/provenance、app-compliance、core/WebView/API、可见/隐藏/播放性能、退出回收                                                                    |
| Tag CI / updater 签名资产 / canary feed            | **PENDING**                  | pre-tag 本地包公钥为空、endpoint 为 stable latest，且没有 updater archive/`.sig`；需用 secrets 重建并验收 canary endpoint 和签名                                                                          |
| Developer ID / notarization（当前政策不采用）      | **N/A（非门禁）**            | 当前发布政策采用 adhoc Hardened Runtime；CI 分支只保留为未来可选能力，不影响当前发布判定                                                                                                                  |
| Windows NSIS / Linux AppImage+deb installed smoke  | **PENDING（实验）**          | CI 仍有 raw exe、core-only 或解包 Sidecar 路径                                                                                                                                                            |
| 90 天 canary                                       | **NOT STARTED**              | 无 prerelease/tag/artifact SHA/观察记录                                                                                                                                                                   |

### API 与 decoder

- route manifest 与生产调用集合完全相等，至少覆盖当前 57 条路径。
- 最终 differential 中每个 case 的 Node、Rust、decoder、comparator 执行次数均须大于零；当前该项是 **PENDING**。
- QR、手机号、邮箱、cookie refresh、logout、session expiry 全部通过。
- 搜索、歌单详情、喜欢/取消喜欢、创建歌单、歌单写入和每日推荐通过。
- raw JSON 不做脆弱的全量深比较；两边先经过同一生产 decoder，再比较稳定语义字段。
- 测试日志和 artifact 自动脱敏 Set-Cookie、账号标识和 URL query。

### Cookie 真实链路

两层门禁都不能只直连后端端口：

- 阶段 1 hermetic：ephemeral frontend → `/api` → ephemeral Rust，允许并行且不占用户端口；
- 阶段 2 packaged：真实 `28232 → /api → 12754`，验证安装包、WebView origin 与固定端口合同。

- 多个 Set-Cookie 不折叠；
- `MUSIC_U` 与 `__csrf` 属性正确；
- 下一请求会携带 cookie；
- refresh 能轮换 cookie；
- 301/过期场景不会保留无效登录；
- logout 清理 cookie；
- macOS、Windows、Linux 的 release WebView 各做一次真实 smoke；当前只有 macOS adhoc installed WebView smoke 完成。

### 云盘

- hermetic 测试已覆盖 multipart 的 MP3/FLAC 文件名、UTF-8 filename metadata fallback、真实 MD5 去重和各 NCM/NOS 中间失败分支；另用 FFmpeg 8.1 独立生成、固定 checksum 的真实 MP3 ID3 与 FLAC Vorbis Comment fixture 验证中文 title/album/artist，不使用被测 `lofty` writer 自证 reader。
- 端到端请求上限为 370 秒，略高于 multipart 60 秒与后续阶段 300 秒的预算总和；WebView relay 使用按 I/O 活动续期的 380 秒 loopback idle timeout，前端在 400 秒停止等待，保证服务端先给出确定响应。外部 CONNECT/HTTP 隧道仍使用 120 秒 activity-based idle timeout，连接池满载时快速返回 503。
- 云盘全流程共享一个上传槽；第二个并发请求会在读取 multipart body、创建临时文件或进入云端状态机前返回 HTTP 429，避免多个 512 MiB 临时文件叠加。
- live smoke 只上传一次，再验证 list/detail、metadata、下载内容 hash，并在 `finally` 删除测试文件。
- live secret 缺失时报告“未执行”，不能计入 required CI green；required CI 使用 mock upstream，永远可执行。

### 音质与播放

- 共享 case table 覆盖 128000、192000、320000、350000、999000 五个 wire value；生产 TypeScript `getMP3` 与 Rust `/song/url` query parser 都读取同一份 fixture，不允许某档静默降级。
- live smoke 下载少量真实字节，检查容器、codec、bitrate、bit depth 和 sample rate。
- UNM provider search/retrieve 使用 15 秒预算；Bilibili 二跳下载另有 120 秒总预算与 30 秒无进度 read timeout，并同时校验 `Content-Length` 与逐 chunk 累计大小。64 MiB 以上立即走现有 provider fallback；下载到 Base64 编码全阶段最多并发 2 个，第三个等待容量且仍受 120 秒总预算与请求取消约束，避免多个原始响应与 Base64 副本无界叠加。
- required CI 的 `--unm-smoke-test` 同时检查完整 provider registry/link，并通过本地 fixture engine 真实执行 production `Executor` 的 search→retrieve 与 Sidecar resolve 链路。上游官方 provider 在内部直接创建 HTTP client，没有 transport 注入钩子，因此这项 hermetic smoke 不代表真实 provider 网络协议已验证；该层仍由 canary 手工 live smoke 覆盖。
- precise WAV 比较 PCM、bit depth、Range seek；不允许隐式重采样。
- UNM live smoke 只比较允许源、媒体可读性与容器等稳定语义；动态签名 URL 不做字符串相等比较。

### 生命周期与错误

- Rust Sidecar 启动失败、panic、端口占用、代理断开、网易云超时不会使 Tauri UI 无提示退出。
- 非幂等请求失败后不跨后端重放。
- supervisor 的重启预算、健康 token、PID identity 和 shutdown race 有行为测试；Windows release Sidecar 的 GUI subsystem 由 PE 产物 parser gate 验证，不用 Rust 源码字符串充当行为证据。
- 主程序主动退出会写入 stdin NUL sentinel，Sidecar 收到后开始 drain；父进程异常消失时 EOF 触发同一路径。host 按对应 PID+generation 最多等待 7 秒，Sidecar 最多 drain 5 秒，超时后通过仍持有的 `CommandChild` 强制终止并再等 2 秒。sentinel 没有独立 ack，最终 `Terminated` 事件才是完成证据。
- 本机 installed smoke 已观察到 Sidecar `Some(0)`、真实强杀后的新 PID/纯本地 health+player 恢复，以及正常退出后所有相关进程消失。退出前 `12754/27232/28232` 在监听，`27233` 因未配置代理始终没有 listener；退出后四个端口均无 listener。
- 当前 macOS pre-tag 本地候选 DMG 的全新临时安装副本已完成端口占用与 restart-budget exhaustion 的真实 Tauri adverse smoke。真实 `SIGKILL` 会进入生产 `CommandEvent::Terminated` 与 restart-budget 路径，因此没有新增 synthetic `--panic` test hook；Windows/Linux 的真实分发物状态由独立门禁行记录。
- 退出后 `12754/27232/27233/28232` 无残留 listener。

#### macOS supervisor adverse smoke（2026-08-11）

`bun run smoke:tauri:supervisor` 默认启动当前 arm64 release bundle。已安装或从 DMG 复制出的候选包可显式指定实际 host executable：

```bash
YPM_TAURI_SMOKE_EXECUTABLE="/path/to/YesPlayMusic.app/Contents/MacOS/yesplaymusic-tauri" \
  bun run smoke:tauri:supervisor
```

脚本只会向本轮创建的 Sidecar 发送 `SIGKILL`。每次发送前同时验证测试 host 仍是预期 executable、Sidecar 是该 host 的直接子进程、Sidecar executable 精确匹配，且命令行 `--parent-pid` 指回该 host；任一条件变化都会拒绝发送信号。端口占用场景使用脚本自己持有的 `127.0.0.1:28232` listener。

checksum 为 `372f921c…0bd08e` 的 pre-tag 本地 DMG 全新临时安装副本实测结果：

| 场景                     | 观察结果                                                                                                                                                  | 判定     |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- | -------- |
| 四个并发冷启动           | primary host/Sidecar PID `78260/78264`；另外三个实例把焦点交给 primary 并以 0 退出                                                                        | **PASS** |
| 预占 `28232` 后启动      | host PID `78302`；四次 Sidecar 启动均命中真实端口冲突，随后报告 restart budget exhausted 并以非零状态退出；无 Sidecar/端口残留                            | **PASS** |
| 连续强杀四代 Sidecar     | host PID `78317`，Sidecar PID `78320 → 78329 → 78348 → 78380`；前三次新 PID 的纯本地 `/__yesplaymusic/health` 与 `/player` 合同恢复，第四次后耗尽重启预算 | **PASS** |
| 第四代后的 unavailable   | 输出“后台服务已停止，自动重启失败。请重启应用。”；直到 host 自动退出，未出现第五代 Sidecar，本地服务未恢复                                                | **PASS** |
| 两个场景退出后的资源回收 | fixture 持有 `28232` 时 `12754/27232/27233` 均无 listener；关闭 fixture 后四端口均无 listener，也没有对应 `--parent-pid` 的 Sidecar                       | **PASS** |

这是运行产物与真实进程的行为证据；配套 Bun 测试只验证 executable/parent 身份筛选，未用 Rust 源码字符串替代上述 smoke。

### 分发物 smoke

四种分发物分别冷启动，不以 raw binary 代替安装包：

- macOS DMG：挂载，启动其中 `.app`，验证 nested Rust Sidecar adhoc seal、Hardened Runtime、后端 provenance 与退出回收；只有显式启用可选 Apple 签名分支时才验证公证/staple。
- Windows NSIS：临时 current-user 静默安装、启动、验证 WebView/API，再静默卸载。
- Linux AppImage：全新 `XDG_CACHE_HOME` 启动，验证资源、Rust Sidecar、API 与退出回收。
- Linux deb：临时容器或 VM 内真实安装，启动主程序和 WebView；不能只解包运行 Sidecar。

每个 smoke 都必须断言：Rust Sidecar 存在且架构正确，Bun runtime/旧 payload 不存在，Renderer 和 API 可用，updater archive 内容与正式安装包一致。

当前证据：`0.8.0-canary.1` pre-tag 本地 DMG 已挂载并复制到全新临时目录，完成上述
runtime/资源检查；Developer ID/公证按当前政策为 **N/A**。本地构建未注入 updater
secrets，包内公钥为空、endpoint 为 stable latest，`dist_tauri/` 没有 updater archive/`.sig`。Tag CI
需要重新构建并验收 canary endpoint、artifact 内嵌版本、updater 签名及 installer 逐文件一致性，
当前均为 **PENDING**。本地 DMG SHA 不代表未来 CI artifact。三个实验分发物也仍为
**PENDING**。现有 CI 中 macOS 运行 build bundle 内 `.app`，Windows 运行 raw release exe，deb 只解包运行
Sidecar，三平台均以 core-only 为主；这些路径不能冒充完整 installed WebView smoke。

### 性能与体积

沿用 [`performance-baseline.md`](./performance-baseline.md) 的完整进程树口径：

- Bun v0.7.0 正式 Release 的 `.app` 是 84,536 KiB（82.555 MiB）；阶段 2 rust-only hard gate 为不高于 54.1 MiB（相对该精确基线约降低 34.5%）。构建脚本现会在打包/收集前用 `du -sk` 执行 hard gate。
- 当前 `.app` 是 23,528 KiB（22.976563 MiB），较本 fork v0.7.0 基线低 72.168%。上游 qier222 v0.4.10 官方 arm64 DMG 为 93,085,284 bytes，挂载后的 `.app` 为 217,020 KiB；当前分别低 86.516% 和 89.159%。上游数据只作外部历史参考，详细 checksum 与签名状态见 [`performance-baseline.md`](./performance-baseline.md)。
- 30-40 MiB 是 stretch target，不作为首次切换 blocker。
- 分别记录 macOS `.app`、Windows installed directory、Linux AppDir/deb installed root；下载压缩包大小单独记录。
- 后端 phys_footprint 以当前 Bun 约 82 MB 为基线；采样冷启动、空闲 5 分钟、播放 5 分钟和播放 10 分钟泄漏趋势。
- 不用单次 RSS 代替完整进程树数据，也不把 WebKit/WebView2/WebKitGTK 内存算成 Rust 后端收益。
- Tauri 主进程 CPU 是独立门禁，不允许用 Sidecar 重写掩盖回归。

macOS 本地验收必须从 DMG 挂载后，把其中 `.app` 复制到新的临时安装目录再启动；不能直接运行 `target/release` 裸二进制。记录以下证据：

| 场景               | 必测数据                                                                                                                            |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------- |
| 分发物             | DMG 字节数、安装后 `.app` 的 `du` 大小、Sidecar 架构/签名、无 Bun/payload、Sidecar 与 app-compliance manifest/`SHA256SUMS` 精确匹配 |
| 冷启动             | 从进程创建到 health、Renderer、WebView ready 的时间；完整进程树 RSS/CPU 与 Sidecar `phys_footprint`                                 |
| 空闲 5 分钟        | Tauri 主进程、Rust Sidecar、WebKit 子进程分别记录 CPU；完整树内存与 Sidecar `phys_footprint`                                        |
| 真实播放 5/10 分钟 | 连续真实可播放媒体，记录曲目/进度、CPU、完整树内存、Sidecar 内存趋势，不把 WebKit 算作后端收益；自动切歌必须仍处于播放态            |
| 退出               | 主程序和所有子进程消失；`12754/27232/27233/28232` 均无 listener                                                                     |

失败项必须保留为 blocker，不能以 debug、裸 Sidecar 或单次 `ps` RSS 替代。

### macOS installed artifact 实测（2026-08-11）

环境：Apple M5 Pro / arm64、macOS 26.4.1（25E253）、Bun 1.3.12、Rust 1.89.0。测试对象是冻结在
`1b891bc1059f1b92b090bf08d56a12f63b5ab5a9` 的 `0.8.0-canary.1` pre-tag 本地 DMG。
将挂载后的 `.app` 复制到全新临时目录再测试，未运行 `target/release` 裸二进制。

#### 分发物与启动

| 项目                       | 实测                                                                                                                                                                    | 判定                                                                          |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------- |
| DMG `0.8.0-canary.1`       | 12,551,495 bytes（11.970038 MiB）；SHA-256 `372f921c199f4d8b29534c24d61bef5a49546582177a2b94668ee0e6ea0bd08e`；`hdiutil verify` 通过                                    | **PASS（pre-tag 本地候选包）**                                                |
| 安装后 `.app`              | 23,528 KiB（22.976563 MiB）                                                                                                                                             | **PASS**；低于 54.1 MiB hard gate，较 v0.7.0 的 82.555 MiB 低 72.168%         |
| 完整 Sidecar source asset  | 75,084,239 bytes（71.605910 MiB）；SHA-256 `bb1bc56c406c1316b24d240ef17dc9a5af1c5a1f89c17984e4ccb3f5722e46d9`                                                           | 独立 release asset，不进入 DMG/`.app`                                         |
| `SOURCE-OFFER`             | 905 bytes；SHA-256 `abd3ff34ae98d24acf9190150f2938c5834ca1cee08005bcffd76b760b55be29`；与 `.app` 内指引一致                                                             | **PASS**                                                                      |
| Updater 签名资产           | 本机未配置发布私钥，未生成 updater archive/`.sig`；包内公钥为空，endpoint 为 stable latest                                                                              | **PENDING**；tag CI 需用 secrets 重建并验收 canary endpoint 与签名            |
| 主程序 / Sidecar           | 两者均为 thin arm64 Mach-O                                                                                                                                              | **PASS**                                                                      |
| bundle seal / entitlements | `codesign --verify --deep --strict` 通过；host/Sidecar 均为 adhoc Hardened Runtime 且 entitlement 为空                                                                  | **PASS（当前目标发布形态）**；Developer ID/公证/staple 为 **N/A（当前政策）** |
| Sidecar provenance/source  | 无 Bun/runtime/payload；Rust Sidecar、GPL/LGPL、354 项 notices、13 份 copyleft source、manifest/relink kit 存在，marker 为 `YPM_RUST_SIDECAR_V1`                        | **PASS**                                                                      |
| 应用合规资源               | target manifest 覆盖 328 个 host package 和 44 个 renderer package；342 项许可证文本来自 package，30 项来自 pinned curated donor；bundle bytes/`SHA256SUMS` exact match | **PASS**                                                                      |
| core cold smoke            | API/Renderer 1.434/1.435 s；5 个样本完整 core RSS mean 95.11 MiB、CPU mean 0.08%；Sidecar RSS mean 11.45 MiB                                                            | **PASS**                                                                      |
| WebView cold smoke         | WebView event 0.396 s，API/Renderer 2.204/2.204 s；8 个启动/网络样本 RSS mean 1,216.46 MiB、CPU mean 14.88%；Sidecar RSS mean 32.25 MiB                                 | **PASS（启动行为）**；只作冷启动诊断                                          |
| Sidecar 强杀恢复           | host PID `78317`，Sidecar PID `78320 → 78329 → 78348 → 78380`；前三次纯本地 health/player 恢复，第四次耗尽预算                                                          | **PASS**                                                                      |
| 正常退出                   | Cmd+Q 观察到 Sidecar `Some(0)`；PID `78505/78514/78517/78518/78519` 均已消失；退出前 `12754/27232/28232` 在监听，`27233` 未配置；退出后四端口均无 listener              | **PASS**                                                                      |

本地产物的 adhoc Hardened Runtime seal、结构、架构、sealed resources、空 entitlement 和运行行为已通过。
Developer ID 和公证按当前政策为 **N/A**，用户首次打开时按 README 的 Gatekeeper 放行步骤操作。
这份 pre-tag 本地 DMG 还缺 tag CI 重建、canary endpoint 和 updater 签名验收，不作为已发布的 canary。

下列结构、smoke 与长时性能数据全部来自 checksum 为 `372f921c…0bd08e` 的本地 DMG，以及它挂载后复制出的同一个临时安装副本。该 SHA 不代表未来 tag CI artifact，也不计入 90 天观察期。

#### 300 样本稳态结果（每秒一次）

完整树 RSS 会重复计算共享页；表中同时保留各进程 RSS，Rust 后端收益只用 Sidecar 自身 RSS/`phys_footprint` 归因。

| 场景                 |  完整树 RSS mean / P95 / max | 完整树 CPU mean / P95 / max | Tauri RSS / CPU mean | Sidecar RSS / CPU mean |              Sidecar `phys_footprint` |
| -------------------- | ---------------------------: | --------------------------: | -------------------: | ---------------------: | ------------------------------------: |
| 可见暂停稳态 0-5 min | 255.35 / 256.89 / 256.98 MiB |           0.14 / 1.1 / 2.7% |    89.64 MiB / 0.01% |      10.15 MiB / 0.01% | 结束 8.203606 MiB；peak 38.391151 MiB |
| 连续播放 0-5 min     | 804.52 / 812.97 / 818.38 MiB |          1.58 / 4.8 / 19.2% |    93.72 MiB / 0.30% |      12.92 MiB / 0.00% |                   5 分钟 8.938004 MiB |
| 连续播放 5-10 min    | 834.59 / 843.91 / 857.64 MiB |          1.81 / 5.0 / 65.3% |    93.59 MiB / 0.32% |      13.28 MiB / 0.01% |                  10 分钟 8.609901 MiB |
| 窗口隐藏 0-5 min     | 553.43 / 560.03 / 592.05 MiB |           0.15 / 1.0 / 1.5% |   103.78 MiB / 0.01% |      13.36 MiB / 0.01% |                          8.703651 MiB |

四个时间窗口来自同一个 installed session：host/Sidecar PID 为 `78505/78514`，WebKit
GPU/Networking/WebContent PID 为 `78517/78518/78519`。每份证据都有独立的 300 样本区间。

播放开始时为 `あいつら全員同窓会 · ずっと真夜中でいいのに。`，进度约 221.55 s；5 分钟点为
`真夜中のドア〜stay with me (シングルver.) · 松原みき` 4.30 s；10 分钟点为
`Notion · The Rare Occasions` 57.62 s。三个检查点均由用户确认处于播放态。播放后暂停并隐藏窗口，
进度保持静止，`lsappinfo` 同时报告 hidden 状态。

第二个播放窗口的完整树 RSS mean 比第一个高 30.07 MiB（3.7%），增量主要来自 WebKit。
Sidecar RSS mean 从 12.92 MiB 增至 13.28 MiB，`phys_footprint` 从 8.938004 MiB 降至
8.609901 MiB。10 分钟内没有 Sidecar 物理内存持续累积的证据，这组数据不能证明长期运行不存在泄漏。

四份性能证据均为 schemaVersion 4，各含 300 个 `rawSamples` 和逐进程摘要：
[`installed-idle-5m.json`](./evidence/sidecar-rust-migration/installed-idle-5m.json)、
[`installed-playback-0-5m.json`](./evidence/sidecar-rust-migration/installed-playback-0-5m.json)、
[`installed-playback-5-10m.json`](./evidence/sidecar-rust-migration/installed-playback-5-10m.json) 和
[`installed-hidden-idle-5m.json`](./evidence/sidecar-rust-migration/installed-hidden-idle-5m.json)。四份均已由
`verify-performance-evidence.mjs` 独立重算通过，并绑定同一 DMG 和 installed executable SHA。人工 UI、
Sidecar `phys_footprint` 与退出回收记录在
[`installed-footprints.json`](./evidence/sidecar-rust-migration/installed-footprints.json)。

判定：

- **PASS**：包体积 hard gate；隐藏窗口完整树 CPU mean 0.15% ≤ 2%；播放态 Tauri CPU mean
  0.30% / 0.32% ≤ 10%；Sidecar `phys_footprint` 从播放 5 分钟的 8.938004 MiB 降至
  10 分钟的 8.609901 MiB；supervisor recovery 与 stdin sentinel graceful 退出。
- **观察项**：可见暂停稳态完整树 CPU mean 为 0.14%，Tauri 主进程为 0.01%；播放第二窗口完整树
  RSS mean 比第一窗口高 30.07 MiB（3.7%）。Sidecar RSS mean 增加 0.36 MiB，
  `phys_footprint` 减少 0.328103 MiB。完整树仍需 matched Bun/WebKit 对照，WebKit 波动不计作后端收益。
- **PENDING**：tag CI 生成的最终 canary artifact、updater archive/`.sig`、canary endpoint 与 feed；
  本地候选包 SHA 不预填到发布观察记录。
- **不能宣称**：尚未在同一机器、同一场景重跑 Bun 安装包，完整树相对 Bun/Electron 的降幅
  仍未确定。Rust Sidecar 自身远低于历史 Bun Sidecar 约 82 MB `phys_footprint`。

## 依赖与许可证决策

- `ncm-api-rs` 锁定确切版本或 commit；不跟随浮动 git HEAD。
- Rust Sidecar 的 12 个 `unm_* 0.4.0` crates 是 `LGPL-3.0-or-later`；传递闭包 `unm_api_utils → unm_engine_kuwo → random-string 1.1.0` 是 normal dependency，后者声明 `GPL-3.0-only`。项目采用 Sidecar `GPL-3.0-only`、独立 Tauri host `MIT` 的进程分发策略，两边 Cargo metadata 均显式声明对应 license。该策略仍需发布者自行承担最终合规判断，自动测试不能提供法律意见。
- `build-sidecar-compliance.mjs` 先以 release workspace 图给独立 Sidecar resolver 划定上界，再以独立 workspace 实际解析出的非 dev 图作为分发闭包。当前闭包是 354 个 registry 依赖；生成器会随同 application source、精确 vendor source、GPL/LGPL 正文、第三方 notice、校验和、固定工具链和重链接脚本一起写入独立完整源码目录，并在空 `CARGO_HOME`、空 target 目录下完成 `--offline --locked --release` 构建。`.app` 只带轻量 notice、二进制 provenance 和同版本源码下载指引；`dist_tauri/` 另生成完整源码压缩包、SHA-256 及与 DMG 同层的 `SOURCE-OFFER`，tag workflow 的 `release/**/*` 上传规则会把它们放进同一 Release。Windows/Linux 的最终 Installation Information 发布审查仍是 **PENDING**。
- `build-app-compliance.mjs` 从 `cargo tree -e normal` 构建 target-specific host 闭包，并从最终 Rollup chunks 记录 renderer package。unknown/missing license、越界 symlink 或闭包漂移都会失败。当前 macOS manifest 覆盖 328 个 host package 和 44 个 renderer package，其中 342 项许可证文本来自 package，30 项来自固定 coordinate/repository/VCS/digest 的 curated donor。macOS bundle exact bytes 已通过；NSIS/AppImage/deb extraction gate 已接入 workflow，实际实验分发物仍是 **PENDING**。
- Sidecar 声明并验证 MSRV 1.89；Tauri host 声明 1.88。两者来自实际锁定依赖闭包，不用低于传递依赖要求的虚假版本。
- 若依赖闭包新增 copyleft 包、缺少 SPDX 元数据、UNM 版本/来源变化或 relink kit 不再解析到本地源码，构建必须失败并重新审查。
- `yt-dlp` 不默认打包。当前行为若依赖系统安装，则 Rust 版保持同样的可选 provider；若以后决定内置，另开产品、体积和 GPL 合规评审。
- 无新 API key。在线验证只使用本地已有网易云会话，cookie 不进入日志、fixture 或 CI artifact。

## 协议维护策略

- 当前可确认的是：固定的 `@neteaseapireborn/api` 4.29.7 在 2026-08-10 仍能完成已观察到的登录和无损播放；这不等于所有协议多年不变。
- [`api-enhanced`](https://github.com/NeteaseCloudMusicApiEnhanced/api-enhanced) 作为活跃参考实现和变化哨兵，但不承诺其响应时效。
- required CI 使用固定、脱敏、可复现 fixtures，不把浮动上游或真实网络当成 merge oracle。
- **计划：**独立 canary 定期对照锁定 JS reference；发现差异时只输出 route、decoder 和稳定字段，不保存账号或 cookie。当前没有 schedule 或 differential runner，不能作为已存在门禁。
- 最脆弱的前提是 Rust 生态能在 72 小时内跟上网易协议变化。若连续两次真实变化都无法达标，则停止删除 Bun 源码，继续发布最后一个稳定后端版本，并重新评估维护成本。

## 回滚策略

- 阶段 1：若停在概念检查点，删除 Rust 测试 package 即可回滚，产品仍是 Bun。
- 阶段 2：安装包是 Rust-only；回滚通过 Git revert 或安装上一个已发布 Bun 版本，不运行双后端。
- origin、端口和数据 schema 不变，回滚不迁移或删除用户数据。
- 阶段 3：Bun 源码删除后仍可从 Git 历史恢复；正常故障处理优先修复 Rust，不在用户机器动态恢复 Bun。

## 明确不做

- 不把 Rust HTTP/NCM/UNM 服务塞进 Tauri 主进程。
- 不在一个安装包内携带两套后端。
- 不做请求级、路由级或运行时自动 Bun fallback。
- 不重写 Vue、Pinia、Player 或页面交互。
- 不为了省体积移除 UNM、云盘、代理或无损精确 seek。
- 不改用第三方远程网易云 API。
- 不从零重写网易云加密协议；优先复用并锁定现有 Rust 实现。
- 不按 README 自报性能直接宣传，只使用可复现的 release 实测数据。
