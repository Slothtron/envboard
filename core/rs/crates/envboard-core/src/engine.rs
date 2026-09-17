//! 进程内引擎实例：一个环境 = 一个 OS 线程 + 一个 current-thread runtime +
//! 一个监听端口。崩溃隔离是任务级的：请求任务 panic 只死这一条连接（tokio
//! 逐任务捕获），引擎线程整体 panic 才让实例进入 failed。
//!
//! 请求管线（阶段模型；插件链与内核快路径的分工见 core/spec/capabilities.md
//! 的「v3 插件与能力注册表」）：
//!
//! 1. 鉴权门（内核）：CONNECT 与 absolute-URI 同一道门，不过即 407。
//! 2. 隧道/直连分流：CONNECT → MITM 按 SNI 现签；absolute-URI → 明文上游。
//! 3. 每请求取环境快照 → ConnectTarget 播种（内核）→ 插件 connect 链修订
//!    → 内核按最终地址做 insecure 判定 → 上游建连（TLS 依 tls_policy）。
//! 4. request_head/body → 上游 → response_head/body 插件链（Early 短路给
//!    mock 类插件；错误按 fail-closed/Bypass 两档）。
//! 5. 终局 LogRecord 扇出给 log 阶段（request-log 默认启用）。

use std::net::SocketAddr;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use arc_swap::ArcSwap;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};

use envboard_core_api::proxy::Listen;
use envboard_core_api::text::parse_ip_literal;
use envboard_core_api::{Error, ErrorCode};

use crate::auth;
use crate::ca::SharedCa;
use crate::config::{self, CompiledConfig, EngineConfig};
use crate::http::{self, HttpError, Message};
use crate::plugin::{self, Flow, LogRecord, RequestView};
use crate::target::{ConnectTarget, TlsPolicy};
use crate::tls;

/// 读一个头（含保活等待）的上限：空闲连接被超时切断，不占 fd。
pub const HEAD_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// 体读取上限（缓冲语义下的整体超时）。
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(120);
/// 上游 TCP 连接超时。
pub const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 上游 TLS 握手超时。
pub const UPSTREAM_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// stop() 等待引擎线程退出的上限（超时仍返回，线程自己收尾）。
pub const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineState {
    Starting,
    Running,
    Stopped,
    /// 绑定失败即端口被占：不换端口（v2 契约），等一次显式动作。
    PortConflict {
        port: u16,
    },
    /// 线程活着但监听失效（accept 持续出错）。
    Unhealthy {
        reason: String,
    },
    /// 线程终止（panic / 引擎主循环以错误退出）。
    Failed {
        reason: String,
    },
}

