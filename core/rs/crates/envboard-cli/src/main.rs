//! envboard 命令行。
//!
//! **M0 的形态说明（别被它误导）**：现在还没有常驻工作台，因此 CLI **直接驱动管理器**，
//! 并在命令执行期间持有 `<state_dir>/lock`。 的终态是"CLI 是本地 API 的
//! 瘦客户端，绝不直接写状态文件"—— 那一层等 M3 的 axum 服务落地后接上。
//!
//! 这里已经按终态预留了两条纪律：
//!
//! 1. **状态只有一个写者**：本进程通过 flock 拿锁；拿不到就响亮失败（说明有别的
//!    envboard 在跑），而不是"先改了再说"。
//! 2. **实例的存活期由谁负责**，如实告诉用户：进程内的 core（FakeCore）实例随本进程
//!    退出而结束，所以要让环境常驻必须用 `envboard run`；这也是为什么 `env` 子命令
//!    只改期望状态、真正的拉起交给 resident 循环。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

mod api_client;
use api_client::ApiClient;
use envboard_core_api::{ClockPort, Error, ErrorCode, InstanceHealth, LoggerPort, ProxyCore};
use envboard_core_fake::FakeCore;
use envboard_core_mitmproxy::{MitmproxyCore, MitmproxyCoreConfig};
use envboard_manager::infra::{
    ProcfsProcessTable, RealFiles, SocketPortProbe, StderrLogger, SystemClock,
};
use envboard_manager::{EnvView, JsonFileStateRepo, Manager, ManagerConfig, StateRepo};

#[derive(Parser, Debug)]
#[command(
    name = "envboard",
    version,
    about = "envboard —— 多环境代理管理器：一个环境 = 一个实例 = 一个端口"
)]
struct Cli {
    /// 状态目录（环境账本、运行时状态、规则库、共享 CA 都在它下面）。
    ///
    /// 默认 `$ENVBOARD_STATE_DIR`，其次 `~/.envboard`。**读环境变量是适配器的职责**
    /// （core 禁止直接读环境变量：读 `env` 是适配器的职责）。
    #[arg(long, global = true)]
    state_dir: Option<PathBuf>,

    /// 随机分配端口的区间，形如 `16000-16999`。
    #[arg(long, global = true)]
    port_range: Option<String>,

    /// 用哪个 proxy core。`fake` 只监听端口（用于无 mitmproxy 的环境与测试），
    /// `mitmproxy` 才会真正改写上连地址。
    #[arg(long, global = true, value_parser = ["fake", "mitmproxy"], default_value = "mitmproxy")]
    core: String,

    /// `mitmdump` 路径（默认按 PATH 解析）。
    #[arg(long, global = true)]
    core_bin: Option<PathBuf>,

    /// 装有 mitmproxy 的解释器路径（CA 预物化用）。
    /// **默认不是环境里的 python3**：本机实测默认 python3 没有 mitmproxy，
    /// 所以默认走"从 core.bin 推导 + 自检"。
    #[arg(long, global = true)]
    core_python: Option<PathBuf>,

    /// 本地 API 地址。省略时先读 `<state_dir>/runtime/api.json`，再退回 127.0.0.1:8900。
    ///
    /// 有常驻实例（工作台或 `run`）时会自动改走 HTTP —— CLI 因此不再抢状态锁。
    #[arg(long, global = true)]
    api: Option<String>,

    /// 本地 API 的访问令牌（仅当工作台监听非回环地址时才需要）。
    #[arg(long, global = true)]
    token: Option<String>,

    /// 实例日志落盘目录（默认 `<state_dir>/logs`）。
    ///
    /// 实例的 stdout/stderr **直接写**这个目录里的 `<env>.log`：没有管道、没有读线程，
    /// 所以"没人读管道 → 管道写满 → 子进程阻塞 → 代理挂住"这条路径不存在。
    /// 日志跨重启留存，崩溃现场可追溯；体积由 `--max-log-bytes` 轮转限制。
    #[arg(long, global = true)]
    log_dir: Option<PathBuf>,

