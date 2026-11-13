# 架构

## 引擎进程模型

一个环境实例 = 一个 OS 线程 + 一个 current-thread tokio runtime + 一个监听端口。
崩溃隔离是**任务级**的：单条请求任务 panic 只死那一条连接；引擎线程整体 panic 才让
实例进入 `failed`，由 reconcile 按期望状态拉回。这要求 release 构建**保持 unwind**
（`panic = "abort"` 会让任务级隔离失效）—— 属构建契约。

## ConnectTarget 是唯一接缝

`authority / sni / resolved_addr / tls_policy / chained_proxy`。改写语义只有一件事：
不动请求内容、不动 `Host` 头、不动 SNI 基准，只改"连到哪"。

## 内置能力

监听、407 门、TLS 策略、共享 CA、协议面是引擎原生快路径；hosts 规则改写（connect
路径直调修订 `resolved_addr`，脏数据 fail-closed 502）与请求终局日志（有界总线投递，
写失败丢行不阻断流量）是引擎内置步骤，不是可插拔接口。逐项语义以 `envboard-engine`
crate 文档的「引擎能力矩阵」为准。

## 同步热装配

管理器把归一化环境字段 + 规则账本 rendered 编译成 `EngineSpec`，`ProxyEngine::apply`
经 **ArcSwap 原子换入**新快照并回执 `config_hash` 与 `epoch`。**失败整套拒绝**：旧快照
继续服务，管理器留 `invalid_config` 标记，健康视图以 `unhealthy` +
"configuration rejected, previous snapshot still serving" 呈现。

## 有界日志总线

数据面写日志走 `try_send` 即返回，队列满则丢弃并计数 —— 数据面永不阻塞在写日志上。
落盘是 `<log_dir>/<env>.log`（管理器侧单一写者）、copytruncate 轮转、有界尾读。

## 分层与目录

依赖单向：`protocol/events ← engine ← manager ← admin ← host ← desktop`。契约不住在
独立文档里，住在各 crate 的文档注释（rustdoc）与 `tests/contract-tests/fixtures/` 的
84 个 golden fixture 里。

```
frontend/        工作台 SPA（Vite 7 + React 19 + HeroUI 3 + Tailwind 4，pnpm）：
                 唯一的界面真相；数据层走 @tauri-apps/api（invoke/Channel）；
                 dist/ 不入库，由 cargo build 自动产出并内嵌
crates/
  core/          领域内核
    protocol         UI 边界词汇：视图 DTO、类型化请求 DTO、帧形状、游标、错误信封
    events           事件词汇：控制面审计 / 数据面轨迹 / JSONL 两档存取
    engine           ProxyEngine 接缝、错误码、端口；domain/rules 纯逻辑 + 引擎本体
    engine-fake      ProxyEngine 的生命周期替身（测试用，真绑端口）
  service/       编排与组合根
    manager          环境 CRUD、端口分配、账本、锁、健康判定、reconcile、规则账本、
                     EngineSpec 编译与热装配
    admin            Admin 管理门面：类型化入口校验 + 用例编排，传输无关
    host             唯一组合根：引擎装配 + 共享 CA 物化 + 管理器构造（含单实例锁）
  app/           交付形态
    desktop          Tauri 2 壳 = 唯一入口与发布物（自成 workspace）：
                     clap 翻译 + 启动编排 + 41 个 IPC 命令 + 两条 Channel 推送流 +
                     严格 CSP + WebView2 加固
tests/
  contract-tests   可执行契约：84 个 fixture 的形状与语义消费
mise.toml        本地开发环境：node/pnpm 版本锁定 + 任务入口
```

分层纪律（结构与评审承载，不设机械门禁）：protocol / events 是叶子；engine 的
domain / rules 是纯逻辑；**引擎装配只住 `crates/service/host`**；**命令行解析库只许
出现在 `envboard-desktop`**；**GUI 框架只用 Tauri 2**；全部 crate 显式
`publish = false`（发布物是二进制，不是 crate）。

## 契约住在哪

- 视图 DTO / 帧 / 游标 / 安全约定 → `envboard-protocol` crate 文档；
- 错误码表 / 可重试性 / 界面三层呈现 → `envboard-engine::api::error`（界面映射唯一
  定义在 `frontend/src/errors.ts`）；
- 方法语义 / 生效方式 / 传输绑定 → `envboard-admin` crate 文档；
- 规则语法 BNF / 归一化 / 确定性渲染 → `envboard-engine::rules`；
- 环境字段不变量 / PATCH 语义 → `envboard-engine::domain::environment`；
- 事件信封 / 两档存储 / 抓包会话 → `envboard-events`；
- 能力清单 / 端口 / 生命周期 / 账本 → `envboard-manager`；
- 引擎能力矩阵 / 热应用 / CA / 上游代理 → `envboard-engine`。

`cargo doc --workspace --no-deps` 在验证链里：文档必须能编译。