impl EngineState {
    /// 契约里的状态字面量（v3 健康表）。
    pub fn as_str(&self) -> &'static str {
        match self {
            EngineState::Starting => "starting",
            EngineState::Running => "running",
            EngineState::Stopped => "stopped",
            EngineState::PortConflict { .. } => "port_conflict",
            EngineState::Unhealthy { .. } => "unhealthy",
            EngineState::Failed { .. } => "failed",
        }
    }
    pub fn reason(&self) -> Option<&str> {
        match self {
            EngineState::Unhealthy { reason } | EngineState::Failed { reason } => Some(reason),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    pub state: EngineState,
    pub config_hash: String,
    pub epoch: u64,
    pub last_error: Option<String>,
}

/// 上游连接的两种形态（absolute 明文 / CONNECT 隧道 TLS）。手写双分发实现
/// AsyncRead/AsyncWrite，避免装箱在热路径扩散。
enum Upstream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for Upstream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        match &mut *self {
            Upstream::Plain(stream) => Pin::new(stream).poll_read(cx, buf),
            Upstream::Tls(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for Upstream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<tokio::io::Result<usize>> {
        match &mut *self {
            Upstream::Plain(stream) => Pin::new(stream).poll_write(cx, data),
            Upstream::Tls(stream) => Pin::new(stream).poll_write(cx, data),
        }
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<tokio::io::Result<()>> {
        match &mut *self {
            Upstream::Plain(stream) => Pin::new(stream).poll_flush(cx),
            Upstream::Tls(stream) => Pin::new(stream).poll_flush(cx),
        }
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<tokio::io::Result<()>> {
        match &mut *self {
            Upstream::Plain(stream) => Pin::new(stream).poll_shutdown(cx),
            Upstream::Tls(stream) => Pin::new(stream).poll_shutdown(cx),
        }
    }
}

struct Shared {
    configs: ArcSwap<CompiledConfig>,
    acceptor: TlsAcceptor,
    strict: Arc<rustls::ClientConfig>,
    permissive: Arc<rustls::ClientConfig>,
    bound: Listen,
    status: Mutex<EngineStatus>,
    epoch: AtomicU64,
    shutdown: watch::Sender<bool>,
}

/// 一个引擎实例的句柄。生命周期动作都是同步语义（与"装配即生效"一致）。
pub struct EngineInstance {
    shared: Arc<Shared>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl EngineInstance {
    /// 拉起实例：配置先编译（响亮失败），线程随后台绑定；绑定结果经
    /// [EngineInstance::status] 可见（starting → running / port_conflict / failed）。
    pub fn start(config: EngineConfig, ca: Arc<SharedCa>) -> Result<Self, Error> {
        let compiled = config::compile(&config)?;
        let hash = compiled.config_hash.clone();
        let bound = compiled.listen;
        let server = tls::mitm_server_config(ca)?;
        let (shutdown, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            configs: ArcSwap::from_pointee(compiled),
            acceptor: TlsAcceptor::from(server),
            strict: tls::strict_upstream_client_config()?,
            permissive: tls::permissive_upstream_client_config()?,
            bound,
            status: Mutex::new(EngineStatus {
                state: EngineState::Starting,
                config_hash: hash,
                epoch: 1,
                last_error: None,
            }),
            epoch: AtomicU64::new(1),
            shutdown,
        });
        let thread_shared = shared.clone();
        let join = std::thread::Builder::new()
            .name(format!("envboard-engine-{}", bound.port))
            .spawn(move || run_thread(thread_shared))
            .map_err(|e| {
                Error::new(
                    ErrorCode::InternalError,
                    format!("cannot spawn engine thread: {e}"),
                )
            })?;
        Ok(EngineInstance {
            shared,
            join: Mutex::new(Some(join)),
        })
    }

    /// 同步热更新：编译通过即整套换入，返回新 config_hash；编译失败即 Err，
    /// 线上旧快照继续服务。listen 是停机字段，改它必须先重启实例。
    pub fn apply_config(&self, config: EngineConfig) -> Result<String, Error> {
        let compiled = config::compile(&config)?;
        if compiled.listen != self.shared.bound {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                format!(
                    "listen is a stop-time field: bound at {}, cannot move in place; restart the instance",
                    self.shared.bound
                ),
            ));
        }
        let hash = compiled.config_hash.clone();
        self.shared.configs.store(Arc::new(compiled));
        let epoch = self.shared.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let mut status = self.shared.status.lock().unwrap();
        status.config_hash = hash.clone();
        status.epoch = epoch;
        Ok(hash)
    }

    pub fn status(&self) -> EngineStatus {
        self.shared.status.lock().unwrap().clone()
    }

    /// 当前链上各插件的 bypass 计数（工作台状态视图的接线点在管理面）。
    pub fn bypass_counts(&self) -> Vec<(&'static str, u64)> {
        let cfg = self.shared.configs.load_full();
        cfg.plugins
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.descriptor.id,
                    cfg.plugins.bypass_count(entry.descriptor.id).unwrap_or(0),
                )
            })
            .collect()
    }

    /// 等实例离开 starting（绑定成功或冲突/失败都是"定态"）。
    pub fn wait_until_settled(&self, within: Duration) -> EngineStatus {
        let deadline = std::time::Instant::now() + within;
        loop {
            let status = self.status();
            if !matches!(status.state, EngineState::Starting)
                || std::time::Instant::now() >= deadline
            {
                return status;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// 优雅停止：关监听、置 stopped；在途连接随线程退出被回收。幂等。
    pub fn stop(&self) {
        let _ = self.shared.shutdown.send(true);
        {
            let mut status = self.shared.status.lock().unwrap();
            if matches!(
                status.state,
                EngineState::Starting | EngineState::Running | EngineState::Unhealthy { .. }
            ) {
                status.state = EngineState::Stopped;
            }
        }
        let join = self.join.lock().unwrap().take();
        if let Some(join) = join {
            let start = std::time::Instant::now();
            while !join.is_finished() && start.elapsed() < SHUTDOWN_JOIN_TIMEOUT {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

impl Drop for EngineInstance {
    fn drop(&mut self) {
        let _ = self.shared.shutdown.send(true);
        // 不阻塞 join：持有者析构不是停机命令的规范入口（那是 stop()）。
        let _ = self.join.lock().unwrap().take();
    }
}

impl std::fmt::Debug for EngineInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineInstance")
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

fn set_state(shared: &Shared, state: EngineState, last_error: Option<String>) {
    let mut status = shared.status.lock().unwrap();
    status.state = state;
    status.last_error = last_error;
}

fn run_thread(shared: Arc<Shared>) {
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(e) => {
                set_state(
                    &shared,
                    EngineState::Failed {
                        reason: format!("cannot build runtime: {e}"),
                    },
                    Some(e.to_string()),
                );
                return;
            }
        };
        runtime.block_on(engine_main(shared.clone()));
    }));
    if outcome.is_err() {
        set_state(
            &shared,
            EngineState::Failed {
                reason: "engine thread panicked".to_string(),
            },
            Some("panic".to_string()),
        );
    }
}

async fn engine_main(shared: Arc<Shared>) {
    let listener = match TcpListener::bind((shared.bound.host, shared.bound.port)).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            set_state(
                &shared,
                EngineState::PortConflict {
                    port: shared.bound.port,
                },
                None,
            );
            return;
        }
        Err(error) => {
            set_state(
                &shared,
                EngineState::Failed {
                    reason: format!("bind failed: {error}"),
                },
                Some(error.to_string()),
            );
            return;
        }
    };
    let mut shutdown = shared.shutdown.subscribe();
    // 绑定即 running：v2 的"状态文件 TTL + 收敛窗口"整条链路不再存在。
    set_state(&shared, EngineState::Running, None);
    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() { break; }
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((socket, _peer)) => {
                        let per_shared = shared.clone();
                        // 任务级崩溃隔离：这个 spawn 里 panic 只死这一条连接。
                        tokio::spawn(async move {
                            let _ = handle_client(socket, per_shared).await;
                        });
                    }
                    Err(error) => {
                        // 持续 accept 错误（fd 耗尽等）：标记 unhealthy 并继续等；
                        // 重启决策属于管理面。
                        set_state(
                            &shared,
                            EngineState::Unhealthy {
                                reason: format!("accept: {error}"),
                            },
                            Some(error.to_string()),
                        );
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
    }
}