    /// 不落盘：日志只留在内存的有界环形缓冲里（随实例结束而消失）。
    ///
    /// 换来的是磁盘零写入，代价是回到"必须持续把管道读走"的形态 ——
    /// 读线程一旦停摆，管道写满后实例会卡死（实测 64 KiB ≈ 213 个请求）。
    #[arg(long, global = true)]
    no_log_file: bool,

    /// 单个环境日志文件的轮转上限（字节，`0` = 不轮转）。
    #[arg(long, global = true)]
    max_log_bytes: Option<u64>,

    /// 注入器检查规则文件 mtime 的间隔（秒）。越小越灵敏，越大越省 CPU。
    #[arg(long, global = true)]
    reload_interval: Option<u64>,

    /// 机器可读输出（JSON）。
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 显示管理器、core 与环境概况。
    Status,
    /// 工作台：常驻管理器 + 本地 HTTP API + 内嵌前端。
    Web {
        /// 监听地址；非 127.0.0.1 时必须给 `--token`。
        #[arg(long, default_value = "127.0.0.1:8900")]
        listen: String,
        /// 非本机监听时必填的访问令牌（**不允许无鉴权对外**）。
        #[arg(long)]
        token: Option<String>,
        /// 只做一轮 reconcile 就退出（烟测用，不起 HTTP 服务）。
        #[arg(long)]
        once: bool,
    },
    /// 常驻（无界面）：先 reconcile，然后持续报告健康（Ctrl-C 退出）。
    Run {
        /// 只做一轮 reconcile 就退出（CI/烟测用）。
        #[arg(long)]
        once: bool,
        /// resident 模式下每隔多少秒刷新一次。
        #[arg(long, default_value_t = 5)]
        interval: u64,
        /// 不真正启动实例，只打印将要做什么。
        #[arg(long)]
        dry_run: bool,
    },
    /// 环境管理。
    Env {
        #[command(subcommand)]
        command: EnvCommand,
    },
    /// 跨环境静态对比：某域名在各环境被覆盖成什么（不发任何请求）。
    Compare { host: String },
    /// 规则文件管理。
    Rules {
        #[command(subcommand)]
        command: RulesCommand,
    },
}

