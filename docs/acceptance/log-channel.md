# 实例日志通道与存活判定（实机验收）

> 触发问题：**"关于 mitmdump 日志，有没有解决方案，避免阻塞通道"**。
> 环境：本机 WSL2（Ubuntu 26.04），**mitmdump 12.2.3**（uv tool 安装），
> Python 3.12.14，内核 6.18.40.1-microsoft-standard-WSL2。
> 脚本：`scripts/verify_live_v2.py`（check 5/8/9/9b/9c）、
> `cargo test -p envboard-core-mitmproxy --test live_manager`。
> 复现：`bash ci/verify.sh live`。

## 1. 先量清楚：管道写满会怎样

mitmdump 的 stdout/stderr 被接成管道后，**没人读**时管道写满，它会阻塞在自己的事件循环里 ——
不是"日志丢了"，而是**所有客户端一起挂住**。

| 配置 | 日志量 | 装满 64 KiB 管道需要 |
|---|---|---|
| 默认（`termlog_verbosity=info`） | 307.5 字节/请求 | **213 个请求** |
| `--set flow_detail=0` | 226.5 字节/请求 | 289 个请求 |
| `--set termlog_verbosity=warn` | 81.0 字节/请求 | 809 个请求 |
| `-q` | 0 字节 | 不会满 |

管道容量实测 65536 字节（`fcntl(F_GETPIPE_SZ)`）。**故意不读管道**的实验：

```
管道容量 = 65536 字节；故意不读
  成功 212 个请求后开始卡住（第 212 个请求 code=000）
  读走 65116 字节后，下一个请求：200
```

读走数据后立刻恢复 → 卡的就是写端。这条实验是"必须把管道读走"的判据来源。

## 2. 改成了什么

子进程的 stdout/stderr **直接接文件**（`Stdio::from(File)`，两路用 `try_clone()` 共享
offset），落在 `<state_dir>/logs/<env>.log`：

- 内核负责写盘，**我们进程不在链路上** —— 第 1 节那条路径从设计上不存在；
- 日志跨重启留存，崩溃现场可追溯；
- 每次启动追加一行运行标记（`--- envboard: env=… listen=… started=… ---`）分段；
- `PYTHONUNBUFFERED=1` 照旧（Python 对**非 TTY 的文件**同样是块缓冲）；
- 体积由 `--max-log-bytes`（默认 8 MiB）封顶，轮转是 **copytruncate**
  （把尾部搬去 `.1` 再 `set_len(0)`）—— 不能用 rename，子进程还持着那个 inode 的 fd；
- `--no-log-file` 时退回"管道 + 内存环形缓冲"，那条路径的硬化：单行 8 KiB 上限、
  持久文件句柄、锁中毒不再 `unwrap()`（一个读线程 panic 会连带另一个死掉）。

## 3. 验收结果

### 3.1 打满 320 个请求（脚本 check 8）

```
PASS  8 连打 320 个请求（足以写满 64 KiB 管道）全部成功
      sent=320 failures=[] log_bytes=97837（管道容量 65536）
```

日志文件 97837 字节 > 65536 —— **已经越过旧的卡死阈值**，而请求一个没丢。
（逐请求 curl，不用"一条命令带 320 个 URL"：后者复用同一条连接在单线程上游前面排队，
测出来的是 curl 的调度而不是日志通道。）

### 3.2 `--no-log-file` 的管道路径（手工）

```
260 个请求成功 260 个（管道容量 64KiB ≈ 213 个请求）
内存环形缓冲里的尾部（经 API）：… "server disconnect 127.0.0.1:27910" …
磁盘上有没有日志文件：0 个（--no-log-file 下应为 0）
```

### 3.3 轮转（手工，`--max-log-bytes 65536`）

```
300 个请求成功 300 个
轮转前：92K（上限 64K）
轮转后：live=273 字节, rotated=32722 字节
两边都不超上限：live=1 rotated=1
```

重启后日志尾部仍是新内容（`envboard injector ready: rules=1 from …`），跨 `.1` 拼接可用。

### 3.4 崩溃可见 + 不留僵尸（脚本 check 9/9b/9c）

`kill -9` 掉实例进程后：

```
PASS  9 实例被 SIGKILL 后：工作台不再报 running、说清是被信号杀死、子进程被回收
      claims_running=0 settled='failed:the proxy core was killed by signal 9' zombie=False
PASS  9b 崩溃后日志仍可读（现场没丢）
PASS  9c 崩溃的实例可以重新拉起
```

Rust 侧同款断言在 `live_manager.rs`
（`a_crashed_instance_is_reported_as_failed_and_the_child_is_reaped`），
另外覆盖"reconcile 把它重新拉起"。

## 4. 顺带修掉的两个缺陷（都先复现过）

改日志通道时在同一条进程管线上发现两个真问题：

1. **子进程自己退出后无人回收 → 僵尸**。实测 `kill -9` 掉 mitmdump 后它是
   `Z [mitmdump] <defunct>`，一直挂到管理器退出（停掉工作台才被 init 收走）。
   判活只比 `PID + starttime`，而僵尸这两个量都没变 → 被判成"活着"。
   修法：判活读 `/proc/<pid>/stat` 的**状态位**（`Z`/`X` 不算活），
   外加每 500 ms 的回收任务 `try_wait()` 收尸（tokio 1.53.1 该方法是公开同步的）。
2. **视图层从不探活**：`health_blocking()` 只要句柄表里有这个环境就返回 `running`，
   与带 15 秒状态文件 TTL 的权威判定 `health()` 分叉。实测 SIGKILL 掉 mitmdump 后
   **60 秒仍报 `health=running, reason=None`**。修法：视图与权威判定共用同一套判据
   （进程存活 → 状态文件时效 → 契约回显），并且"由本管理器拉起、又没人叫它停、
   进程却没了"报 `failed` 而非 `stopped`（"它自己死了"与"我停的"是两件事）。

### 4.1 过程中自己造的一个 bug（记下来，因为很典型）

把 `require()`（会锁 `state`）放进 `health_blocking()` 之后，`env add` 直接挂死：
`view_of` 有调用点是在**持有 `state` 锁**的情况下调它的，而 `std::sync::Mutex` 不可重入。
修法：环境定义由调用方传进来，判定路径上不再取状态锁。

## 5. 仍然存在的代价

- 默认**会写盘**：不想落盘用 `--no-log-file`，那条路径又回到"读线程必须一直活着"的形态
  （硬化过，但结构性依赖还在）。
- 日志按体积轮转（8 MiB + 一份 `.1`），不做按时间保留/压缩。
- 上游连接复用丧失（被规则覆盖的域名每请求重建上连）、`sni is None` 分支、
  Windows 保留端口段这三条老限制不受本次影响（见 README 的「已知限制」）。