/// 一条客户端连接：头（逐字节读，绝不吞 CONNECT 后的 TLS 记录）→ 鉴权门 →
/// 分流 CONNECT 隧道 / absolute-URI 转发。
async fn handle_client(mut socket: TcpStream, shared: Arc<Shared>) -> Result<(), HttpError> {
    let cfg = shared.configs.load_full();
    let head =
        match tokio::time::timeout(HEAD_READ_TIMEOUT, http::read_head_exactly(&mut socket)).await {
            Err(_elapsed) => return Ok(()),
            Ok(Err(error)) => {
                let _ = write_status(&mut socket, error.status(), &error.reason()).await;
                return Err(error);
            }
            Ok(Ok(None)) => return Ok(()),
            Ok(Ok(Some(head))) => head,
        };
    let method = head.method().unwrap_or("").to_ascii_uppercase();
    if cfg.auth_required() && !credentials_ok(&cfg, &head) {
        let _ = socket.write_all(auth::challenge()).await;
        return Ok(());
    }
    if method == "CONNECT" {
        connect_flow(socket, &head, &shared).await
    } else {
        absolute_flow(socket, head, &shared).await
    }
}

fn credentials_ok(cfg: &CompiledConfig, head: &Message) -> bool {
    let Some(value) = head.header("proxy-authorization") else {
        return false;
    };
    match auth::parse_basic(value) {
        Some((user, password)) => cfg.check_credentials(&user, &password),
        None => false,
    }
}