#[derive(Subcommand, Debug)]
enum EnvCommand {
    /// 列出环境。
    List,
    /// 查看一个环境。
    Show { name: String },
    /// 新建环境（不给 --port 就在区间里随机分配一个空闲端口）。
    Add {
        name: String,
        /// 显式指定端口（人工固定时用）。
        #[arg(long)]
        port: Option<u16>,
        /// 绑定规则名（需先 `envboard rules import`）。
        #[arg(long)]
        rules: Option<String>,
        /// 透传给 core 的每实例选项，可重复，形如 `--option ssl_insecure=true`
        /// （**持久化在环境上**，受 denylist 约束；改它要 `env edit`）。
        #[arg(long = "option", value_name = "K=V")]
        options: Vec<String>,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// 修改已有环境（PATCH 语义：只改给出来的字段）。
    ///
    /// **改名、换端口、换绑定与换选项都必须先停止**：它们都是环境的启动契约，
    /// 运行中改会让客户端代理配置、状态文件与进程记录同时对不上。描述是热改。
    Edit {
        name: String,
        /// 改名（新名字不能已存在）。
        #[arg(long, value_name = "NEW")]
        rename: Option<String>,
        /// 换端口（PATCH 到 `listen.port`）。
        #[arg(long)]
        port: Option<u16>,
        /// 绑定（或换绑）规则名。
        #[arg(long)]
        rules: Option<String>,
        /// 解除规则绑定（回到"不覆盖"）。与 `--rules` 互斥。
        #[arg(long, conflicts_with = "rules")]
        no_rules: bool,
        /// 整体替换每实例选项（PATCH 到 `options`，**不是**逐键合并）。可重复。
        #[arg(long = "option", value_name = "K=V")]
        options: Vec<String>,
        /// 清空全部每实例选项（回到"不透传"）。与 `--option` 互斥。
        #[arg(long, conflicts_with = "options")]
        no_options: bool,
        /// 改描述；给空串即清空。
        #[arg(long)]
        description: Option<String>,
    },
    /// 标记为"期望运行"并尝试启动（选项取环境上持久化的 `options`）。
    Start { name: String },
    /// 停止并标记为"期望停止"。
    Stop { name: String },
    /// 重启。
    Restart { name: String },
    /// 删除（必须先是停止状态）。
    Rm { name: String },
    /// 实例日志尾部。
    Logs {
        name: String,
        #[arg(long, default_value_t = 200)]
        lines: usize,
    },
}

#[derive(Subcommand, Debug)]
enum RulesCommand {
    /// 导入一份 hosts 风格文本：解析 → 规范化 → 落盘（`<rules_dir>/<name>.rules`）。
    ///
    /// 同名文件会被**整体覆盖**（不是追加）。想改一份已有规则，先 `rules show` 取回内容。
    Import {
        name: String,
        /// 源文件；省略则从 stdin 读。
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 打印一份已导入的规则文件原文（改之前先取回，避免盲覆盖）。
    Show { name: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 错误码与字段路径是契约的一部分：CLI 只翻译，不重新判定。
            eprintln!(
                "envboard: {}: {}{}",
                error.code.as_str(),
                error.message,
                error
                    .field
                    .as_deref()
                    .map(|field| format!(" (field: {field})"))
                    .unwrap_or_default()
            );
            match error.code.retryability() {
                envboard_core_api::Retryability::No => ExitCode::from(2),
                _ => ExitCode::from(1),
            }
        }
    }
}

fn run(cli: Cli) -> Result<(), Error> {
    let config = build_config(&cli)?;

    // ① 先看有没有常驻实例。有就走 HTTP（状态只有一个写者：那个进程是写者）。
    if let Some(client) = resolve_api(&cli, &config) {
        if !cli.json {
            eprintln!("(thin client → {})", client.addr);
        }
        return thin_dispatch(client, &cli);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Error::internal_error(format!("cannot start tokio runtime: {error}")))?;

    let clock: Arc<dyn ClockPort> = Arc::new(SystemClock);
    let logger: Arc<dyn LoggerPort> = Arc::new(StderrLogger);
    let core = build_core(&cli, &config, Arc::clone(&clock))?;

    let manager = Manager::new(
        config,
        core,
        Arc::new(SocketPortProbe),
        Arc::new(ProcfsProcessTable),
        Arc::new(RealFiles),
        Arc::clone(&clock),
        logger,
        Arc::new(JsonFileStateRepo::new(config_path(&cli))) as Arc<dyn StateRepo>,
        // 单实例锁：拿不到就响亮失败（状态只有一个写者）。
        true,
    )
    .map_err(|error| {
        if error.code == ErrorCode::Conflict && !cli.json {
            return Error {
                message: format!(
                    "{} — a workbench or `envboard run` is probably holding it; \
                     point me at it with `--api <addr>` (or stop it)",
                    error.message
                ),
                ..error
            };
        }
        error
    })?;

    // 在 match 之前取出来：`match cli.command` 会部分移动 `cli`
    let state_dir = cli_state_dir(&cli);
    runtime.block_on(async {
        match cli.command {
            Command::Status => status(&manager, cli.json),
            Command::Run {
                once,
                interval,
                dry_run,
            } => resident(&manager, once, interval, dry_run, cli.json).await,
            Command::Web {
                listen,
                token,
                once,
            } => {
                if once {
                    let report = manager.reconcile().await?;
                    print_reconcile(&report, cli.json);
                    return Ok(());
                }
                let web_config =
                    envboard_web::WebConfig::parse(&listen, token, &manager.config().state_dir)?;
                // 把监听地址写进 runtime/api.json：CLI 靠它发现自定义端口的工作台
                // （否则只会去试默认的 8900），退出时清掉。
                api_client::write_api_record(&state_dir, web_config.listen).ok();
                let result = envboard_web::serve(Arc::new(manager), web_config).await;
                api_client::remove_api_record(&state_dir);
                result
            }
            Command::Env { command } => env_command(&manager, command, cli.json).await,
            Command::Compare { host } => {
                let value = manager.compare(&host)?;
                if cli.json {
                    println!("{}", serde_json::to_string_pretty(&value)?);
                } else {
                    println!("{}", value["host"].as_str().unwrap_or(&host));
                    for row in value["environments"].as_array().into_iter().flatten() {
                        let covered = row["covered"].as_bool().unwrap_or(false);
                        println!(
                            "  {:<12} {:<8} {}",
                            row["env"].as_str().unwrap_or("-"),
                            format!(":{}", row["port"].as_u64().unwrap_or(0)),
                            if covered {
                                format!("→ {}", row["ip"].as_str().unwrap_or("-"))
                            } else {
                                "未覆盖".to_string()
                            }
                        );
                    }
                }
                Ok(())
            }
            Command::Rules { command } => rules_command(&manager, command, cli.json),
        }
    })
}

/// 按 `--core` 构造 proxy core。
///
/// **默认 mitmproxy**：它是产品本体。`fake` 只监听端口、不改写任何流量，
/// 存在的意义是让"没有 mitmproxy 的机器"也能跑管理器与测试。
fn build_core(
    cli: &Cli,
    config: &ManagerConfig,
    clock: Arc<dyn ClockPort>,
) -> Result<Arc<dyn ProxyCore>, Error> {
    match cli.core.as_str() {
        "fake" => Ok(Arc::new(FakeCore::new(clock))),
        _ => Ok(Arc::new(MitmproxyCore::new(
            MitmproxyCoreConfig {
                core_bin: cli
                    .core_bin
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("mitmdump")),
                python: cli.core_python.clone(),
                agent_dir: config.agent_dir.clone(),
                reload_interval_secs: config.reload_interval_secs,
                annotate_flow: true,
                startup_timeout_secs: 30,
            },
            clock,
        )?)),
    }
}

