//! envboard 引擎库：编排层共享词汇（api）、hosts 规则解析与渲染（rules）、
//! 领域纯逻辑（domain）与进程内代理引擎本体的唯一合并 crate。
//!
//! 依赖纪律：本 crate 不依赖 axum/web 层；外部世界只经参数注入的路径与标准
//! 网络类型（端口语义由 manager 接线时再收口）。纯逻辑模块（api / rules /
//! domain）不引运行时库（tokio / rustls / rcgen 等），运行时只在引擎本体。
//! 各子契约住在对应模块文档：规则语法见 [`rules`]，错误码见 [`api::error`]，
//! 环境字段不变量见 [`domain`]。
//!
//! ## 引擎能力矩阵（换实现时必须逐项重新满足）
//!
//! | 能力 | 契约要求 | 实现位置 |
//! |---|---|---|
//! | 监听 | bind listen.host:port；EADDRINUSE → port_conflict（不试绑、不换端口） | 引擎线程内 TcpListener::bind，绑定即 running |
//! | 代理鉴权 | Basic 407；CONNECT 与 absolute-URI 同一条门；凭据不进视图/日志 | [`auth`] 鉴权门，内存定长时间比对 |
//! | TLS 中间人 | 自有共享 CA（CN=envboard，不复用 mitmproxy-ca*.pem）；按 SNI 现签叶子 | [`ca::SharedCa`] + [`tls::SniResolver`] |
//! | 上游证书策略 | insecure_hosts 精确匹配：SNI 优先、无 SNI 回退地址、SNI 存在未命中不回退；无全局关校验 | [`ConnectTarget`] 的 tls_policy + 连接执行器两档 |
//! | 改写 | hosts 规则只改连接目标：不动请求内容/Host 头/SNI 基准 | [`hosts`] 修订 resolved_addr |
//! | 日志 | 文件直写、copytruncate 轮转、有界尾读；**数据面永不阻塞在写日志上** | [`request_log`] + [`logsink`] 有界总线（丢弃计数入 EngineReport.log_drops） |
//! | 协议面 | HTTP/1.1（CONNECT 隧道 + absolute-URI）；ALPN 只协商 h1；101 透传 | [`http`] / [`engine`] |
//! | 配置生效 | 装配即生效；失败整套拒绝 + 旧快照继续 + 标记可见 | [`ProxyEngine::apply`]（ArcSwap） |
//!
//! **已知限制（随 README 发布）**：只实现 HTTP/1.1，强制 h2 的客户端（gRPC 等）
//! 会失败；上游信任库来自系统库（rustls-native-certs）。
//!
//! ## 配置下发与热应用（内存装配）
//!
//! 引擎在管理器的**同一进程**里：配置不是写给另一个进程轮询的文件，而是一次
//! 同步装配。**compile**：归一化环境字段 + 规则正文 → [`EngineSpec`] → 编译
//! （凭据成对校验、规则解析成查找表、config_hash），任何一步失败即
//! `invalid_config`（附字段路径），整套拒绝。**apply**：新快照原子换入
//! （ArcSwap），装配即生效，没有轮询间隔与收敛窗口；运行中请求按进入时取到的
//! 旧快照跑完（每请求 load_full，单请求内配置一致）。**失败保留旧快照**：
//! apply 失败时旧快照继续服务，管理器记 invalid_config 标记，健康视图以
//! unhealthy + "configuration rejected, previous snapshot still serving"
//! 呈现 —— 允许降级，禁止静默。回执：`EngineReport.config_hash` 与 `epoch`。
//!
//! ## mitm_hosts（只解密这些域名）
//!
//! 判定发生在 **CONNECT 的 authority 主机名**上（此时未解密，拿不到 SNI）；
//! 匹配与 insecure_hosts 同一份实现（见 [`rules`] 文档末节）。空清单 = 全部
//! 解密（默认）；非空 = 只解密列出的域名，其余 CONNECT **直通 = 盲隧道**
//! （应答 200 后双向抄字节，不 TLS accept、不抓包、不记轨迹）。**鉴权仍必须
//! 在 CONNECT 上判**（隧道里看不到 HTTP，缺 Proxy-Authorization 就 407，否则
//! 名单外域名成了绕过鉴权的通道）。**路由照旧**：白名单只管解密不管路由，
//! hosts 规则仍改写隧道目标地址。建连先于 200：上游连不上如实回 502。
//! 判定每连接读一次配置快照，热生效。
//!
//! ## 共享 CA（身份、轮换、按域名签发、信任库探测）
//!
//! 身份是自己的：CN=envboard，落盘 `shared/confdir/envboard-ca.pem`（私钥+证书）
//! 与 `envboard-ca-cert.pem`（客户端安装的就是它）；mitmproxy-ca*.pem 不再读取
//! 或复用，旧文件不删（用户的文件）。**轮换**：生成新 CA 覆盖两文件，CaStore
//! 是引擎每连接读取的当前值，下一条 CONNECT 即用新证书，不必重启环境；旧证书
//! 不从系统信任库移除（用户的决定）；轮换后客户端必须重装新证书（界面明说）。
//! **按域名签发**：现签叶子证书返回证书与私钥 PEM（只经剪贴板交付，不落盘、
//! 不进视图日志）。**信任库探测只读**：枚举当前用户与本机两个 ROOT 存储，与
//! 当前 CA 的 DER 逐字节比较；检测，不安装也不卸载；探测不到如实说"不知道"。
//!
//! ## 上游代理（chained proxy）建连语义
//!
//! 环境出方向先到一个 HTTP 代理再转达目标（代理账本与引用完整性在 manager）。
//! TLS 目标：TCP 连代理 → `CONNECT <authority>`（有凭据带 Proxy-Authorization:
//! Basic）→ 非 2xx → 502（消息带代理状态码与可行动建议）；2xx 后隧道内按既有
//! TlsPolicy 握手 —— 内层 TLS 与直连逐字节一致，insecure_hosts 语义不变；
//! CONNECT 目标地址 = 规则改写后的终址。明文 http：请求行改写为 absolute-URI
//! 直发代理，Host 头透传；凭据两半都带，客户端自己的 proxy-authorization 不
//! 透传（鉴权头只归属相邻跳）。超时：TCP+CONNECT 共用上游连接超时（10s），
//! 内层 TLS 用上游握手超时（10s）。**明确不做**：SOCKS5、https:// 代理形态、
//! PAC/透明代理、按域名分流、代理链。

pub mod api;
pub mod auth;
pub mod backend;
pub mod ca;
pub mod capture;
pub mod config;
pub mod der;
pub mod domain;
pub mod engine;
pub mod hosts;
pub mod http;
pub mod logsink;
pub mod request_log;
pub mod rules;
pub mod target;
pub mod tls;
pub mod trajectory;

/// 编排层共享词汇的根再导出：`crate::EngineSpec` 等接缝类型、
/// 错误码、端口（ClockPort/LineWriter/LoggerPort）从这里拿。
pub use api::*;
pub use backend::EngineBackend;
pub use ca::{CaOutcome, SharedCa};
pub use config::{CompiledConfig, EngineConfig};
pub use engine::{EngineInstance, EngineState, EngineStatus};
pub use request_log::LogRecord;
pub use target::{ConnectTarget, TlsPolicy};
pub use tls::{
    NO_SNI_FALLBACK_HOST, SniResolver, mitm_server_config, strict_upstream_client_config,
};