async fn connect_flow(socket: TcpStream, head: &Message, shared: &Shared) -> Result<(), HttpError> {
    let mut socket = socket;
    let authority = head.uri().unwrap_or("");
    let Some((host, port)) = http::split_authority(authority, 443) else {
        let _ = write_status(&mut socket, 400, "malformed CONNECT authority").await;
        return Ok(());
    };
    socket
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    let tls = match shared.acceptor.accept(socket).await {
        Ok(tls) => tls,
        // 客户端不发 TLS：它可能只是探测端口。静默关闭，不报错给谁。
        Err(_) => return Ok(()),
    };
    let client_sni = {
        let (_, conn) = tls.get_ref();
        conn.server_name().map(str::to_string)
    };
    let mut io = BufReader::new(tls);
    loop {
        let cfg = shared.configs.load_full();
        let head = match tokio::time::timeout(HEAD_READ_TIMEOUT, http::read_head(&mut io)).await {
            Err(_) => return Ok(()),
            Ok(Err(error)) => {
                let _ = write_status(io.get_mut(), error.status(), &error.reason()).await;
                return Err(error);
            }
            Ok(Ok(None)) => return Ok(()),
            Ok(Ok(Some(head))) => head,
        };
        if head
            .method()
            .is_some_and(|method| method.eq_ignore_ascii_case("CONNECT"))
        {
            let _ = write_status(
                io.get_mut(),
                400,
                "CONNECT inside a TLS tunnel is not supported",
            )
            .await;
            return Ok(());
        }
        if cfg.auth_required() && !credentials_ok(&cfg, &head) {
            let _ = io.get_mut().write_all(auth::challenge()).await;
            return Ok(());
        }
        let origin = RequestOrigin {
            host: host.clone(),
            port,
            sni: client_sni.clone(),
            with_tls: true,
        };
        if serve_request(&mut io, shared, &cfg, head, &origin).await? {
            return Ok(());
        }
    }
}

async fn absolute_flow(
    socket: TcpStream,
    first: Message,
    shared: &Shared,
) -> Result<(), HttpError> {
    let mut io = BufReader::new(socket);
    let mut head = Some(first);
    loop {
        let cfg = shared.configs.load_full();
        let message = match head.take() {
            Some(message) => message,
            None => match tokio::time::timeout(HEAD_READ_TIMEOUT, http::read_head(&mut io)).await {
                Err(_) => return Ok(()),
                Ok(Err(error)) => {
                    let _ = write_status(io.get_mut(), error.status(), &error.reason()).await;
                    return Err(error);
                }
                Ok(Ok(None)) => return Ok(()),
                Ok(Ok(Some(head))) => head,
            },
        };
        let method = message.method().unwrap_or("GET").to_ascii_uppercase();
        if method == "CONNECT" {
            let _ = write_status(io.get_mut(), 405, "use a plain TCP connection for CONNECT").await;
            return Ok(());
        }
        let Some((host, port, _path)) = message.uri().and_then(http::parse_absolute_http_uri)
        else {
            let _ = write_status(
                io.get_mut(),
                400,
                "forwarded request needs an absolute http:// URI",
            )
            .await;
            return Ok(());
        };
        // absolute-URI 形态：authority 取 URI 本身，Host 头只透传不改写。
        let origin = RequestOrigin {
            host,
            port,
            sni: None,
            with_tls: false,
        };
        if serve_request(&mut io, shared, &cfg, message, &origin).await? {
            return Ok(());
        }
    }
}

/// 请求的来源形态：CONNECT 隧道（tls）或 absolute-URI（明文）。
struct RequestOrigin {
    host: String,
    port: u16,
    sni: Option<String>,
    with_tls: bool,
}