/// 找到常驻实例：`--api` > `<state_dir>/runtime/api.json` > 默认 8900。
///
/// 三个来源都**必须探测成功**才算找到 —— 端口上蹲着别的程序时不能误判。
fn resolve_api(cli: &Cli, config: &ManagerConfig) -> Option<ApiClient> {
    let token = cli.token.clone();

    if let Some(raw) = &cli.api {
        let addr = raw.parse().ok()?;
        return ApiClient::probe(addr, token);
    }
    if let Some(addr) = api_client::read_api_record(&config.state_dir)
        && let Some(client) = ApiClient::probe(addr, token.clone())
    {
        return Some(client);
    }
    // 默认端口的兜底**只在用户没有显式指定状态目录时**生效。
    //
    // 否则 `--state-dir /tmp/scratch` 会被 8900 上那个跟它毫无关系的常驻实例接管：
    // 命令看着成功了，改的却是**别人的**那份状态（孤立测试就是这么被污染的）。
    // 显式给了状态目录，就只认那个目录里的 `api.json`。
    if cli.state_dir.is_some() {
        return None;
    }
    let fallback: std::net::SocketAddr = "127.0.0.1:8900".parse().ok()?;
    ApiClient::probe(fallback, token)
}

/// 瘦客户端模式：把命令翻译成 REST 调用。
///
/// 这一层**不做任何领域判断**：错误码与字段路径都来自服务端（"适配器只翻译"），
/// 所以 CLI 与工作台看到的行为永远一致。
fn thin_dispatch(client: ApiClient, cli: &Cli) -> Result<(), Error> {
    let json = cli.json;
    match &cli.command {
        Command::Status => print_json(&client.get("/api/status")?, json),
        Command::Run { .. } | Command::Web { .. } => Err(Error::new(
            ErrorCode::Conflict,
            "a resident envboard already owns the state; `run`/`web` here would be a second writer",
        )),
        Command::Compare { host } => {
            print_json(&client.get(&format!("/api/compare?host={host}"))?, json)
        }
        Command::Rules { command } => match command {
            RulesCommand::Import { name, file } => {
                let text = read_source(file)?;
                let value = client.post(
                    "/api/rules",
                    Some(&serde_json::json!({"name": name, "text": text})),
                )?;
                print_json(&value, json)
            }
            RulesCommand::Show { name } => {
                let value = client.get(&format!("/api/rules/{name}"))?;
                if json {
                    print_json(&value, true)
                } else {
                    print!("{}", value["text"].as_str().unwrap_or_default());
                    Ok(())
                }
            }
        },
        Command::Env { command } => match command {
            EnvCommand::List => {
                let value = client.get("/api/environments")?;
                match value.as_array() {
                    Some(items) => {
                        for item in items {
                            print_json_or_view(item, json)?;
                        }
                        Ok(())
                    }
                    None => print_json(&value, json),
                }
            }
            EnvCommand::Show { name } => {
                print_json(&client.get(&format!("/api/environments/{name}"))?, json)
            }
            EnvCommand::Add {
                name,
                port,
                rules,
                options,
                description,
            } => {
                let mut body = serde_json::json!({"name": name, "description": description});
                if let Some(rules) = rules {
                    body["rules"] = serde_json::Value::String(rules.clone());
                }
                if let Some(port) = port {
                    body["listen"] = serde_json::json!({"port": port});
                }
                if !options.is_empty() {
                    body["options"] = options_value(options)?;
                }
                print_json(&client.post("/api/environments", Some(&body))?, json)
            }
            EnvCommand::Edit {
                name,
                rename,
                port,
                rules,
                no_rules,
                options,
                no_options,
                description,
            } => {
                let patch = edit_patch(
                    rename,
                    *port,
                    rules,
                    *no_rules,
                    options,
                    *no_options,
                    description,
                )?;
                print_json(
                    &client.patch(&format!("/api/environments/{name}"), &patch)?,
                    json,
                )
            }
            EnvCommand::Start { name } => print_json(
                &client.post(&format!("/api/environments/{name}/start"), None)?,
                json,
            ),
            EnvCommand::Stop { name } => print_json(
                &client.post(&format!("/api/environments/{name}/stop"), None)?,
                json,
            ),
            EnvCommand::Restart { name } => print_json(
                &client.post(&format!("/api/environments/{name}/restart"), None)?,
                json,
            ),
            EnvCommand::Rm { name } => {
                print_json(&client.delete(&format!("/api/environments/{name}"))?, json)
            }
            EnvCommand::Logs { name, lines } => print_json(
                &client.get(&format!("/api/environments/{name}/logs?lines={lines}"))?,
                json,
            ),
        },
    }
}