/// 每请求管线。返回 true = 这条连接到此为止（错误、透传或显式 close）。
async fn serve_request<W: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<W>,
    shared: &Shared,
    cfg: &Arc<CompiledConfig>,
    head: Message,
    origin: &RequestOrigin,
) -> Result<bool, HttpError> {
    let started_ms = unix_ms();
    let clock = std::time::Instant::now();
    let method = head.method().unwrap_or("GET").to_ascii_uppercase();
    let path = origin_path(&head);
    let authority = format!("{}:{}", origin.host, origin.port);
    let keep_alive = !head
        .header("connection")
        .is_some_and(|value| value.to_ascii_lowercase().contains("close"));

    // 内核播种 → 插件 connect 修订 → 内核按最终地址做 insecure 判定。
    let mut target = ConnectTarget::seed(&origin.host, origin.port);
    let seed_addr = target.resolved_addr;
    if let Err(fault) = plugin::run_connect(&cfg.plugins, &cfg.log_writer, &mut target).await {
        return fail(
            cfg,
            io,
            started_ms,
            clock,
            &method,
            &authority,
            &path,
            &target,
            seed_addr,
            &fault.to_report(),
        )
        .await;
    }
    let address_text = target
        .resolved_addr
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|| origin.host.clone());
    target.apply_insecure_match(cfg.is_insecure(origin.sni.as_deref(), Some(&address_text)));

    // 请求体：分帧 + 缓冲上限（快照值，不是编译期常量）。
    let request_bytes = match http::request_framing(&method, &head) {
        Err(error) => {
            let _ = write_status(io.get_mut(), error.status(), &error.reason()).await;
            return Ok(true);
        }
        Ok(framing) => {
            match tokio::time::timeout(
                BODY_READ_TIMEOUT,
                http::read_body(io, framing, cfg.max_buffered_body),
            )
            .await
            {
                Err(_) => {
                    let _ = write_status(io.get_mut(), 408, "request body read timed out").await;
                    return Ok(true);
                }
                Ok(Err(error)) => {
                    let _ = write_status(io.get_mut(), error.status(), &error.reason()).await;
                    return Ok(true);
                }
                Ok(Ok(body)) => body,
            }
        }
    };

    // request 阶段插件链（head → 可能 Early；body 改写）。
    let mut request_body = request_bytes;
    let early = {
        let view = RequestView {
            target: &target,
            method: &method,
            path: &path,
            headers: &head.headers,
            client_sni: origin.sni.as_deref(),
        };
        match plugin::run_request_head(&cfg.plugins, &cfg.log_writer, &view).await {
            Err(fault) => {
                return fail(
                    cfg,
                    io,
                    started_ms,
                    clock,
                    &method,
                    &authority,
                    &path,
                    &target,
                    seed_addr,
                    &fault.to_report(),
                )
                .await;
            }
            Ok(Flow::Early(planned)) => Some(planned),
            Ok(Flow::Continue) => None,
        }
    };
    if early.is_none()
        && let Err(fault) =
            plugin::run_request_body(&cfg.plugins, &cfg.log_writer, &mut request_body).await
    {
        return fail(
            cfg,
            io,
            started_ms,
            clock,
            &method,
            &authority,
            &path,
            &target,
            seed_addr,
            &fault.to_report(),
        )
        .await;
    }

    let mut status;
    let mut response_headers;
    let mut response_body;
    if let Some(planned) = early {
        status = planned.status;
        response_headers = planned.headers;
        response_body = planned.body;
    } else {
        let upstream = match connect_upstream(shared, &target, origin.with_tls).await {
            Err(fault) => {
                return fail(
                    cfg, io, started_ms, clock, &method, &authority, &path, &target, seed_addr,
                    &fault,
                )
                .await;
            }
            Ok(upstream) => upstream,
        };
        match exchange(io, upstream, &head, &method, &path, &request_body).await {
            Err(fault) => {
                return fail(
                    cfg, io, started_ms, clock, &method, &authority, &path, &target, seed_addr,
                    &fault,
                )
                .await;
            }
            Ok(Exchange::Tunneled) => {
                // 101 的响应写出与双向透传都在 exchange 内完成（缓冲的 WS
                // 帧要跟着走）。这里只补终局记录。
                record(
                    cfg,
                    started_ms,
                    clock,
                    &method,
                    &authority,
                    &path,
                    101,
                    &request_body,
                    &[],
                    &target,
                    seed_addr,
                );
                return Ok(true);
            }
            Ok(Exchange::Parts(part)) => {
                status = part.0;
                response_headers = part.1;
                response_body = part.2;
            }
        }
    }

    // response 阶段插件链（head 可整条替换；body 可改写）。
    let replacement = match plugin::run_response_head(
        &cfg.plugins,
        &cfg.log_writer,
        &method,
        &authority,
        status,
        &mut response_headers,
    )
    .await
    {
        Err(fault) => {
            return fail(
                cfg,
                io,
                started_ms,
                clock,
                &method,
                &authority,
                &path,
                &target,
                seed_addr,
                &fault.to_report(),
            )
            .await;
        }
        Ok(replacement) => replacement,
    };
    if let Some(planned) = replacement {
        status = planned.status;
        response_headers = planned.headers;
        response_body = planned.body;
    }
    if let Err(fault) =
        plugin::run_response_body(&cfg.plugins, &cfg.log_writer, &mut response_body).await
    {
        return fail(
            cfg,
            io,
            started_ms,
            clock,
            &method,
            &authority,
            &path,
            &target,
            seed_addr,
            &fault.to_report(),
        )
        .await;
    }

    let first = format!("HTTP/1.1 {status} {}", reason_for_status(status));
    http::write_message(
        io.get_mut(),
        &first,
        &response_headers,
        &response_body,
        !(100..200).contains(&status),
    )
    .await?;
    record(
        cfg,
        started_ms,
        clock,
        &method,
        &authority,
        &path,
        status,
        &request_body,
        &response_body,
        &target,
        seed_addr,
    );
    Ok(!keep_alive)
}