fn print_json(value: &serde_json::Value, json: bool) -> Result<(), Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    print_json_or_view(value, false)
}

/// 非 `--json` 时，环境对象渲染成与直连模式**同一行**格式。
fn print_json_or_view(value: &serde_json::Value, json: bool) -> Result<(), Error> {
    if json || value.get("proxy_command").is_none() {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    println!(
        "{:<12} {:<6} {:<16} rules={:<8} {}{}",
        value["name"].as_str().unwrap_or("-"),
        value["desired"].as_str().unwrap_or("-"),
        value["health"].as_str().unwrap_or("-"),
        format!(
            "{}({})",
            value["rules"].as_str().unwrap_or("-"),
            value["rules_count"].as_u64().unwrap_or(0)
        ),
        value["description"]
            .as_str()
            .filter(|text| !text.is_empty())
            .map(|text| format!("\"{text}\""))
            .unwrap_or_default(),
        value["health_reason"]
            .as_str()
            .map(|reason| format!(" [{reason}]"))
            .unwrap_or_default()
    );
    if value["health"].as_str() == Some("running")
        && let Some(command) = value["proxy_command"].as_str()
    {
        println!("  {command}");
    }
    Ok(())
}

fn read_source(file: &Option<PathBuf>) -> Result<String, Error> {
    match file {
        Some(path) => std::fs::read_to_string(path).map_err(Error::from),
        None => {
            let mut text = String::new();
            std::io::stdin()
                .read_to_string(&mut text)
                .map_err(|error| Error::internal_error(format!("cannot read stdin: {error}")))?;
            Ok(text)
        }
    }
}

/// `--state-dir` / `ENVBOARD_STATE_DIR` / `~/.envboard` —— 只在一处定义。
fn cli_state_dir(cli: &Cli) -> PathBuf {
    cli.state_dir
        .clone()
        .or_else(|| std::env::var_os("ENVBOARD_STATE_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".envboard")))
        .unwrap_or_else(|| PathBuf::from(".envboard"))
}

fn build_config(cli: &Cli) -> Result<ManagerConfig, Error> {
    let state_dir = cli
        .state_dir
        .clone()
        .or_else(|| std::env::var_os("ENVBOARD_STATE_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".envboard")))
        .ok_or_else(|| {
            Error::invalid_config(
                "state_dir",
                "cannot determine a state dir; pass --state-dir",
            )
        })?;

    let mut config = ManagerConfig::new(state_dir);
    if let Some(dir) = cli.log_dir.clone() {
        config.log_dir = Some(dir);
    }
    if cli.no_log_file {
        config.log_dir = None;
    }
    if let Some(bytes) = cli.max_log_bytes {
        config.max_log_bytes = bytes;
    }
    if let Some(seconds) = cli.reload_interval {
        config.reload_interval_secs = seconds.max(1);
    }
    if let Some(raw) = &cli.port_range {
        let (min, max) = raw.split_once('-').ok_or_else(|| {
            Error::invalid_config("port_range", format!("expected <min>-<max>, got {raw:?}"))
        })?;
        let min: u16 = min
            .trim()
            .parse()
            .map_err(|_| Error::invalid_config("port_range", format!("bad min port in {raw:?}")))?;
        let max: u16 = max
            .trim()
            .parse()
            .map_err(|_| Error::invalid_config("port_range", format!("bad max port in {raw:?}")))?;
        config.port_range = (min, max);
    }
    config.validate()?;
    Ok(config)
}

fn config_path(cli: &Cli) -> PathBuf {
    let state_dir = cli
        .state_dir
        .clone()
        .or_else(|| std::env::var_os("ENVBOARD_STATE_DIR").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".envboard")))
        .unwrap_or_else(|| PathBuf::from(".envboard"));
    ManagerConfig::new(state_dir).state_file()
}

fn status(manager: &Manager, json: bool) -> Result<(), Error> {
    let core = manager.core_info();
    let capabilities = manager.capabilities();
    let views = manager.list()?;
    let running = views
        .iter()
        .filter(|view| view.health.as_str() == "running")
        .count();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "core": {"name": core.name, "version": core.version},
                "capabilities": {
                    "listen": capabilities.listen,
                    "dynamic_certs": capabilities.dynamic_certs,
                    "rewrite_upstream": capabilities.rewrite_upstream,
                    "external_processes": capabilities.external_processes,
                    "reports_rules_count": capabilities.reports_rules_count,
                },
                "config": manager.config().to_string(),
                "environments": views.len(),
                "running": running,
            }))?
        );
    } else {
        println!("core        : {} {}", core.name, core.version);
        println!("config      : {}", manager.config());
        println!("environments: {} (running: {running})", views.len());
        if !capabilities.rewrite_upstream {
            println!(
                "note        : this core does NOT rewrite upstream addresses — it only listens. \
                 Use `--core mitmproxy` for real hosts-rule rewriting"
            );
        }
    }
    Ok(())
}

fn print_reconcile(report: &envboard_manager::ReconcileReport, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({"actions": report.actions, "warnings": report.warnings})
        );
        return;
    }
    println!("reconcile: {} action(s)", report.actions.len());
    for (env, action) in &report.actions {
        println!("  {action:<14} {env}");
    }
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
}

async fn resident(
    manager: &Manager,
    once: bool,
    interval: u64,
    dry_run: bool,
    json: bool,
) -> Result<(), Error> {
    let report = manager.reconcile().await?;
    print_reconcile(&report, json);
    if dry_run {
        println!("dry-run: nothing was actually started");
        return Ok(());
    }
    if once {
        return Ok(());
    }

    println!("resident: holding the state lock; Ctrl-C to exit");
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval.max(1))).await;
        manager.maintain_logs();
        let views = manager.list()?;
        for view in views {
            print_view(&view, json)?;
        }
    }
}

/// 把 `env edit` 的开关折成一份 PATCH 体。
///
/// 直连模式与瘦客户端模式**共用这一份**：否则两边各写一次字段映射，
/// 迟早出现"工作台能改、命令行不能改"的分叉（分配器/端点推导只写一次是同一理由）。
///
/// 只放**给出来的**字段：PATCH 语义是"改这几项"，没给的一律不动。
/// `--no-rules` 显式写成 `null` —— 契约里 `rules: null` 就是"不覆盖"，
/// 与"没给这个字段"是两件事。`--option` / `--no-options` 同款：
/// `options` 是**整体替换**（不是逐键合并），`null` 即清空回 `{}`。
fn edit_patch(
    rename: &Option<String>,
    port: Option<u16>,
    rules: &Option<String>,
    no_rules: bool,
    options: &[String],
    no_options: bool,
    description: &Option<String>,
) -> Result<serde_json::Value, Error> {
    let mut patch = serde_json::Map::new();
    if let Some(new_name) = rename {
        patch.insert("name".into(), serde_json::Value::from(new_name.clone()));
    }
    if let Some(port) = port {
        patch.insert("listen".into(), serde_json::json!({"port": port}));
    }
    if no_rules {
        patch.insert("rules".into(), serde_json::Value::Null);
    } else if let Some(rules) = rules {
        patch.insert("rules".into(), serde_json::Value::from(rules.clone()));
    }
    if no_options {
        patch.insert("options".into(), serde_json::Value::Null);
    } else if !options.is_empty() {
        patch.insert("options".into(), options_value(options)?);
    }
    if let Some(description) = description {
        patch.insert(
            "description".into(),
            serde_json::Value::from(description.clone()),
        );
    }
    if patch.is_empty() {
        return Err(Error::invalid_config(
            "environment",
            "nothing to change: pass at least one of --rename/--port/--rules/--no-rules/\
             --option/--no-options/--description (an empty patch would report success without \
             doing anything)",
        ));
    }
    Ok(serde_json::Value::Object(patch))
}