#[allow(clippy::too_many_arguments)]
fn record(
    cfg: &CompiledConfig,
    timestamp: u64,
    clock: std::time::Instant,
    method: &str,
    authority: &str,
    path: &str,
    status: u16,
    request_body: &[u8],
    response_body: &[u8],
    target: &ConnectTarget,
    seed_addr: Option<SocketAddr>,
) {
    let event = LogRecord {
        timestamp_unix_ms: timestamp,
        method: method.to_string(),
        authority: authority.to_string(),
        path: path.to_string(),
        status,
        request_bytes: request_body.len(),
        response_bytes: response_body.len(),
        duration_ms: clock.elapsed().as_millis() as u64,
        rewritten: seed_addr != target.resolved_addr,
        insecure: matches!(target.tls_policy, TlsPolicy::Insecure),
        error: None,
    };
    plugin::run_log(&cfg.plugins, Arc::new(event));
}

#[allow(clippy::too_many_arguments)]
async fn fail<W: AsyncRead + AsyncWrite + Unpin>(
    cfg: &CompiledConfig,
    io: &mut BufReader<W>,
    timestamp: u64,
    clock: std::time::Instant,
    method: &str,
    authority: &str,
    path: &str,
    target: &ConnectTarget,
    seed_addr: Option<SocketAddr>,
    reason: &str,
) -> Result<bool, HttpError> {
    let event = LogRecord {
        timestamp_unix_ms: timestamp,
        method: method.to_string(),
        authority: authority.to_string(),
        path: path.to_string(),
        status: 502,
        request_bytes: 0,
        response_bytes: 0,
        duration_ms: clock.elapsed().as_millis() as u64,
        rewritten: seed_addr != target.resolved_addr,
        insecure: matches!(target.tls_policy, TlsPolicy::Insecure),
        error: Some(reason.to_string()),
    };
    plugin::run_log(&cfg.plugins, Arc::new(event));
    let _ = write_status(io.get_mut(), 502, reason).await;
    Ok(true)
}

fn origin_path(head: &Message) -> String {
    let uri = head.uri().unwrap_or("/");
    if let Some((_, _, path)) = http::parse_absolute_http_uri(uri) {
        return path;
    }
    if uri.starts_with('/') {
        return uri.to_string();
    }
    format!("/{uri}")
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

/// 按描述符建上游：地址（插件链修订后的终值）+ TLS（依策略）。
///
/// with_tls 由流量形态决定：CONNECT 隧道内必然 TLS；absolute-URI 是 http
/// scheme 的明文代理语义。校验档位始终由 target.tls_policy 决定。
async fn connect_upstream(
    shared: &Shared,
    target: &ConnectTarget,
    with_tls: bool,
) -> Result<Upstream, String> {
    let tcp = match tokio::time::timeout(UPSTREAM_CONNECT_TIMEOUT, async {
        match target.resolved_addr {
            Some(addr) => TcpStream::connect(addr).await,
            None => TcpStream::connect((target.host.as_str(), target.port)).await,
        }
    })
    .await
    {
        Err(_) => return Err(format!("upstream connect timed out: {}", target.authority)),
        Ok(Ok(tcp)) => tcp,
        Ok(Err(error)) => return Err(format!("upstream connect failed: {error}")),
    };
    if !with_tls {
        return Ok(Upstream::Plain(tcp));
    }
    let config = match target.tls_policy {
        TlsPolicy::Verify => &shared.strict,
        TlsPolicy::Insecure => &shared.permissive,
    };
    let server_name = match server_name_for(target) {
        Ok(name) => name,
        Err(text) => return Err(text),
    };
    let connector = TlsConnector::from(Arc::clone(config));
    match tokio::time::timeout(
        UPSTREAM_HANDSHAKE_TIMEOUT,
        connector.connect(server_name, tcp),
    )
    .await
    {
        Err(_) => Err(format!(
            "upstream TLS handshake timed out: {}",
            target.authority
        )),
        Ok(Ok(tls_stream)) => Ok(Upstream::Tls(Box::new(tls_stream))),
        Ok(Err(error)) => {
            let mut message = format!("upstream TLS handshake failed: {error}");
            let text = error.to_string().to_ascii_lowercase();
            if text.contains("certificate") || text.contains("cert") {
                message.push_str(&format!(
                    " | if {} must be served with an unverifiable certificate, add it to insecure_hosts",
                    target.sni.as_deref().unwrap_or(&target.host)
                ));
            }
            Err(message)
        }
    }
}

fn server_name_for(
    target: &ConnectTarget,
) -> Result<rustls::pki_types::ServerName<'static>, String> {
    let host = target.sni.as_deref().unwrap_or(&target.host);
    if let Some(ip) = parse_ip_literal(host) {
        return Ok(rustls::pki_types::ServerName::IpAddress(ip.into()));
    }
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| format!("host {host:?} is not a legal TLS server name"))
}