/// `--option K=V`（可重复）折成契约里的 `options` 对象。
///
/// denylist 的判定权在 core（`envboard_core_api::validate_options`），这里**不复制**
/// 一份键表；CLI 只负责把 `K=V` 拆开。
fn parse_options(raw: &[String]) -> Result<BTreeMap<String, String>, Error> {
    let mut options = BTreeMap::new();
    for item in raw {
        let Some((key, value)) = item.split_once('=') else {
            return Err(Error::invalid_config(
                "instance.options",
                format!("expected K=V, got {item:?}"),
            ));
        };
        options.insert(key.trim().to_string(), value.trim().to_string());
    }
    Ok(options)
}

fn options_value(raw: &[String]) -> Result<serde_json::Value, Error> {
    Ok(serde_json::to_value(parse_options(raw)?)?)
}

async fn env_command(manager: &Manager, command: EnvCommand, json: bool) -> Result<(), Error> {
    match command {
        EnvCommand::List => {
            for view in manager.list()? {
                print_view(&view, json)?;
            }
            Ok(())
        }
        EnvCommand::Show { name } => print_view(&manager.get(&name)?, json),
        EnvCommand::Add {
            name,
            port,
            rules,
            options,
            description,
        } => {
            let mut input = serde_json::Map::new();
            input.insert("name".into(), name.clone().into());
            input.insert("description".into(), description.into());
            if let Some(rules) = rules {
                input.insert("rules".into(), rules.into());
            }
            if let Some(port) = port {
                input.insert("listen".into(), serde_json::json!({"port": port}));
            }
            if !options.is_empty() {
                input.insert("options".into(), options_value(&options)?);
            }
            let view = manager.create(&serde_json::Value::Object(input))?;
            print_view(&view, json)
        }
        EnvCommand::Edit {
            name,
            rename,
            port,
            rules,
            no_rules,
            options,
            no_options,
            description,
        } => {
            let patch = edit_patch(
                &rename,
                port,
                &rules,
                no_rules,
                &options,
                no_options,
                &description,
            )?;
            print_view(&manager.update(&name, &patch)?, json)
        }
        EnvCommand::Start { name } => {
            let view = manager.start(&name).await?;
            print_view(&view, json)?;
            if !json {
                eprintln!(
                    "note: instances are supervised by this process (they end when it exits), so \
                     `env start` suits a quick check — use `envboard run` or `envboard web` to \
                     keep environments up"
                );
            }
            Ok(())
        }
        EnvCommand::Stop { name } => print_view(&manager.stop(&name).await?, json),
        EnvCommand::Restart { name } => {
            manager.stop(&name).await?;
            print_view(&manager.start(&name).await?, json)
        }
        EnvCommand::Rm { name } => {
            manager.remove(&name)?;
            if !json {
                println!("removed {name}");
            }
            Ok(())
        }
        EnvCommand::Logs { name, lines } => {
            let tail = manager.logs_tail(&name, lines)?;
            if json {
                println!("{}", serde_json::json!({"env": name, "lines": tail}));
            } else if tail.is_empty() {
                println!("（{name} 还没有日志）");
            } else {
                for line in tail {
                    println!("{line}");
                }
            }
            Ok(())
        }
    }
}