enum Exchange {
    /// 101：透传已结束这条上游连接上的一切事务。
    Tunneled,
    /// (status, headers, body)
    Parts((u16, Vec<(String, String)>, Vec<u8>)),
}

/// 一问一答的转发内核（插件链包裹在外层 serve_request 里）。
async fn exchange<W: AsyncRead + AsyncWrite + Unpin>(
    io: &mut BufReader<W>,
    mut upstream: Upstream,
    head: &Message,
    method: &str,
    path: &str,
    request_body: &[u8],
) -> Result<Exchange, String> {
    let first = format!("{method} {path} HTTP/1.1");
    let mut headers = http::forward_request_headers(head);
    if !head.is_upgrade_request() {
        headers.push(("connection".to_string(), "close".to_string()));
    }
    http::write_message(&mut upstream, &first, &headers, request_body, true)
        .await
        .map_err(|e| format!("upstream write failed: {}", e.reason()))?;

    let mut reader = BufReader::new(&mut upstream);
    let response = match http::read_head(&mut reader).await {
        Err(error) => return Err(format!("upstream response head: {}", error.reason())),
        Ok(None) => return Err("upstream closed without a response".to_string()),
        Ok(Some(response)) => response,
    };
    let status = response.status().unwrap_or(505);
    let framing = http::response_framing(method, status, &response);
    let body = http::read_body(&mut reader, framing, usize::MAX)
        .await
        .map_err(|e| format!("upstream response body: {}", e.reason()))?;
    std::mem::drop(reader);

    if status == 101 && head.is_upgrade_request() {
        // 先冲掉客户端读侧已缓冲的 WS 帧，写出 101，再裸字节互抄。
        let pending = io.buffer().to_vec();
        if !pending.is_empty() {
            let _ = upstream.write_all(&pending).await;
        }
        let headers = http::forward_response_headers(&response);
        http::write_message(io.get_mut(), &response.first, &headers, &body, false)
            .await
            .map_err(|e| format!("client write failed: {}", e.reason()))?;
        let client = io.get_mut();
        let _ = tokio::io::copy_bidirectional(client, &mut upstream).await;
        return Ok(Exchange::Tunneled);
    }
    Ok(Exchange::Parts((
        status,
        http::forward_response_headers(&response),
        body,
    )))
}

/// 我们自产的错误响应。
async fn write_status<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    status: u16,
    reason: &str,
) -> Result<(), HttpError> {
    let body = format!("{reason}\n");
    let head = format!(
        "HTTP/1.1 {status} {text}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
        text = reason_for_status(status),
    );
    writer
        .write_all(head.as_bytes())
        .await
        .map_err(|e| HttpError::Io(e.to_string()))?;
    writer
        .write_all(body.as_bytes())
        .await
        .map_err(|e| HttpError::Io(e.to_string()))?;
    writer
        .flush()
        .await
        .map_err(|e| HttpError::Io(e.to_string()))
}

fn reason_for_status(status: u16) -> &'static str {
    match status {
        101 => "Switching Protocols",
        400 => "Bad Request",
        405 => "Method Not Allowed",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        502 => "Bad Gateway",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}