fn rules_command(manager: &Manager, command: RulesCommand, json: bool) -> Result<(), Error> {
    match command {
        RulesCommand::Import { name, file } => {
            let mut text = String::new();
            match file {
                Some(path) => text = std::fs::read_to_string(&path)?,
                None => {
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .map_err(|error| {
                            Error::internal_error(format!("cannot read stdin: {error}"))
                        })?;
                }
            }
            let path = manager.import_rules(&name, &text)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"name": name, "path": path.display().to_string()})
                );
            } else {
                println!("imported {name} → {}", path.display());
            }
            Ok(())
        }
        RulesCommand::Show { name } => {
            let text = manager.rules_read(&name)?;
            if json {
                println!("{}", serde_json::json!({"name": name, "text": text}));
            } else {
                print!("{text}");
            }
            Ok(())
        }
    }
}

fn print_view(view: &EnvView, json: bool) -> Result<(), Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(&view.to_json())?);
        return Ok(());
    }
    let health = view.health.as_str();
    let reason = view.health.reason().unwrap_or_default();
    let rules = view.rules.as_deref().unwrap_or("-");
    println!(
        "{:<12} {:<6} {:<16} rules={:<8} {}{}",
        view.name,
        format!("{:?}", view.desired).to_lowercase(),
        health,
        format!("{rules}({})", view.rules_count),
        quote_if_set(&view.description),
        if reason.is_empty() {
            String::new()
        } else {
            format!(" [{reason}]")
        }
    );
    if matches!(view.health, InstanceHealth::Running) {
        println!("  {}", view.proxy_command);
    }
    Ok(())
}

fn quote_if_set(text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("\"{text}\"")
    }
}
