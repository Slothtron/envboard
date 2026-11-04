# Changelog

All notable changes to `slothtron-envboard` are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed（走查整改 M2：静默失败与文案误导）

- **端口提示不再写死**：配置页的监听端口说明从组合根 `port_range()` 取区间，
  并说明"仅自动分配受限；显式端口 1–65535 均可"，与侧栏页脚、服务端校验同源
  （此前写死 16000–16999，与三处判据互相矛盾）。
- **三个模态表单不再"点了没反应"**：导入规则集、新建/编辑上游代理、按域名签发
  统一用共享的 `DialogProblem`（`views/mod.rs`）：校验失败把原因写在弹窗内、
  弹窗保持打开；逐字段说清是哪一项不合格。顺带修掉「凭据只填一栏被
  `build_proxy_body` 静默丢掉、用户以为保存成功」。
- **配置表单**：错误条从表单最底部移到顶部；任一字段改动即撤掉上一次保存的错误
  （旧结论不再成立）；新增 **Ctrl+Enter 保存**，不必切鼠标（Textarea 换行与
  Esc 三层语义故不绑裸 Enter / Esc）。
- **调试会话边界**：切换目标且已有记录时先确认"会清掉 N 条"；会话失效后记录
  仍可清空（按钮按"有没有记录"而非"有没有会话"），状态行也不再谎称"未开始"。
- 单测新增：改值清错误、端口提示取真实区间、无会话也能清空记录、`DialogProblem`
  接受后清原因。

### Fixed（走查整改 M1：三条界面数据不可信 + 每秒空转）

- **表头排序不再被快照覆盖**：`EnvTable` 存下排序列与方向，全量快照按同一个键
  重排后再写进 `rows`（此前点完表头最多 1s 就退回账本序、而箭头仍指着那档）。
  `apply` 的 `changed` 改为顺序无关：把新帧排好再比较，顺带修掉"排过序就与
  账本序不等 → 每帧误报变化"。新增 `sort_rows` 自由函数与一条回归单测。
- **换环境不再串规则正文**：`Detail::rename` 取消在途读盘（`Task` drop 即取消）、
  按当前页签重新加载，回执带环境名校验，旧环境的正文不会落进新面板；新增
  `WaitingSnapshot` 状态接住"新环境还没进这一拍快照"，不再卡在误导的 Idle 文案，
  也不在渲染期每帧重复起读盘任务。
- **删除环境后详情面板一起关**：`remove` 回执里 `close_sheet` + 清 `detail` 实体
  （此前只清 `selected`，Sheet 变成一个每帧被 notify 的「环境已不存在」空壳）。
- **快照没变时不再整屏重绘**：轮询回执只在 `apply` 返回真变化时
  `cx.notify()` / `touch_detail`，`tier_counts()` 的四次全量遍历与
  `rebuild_visible()` 不再是每秒固定开销；错误条撤销、首帧脱 loading、在途标记
  清理也算变化，不会因为收敛重绘而漏刷。
- L0 新增两条断言：删除选中环境后面板锚点消失、连续换环境后面板指向新环境。

### Added（工作台可用性补强：状态栏聚合 / 记录筛选 / 耗时列）

- **状态栏环境聚合（可点）**：底部状态栏左段常驻显示运行中/总数、需处理、抓包、
  规则、上游、证书信任。数字由当前快照聚合，与四档过滤**同一判据**（计数 = 点下去
  的行数）；「需处理」与「证书未安装」用警告色。**每一项都是入口**：运行中/需处理
  进环境屏并切到对应档，抓包 → 调试，规则 → 规则屏，上游 → 代理屏，证书 → CA 屏。
  聚合只在**数字变化**时触发根视图重绘 —— 环境屏每秒 notify 不会把其他屏（如 CA 屏
  的信任探测）拖进每一拍。
- **截断文本可悬停读全文**：环境「说明」、规则「被引用」、代理「被引用」三列被裁时
  给完整内容的 tooltip（`views::text_tooltip`，普通 `div` 的统一写法）。
- **调试屏记录筛选**：方法 / 状态档 / 主机三个维度可叠加，选项从当前记录派生
  （方法去重排序、主机去重排序），右端给命中数与「清除筛选」。只筛**视图**：
  会话缓冲、`request_id` 游标与已捕获计数都不动；空态区分"没有记录"与"没有匹配"。
- **抓包耗时列**：引擎在抓包记录里写入 `duration_ms`（请求行到达 → 响应体收齐，
  与轨迹/日志同一起点、终点略早一档：抓包在写回客户端之前就落）；HAR 导入取 entry
  的 `time`。表里新增「耗时」列（<1s 显示毫秒，往上进位到秒），没有响应的记录显示
  「—」而不是 0 ms。

### Changed（设置屏只剩"可以改的东西"）

- 「关于」不再列状态目录、端口区间、界面偏好路径：这些是提问/排障才用得上的本机
  信息，摆在设置页只会把唯一可改的那一项（主题）淹掉；装配警告照旧保留（它是
  "这份宿主跑得不对"的信号）。设置屏的复制路径动作随之移除。
- 状态目录路径也不再占状态栏左段（那一格给了环境聚合）：报错时错误条本来就会点名
  路径，端口区间在侧栏页脚。状态栏右段只保留「装配警告（有才显示）· 当前屏」，
  当前主题不再重复 —— 它是设置页里改的东西。

### Changed（走查后的版面收口）

- **键盘可达：状态栏是一个 Tab 落点**。F6 把焦点交给状态栏（根视图首帧落焦点，
  F6 才能在"没有焦点"时也生效），←/→ 在六项聚合之间走查并在两端夹住，
  Enter/Space 触发与点击相同的导航，Esc 回到根焦点。整条工具栏带
  `Role::Toolbar` / `tab_group` / 每项 `aria_label`，焦点环只在键盘聚焦时出现。
- **表格"选中"与"打开详情"拆开**：↑/↓ 只移动高亮（不再每移一行就重开详情面板、
  把焦点抢走），Enter 才打开当前行；鼠标单击仍是点即开。判据是
  `Window::last_input_was_keyboard()`，两个表（环境 / 抓包）同一套。
- **行高回归断言**：L0 给环境表前三行注册 `debug_selector`，量相邻两行的原点差
  （真实行高）并断言一致 —— 行内加东西导致某一行变高时，这条会先红。
- **CA 屏动作按钮不再被拉成整行横条**：`GroupBox` 内容区是 `v_flex`，子项沿交叉轴
  stretch，按钮包一层行容器后回到自己的宽度。
- **CA 屏两列**：窗口 ≥1160 时「摘要 / 文件 / 签发」与「信任 / 轮换 / 如何使用」并成
  两列，六个分组一屏看全；窄窗口仍是单列。两列暴露的"步骤文本不换行被裁"一并修掉
  （文本要包一层吃剩余宽度的容器）。
- **环境屏状态行不再重复计数**：工具条四档计数已经给出总数，状态行只在过滤/选中时
  说话（`显示 x / y 个环境`、`选中 …`）。
- **规则 / 代理屏的计数并进工具条**（搜索 → 弹性 → 计数 → 主操作），各屏省一行给表格。
- **规则 / 代理空账本给就地动作**：空态里直接放「导入规则集」/「新建上游代理」，
  与环境屏空态同一写法（不再只指向右上角）。
- **调试屏筛选条可收起**：工具条新增「筛选 (n)」开关，默认收起、不占一行；已有条件时
  强制显示（否则条件没有入口可改回）。筛选只在方法 / 状态档 / 主机任一非空时计数。
- **抓包时间可看日期**：表里仍只放时分秒（列宽宝贵），悬停给 `YYYY-MM-DD HH:MM:SS.mmm UTC`；
  详情面板 meta 直接给完整日期 —— 跨天长会话不再"两条 09:15 分不清哪天"。
- **环境删除收进「更多」**：删除是不可逆动作，不再与重启并排成一个图标；行末 `…`
  菜单里一项「删除 <env>」，仍走确认弹窗。
- **代理地址列改等宽**：`host:port` 与监听列、抓包表同一读法。

### Added（CA 有了自己的身份：轮换、按域名签发、系统信任库只读探测）

- **不再借用 mitmproxy 的 CA**：`crates/engine/src/ca.rs` 只读写
  `envboard-ca.pem`（私钥+证书）与 `envboard-ca-cert.pem`（只含证书），CN = `envboard`。
  同目录下的 `mitmproxy-ca*.pem` **不再被复用**：借用别的工具的 CA 身份意味着
  "这个私钥是谁签的"永远说不清，也没法独立轮换。旧文件**不删**（那是用户的文件），
  首次生成时回一条 `Regenerated { reason: "legacy mitmproxy CA is no longer reused" }`
  的结果。
- **重新签发（轮换）**：`SharedCa::rotate` 生成新 CA、覆盖两个文件，并经
  `CaStore`（`arc_swap`）换入运行中实例 —— 引擎每连接读一次当前 CA，所以**下一条
  CONNECT 就用新证书，不必重启环境**。旧证书不从系统信任库移除：那是用户的决定。
- **按域名签发**：`CaControl::issue_pem(host)` 用当前 CA 现签一张叶子证书，返回
  证书与私钥 PEM（界面只经剪贴板交付，不落盘、不进日志）。用途是把某个服务固定到
  一张具体证书上。
- **系统信任库探测（只读）**：`crates/host/src/trust_store.rs` 用 crypt32 的直接声明
  （`CertOpenStore` / `CertEnumCertificatesInStore`，不引 `windows-sys`）枚举
  `当前用户\ROOT` 与 `本机\ROOT`，与当前 CA 的 DER **逐字节**比较；任一命中即
  "已安装"。**只检测，不安装也不卸载** —— 写系统存储会弹安全警告，那是用户的动作。
  探测不了（非 Windows / API 失败）时如实说"不知道"，不猜"未安装"。
- **CA 屏重做**：证书摘要 / 系统信任（两行存储状态）/ 证书文件 / 按域名签发 / 重新签发
  （确认弹窗 → 轮换 → toast 出新指纹）/ 如何使用。**CA 私钥不进界面**：界面上出现的
  只有 `envboard-ca-cert.pem` 与它对应的 PEM。
- 用例：`ca` 四个单测（新目录拿到 envboard 命名的 CA、legacy 文件只忽略不复用、
  轮换替换 CA 与文件、按域名签发签出请求主机的叶子）+ `trust_store` 两条状态判据。

### Added（MITM 白名单 `mitm_hosts`：只解密点名的域名）

- 新环境字段 `mitm_hosts`（第 9 个字段）：**空 = 全部解密**（与引入它之前完全一致），
  非空 = 只有列出的域名被解密，其余 `CONNECT` 走**盲隧道**（`copy_bidirectional`
  原样抄字节：不解密、不抓包、不记轨迹、不改写内容）。
- 归一化/校验/去重排序与 `insecure_hosts` **同一份实现**（精确相等、拒绝通配符、
  ≤200 条），因为两者回答的是同一个问题"这个域名算不算在内"，只是方向相反。
- **鉴权仍必须在 CONNECT 上判**：隧道里看不到 HTTP，所以 CONNECT 没带正确的
  `Proxy-Authorization` 就 407 —— 否则名单外的域名成了绕过代理鉴权的通道。
- **白名单只管解密，不管路由**：hosts 规则照旧改写隧道要连的目标地址。同一份规则在
  解密/不解密两种情形下连到不同的地方，用户无从预期；是否解密是显式选择，连到哪
  不该跟着变。
- 热字段：判定在每连接的配置快照上做，换名单下一条 CONNECT 即生效；`config_hash`
  的输入形状也加上了它（否则"只改白名单"的 apply 在自身幂等判据里看不见）。
- 打通整条链：domain（`KNOWN_FIELDS` / merge / JSON）→ `EngineSpec` → 管理器投影与
  热/停机矩阵（`mitm_hosts` 进热字段集合与审计 `fields`）→ 协议 DTO（`EnvView` /
  create / patch）→ 桌面「配置 → 高级配置」表单。
- 用例：引擎 `mitm_allowlist_decrypts_only_listed_hosts`（名单内走完 MITM、名单外
  明文直通证明没解密、缺凭据 407、带凭据放行）、管理器热改 + 落盘 + 清空、
  契约 fixture 5 枚（环境校验 3、merge 2）。

### Changed（CA 换了身份：已装证书的客户端必须重装）

- 这是本次唯一的**破坏性**变化：以前客户端信任的是借用来的 mitmproxy CA，现在签发
  链的根换成了 `envboard`。旧证书继续留在系统信任库里（我们不删），但对新链路无效，
  客户端会报链校验失败 —— 重新安装 `envboard-ca-cert.pem` 即可。
- `shared/confdir/` 下因此会同时存在两套文件；`mitmproxy-ca*.pem` 从此不再被读取。

### Fixed（按住标题栏拖不动窗口）

- **根因是标题栏里那层"吞掉按下事件"的拦截**：`crates/desktop/src/app.rs` 的
  `render_title_bar` 在内层 div 上挂了 `on_mouse_down` + `cx.stop_propagation()`
  （当初是为了标题栏里的深色开关；开关搬进「设置」后它就成了残留）。而自绘标题栏
  在 Windows 上靠系统的 `HTCAPTION` 拖动：`WM_NCLBUTTONDOWN` **先**交给应用的输入
  处理器，一旦 `propagate = false`，gpui-pre-windows 的 `handle_nc_mouse_down_msg`
  直接返回 `Some(0)`，`DefWindowProc` 收不到这条消息，拖动根本不会开始。
  gpui-pre 0.3.9 的 `start_window_move` 在 Windows 上是空实现，所以"应用自己拖"
  这条退路也不存在 —— 拦掉系统那条就等于完全拖不动。
- **改法**：删掉拦截与那个 handler；标题栏里只留文字，纯展示元素不该吞事件。
  将来要往栏里放可点控件，才需要控件自己吞按下（否则系统会开始拖窗口，控件只能
  收到按下、收不到抬起）—— 这条纪律写进了 `app.rs` 的模块文档。
- **证据边界**：远端会话当前是锁屏状态（`SetForegroundWindow` 返回 false、前台是
  "Windows 默认锁屏界面"），合成鼠标输入全部落在锁屏上，**没做成端到端拖动验证**。
  做成的两项：①`WM_NCHITTEST` 在标题栏（client 300,17）返回 `HTCAPTION`、内容区返回
  `HTCLIENT`；②对照实验里旧包（`dist/` 里修复前那份）与修复后的
  `WM_NCLBUTTONDOWN` 都会在 10ms 内返回（无真实按键时移动循环立即退出），这条判据
  分辨不出差别，所以不作为结论。真实拖动需要解锁后在界面上按一次确认。

### Fixed（双击启动不再附带一个控制台黑框）

- **release 二进制原来是 CONSOLE 子系统**：从资源管理器双击时 Windows 会为它分配一个
  控制台窗口（一闪而过，或一直挂着），一个窗口应用看起来像个命令行工具。现在
  `crates/desktop/src/main.rs` 首行声明 `#![cfg_attr(not(debug_assertions),
  windows_subsystem = "windows")]`：release（也就是打包分发的那份）是 GUI 子系统，
  双击没有黑框；debug 保留控制台，`cargo run` 的日志与 panic 回溯照旧可见。
- 实测：release 产物 PE `Subsystem = 2 (GUI)`；launch 后只有 `envboard` 一个带窗口的
  进程；重定向到文件时 stderr 仍能收到错误文本；无 stderr 接收者时启动失败对话框照常
  弹出（GUI 子系统不改变那条判据）。

### Fixed（双击启动的失败不再"闪退"成谜）

- **GUI 子系统没有控制台，双击启动时的致命错误只写进虚空**：从资源管理器双击运行
  `envboard.exe`，启动失败（状态目录不可写、锁被占、开窗失败）表现为"窗口一闪就
  没了"，错误文本谁也没看到。现在 `crates/desktop/src/main.rs` 在**stderr 没人读**
  时用 `MessageBoxW` 把同一句错误弹出来（`state_dir` 类失败再附一句换目录的办法），
  并装了 panic 钩子覆盖 `open_window` 之类的崩溃路径。
- **判据是"有没有人读"，不是"句柄在不在"**：附着控制台（`GetConsoleMode` 成功）
  或输出被重定向到文件/管道（`GetFileType` 为 disk/pipe）时不弹 —— 否则一条无人
  应答的模态框会把 CI 与脚本卡死。`cmd start` 那种继承来的、但进程并未附着的控制台
  句柄，归入"要弹"这一侧。
- 只为一次 `MessageBoxW` 用 `#[link]` 直接声明了 `GetStdHandle` / `GetConsoleMode` /
  `GetFileType` / `MessageBoxW` 四个符号，没有引入 `windows-sys` 直接依赖（不牵动
  依赖门禁与锁文件）。

### Added（绿色免安装分发：便携模式 + 打包入口）

- **便携模式**：exe 同目录存在 `<应用名>.portable` 标记时，状态目录取同目录的
  `<应用名>-data`（`crates/host/src/options.rs` 的 `portable_state_dir`）。判据是
  **标记文件**而不是目录是否存在 —— 目录由应用首次运行时自己创建，拿它当判据在
  首次运行之前永远不成立。基准是 `current_exe()` 而不是进程 cwd：数据在哪不能由
  启动方式决定。回退链因此变四级：调用方给的目录 → `$ENVBOARD_STATE_DIR` →
  便携标记 → 平台默认；**环境变量压过标记**，绿色包里想临时换位置不必去动发行包。
- **`scripts/package.py`**（`mise run package-portable`）：`cargo build --release
  --locked --offline` → 组装 `<exe> + <exe>.portable + README.txt` → 打成
  `dist/envboard-<版本>-<平台>-portable.zip`。33.0 MiB 的 exe 压出来 11.3 MB。
  数据目录**不预建**，脚本不签名、不写注册表、不改 PATH。
- **`--static-crt`**：静态链接 CRT 的变体（独立目标目录 `target/crt-static`，
  产物名多一个 `-static`）。实测去掉 `VCRUNTIME140.dll` 依赖、只多 174 KB，
  代价是首次多 3~4 分钟构建 —— 给"目标机器没装 VC++ 运行库"的场景兜底。
- README 新增「打包与分发」：单文件 exe 的运行时依赖（系统 DLL + `VCRUNTIME140.dll`）、
  压缩版的目录形态与"整个目录搬走 / 删目录即卸载 / 一个副本一个实例"的语义、
  SmartScreen 与杀软误报的现实。

### Changed（错误呈现统一为"错误码 + 英文 msg + 中文提示"）

- **错误条不再只显示服务端英文**：多处调用点原来把 `Error` 拍平成 `error.message`
  直接扔进界面，用户看到的是 `debug session vanished immediately` 这种句子混在
  一整屏中文里（走查 F10）。现在统一走 `crate::errors::Notice`：
  - 第一行是**中文主句** `{动作}{对象} 失败：{该错误码的中文结论}`
    （`启动 dev 失败：端口已被其他程序占用`），由 `ErrorCode` 穷尽映射；
  - 第二行是**契约错误码 + 服务端原文**（`[port_conflict] port 8080 already in use
    · field: environment.listen.port`），原样保留 —— 端口号、路径、占用人只在这句里。
  - 契约新增一节「界面呈现」（`spec/errors.md`），把三层分工与四条硬要求写死：
    不得只显示英文、不得只显示中文、码不翻译、动作要点名。
- **写操作的失败不再被下一帧快照抹掉**：取数成功原本会无条件清空错误条，而写操作
  失败后紧跟着的那次 `refresh_now()` 正好是一次成功取数 —— 于是「删除失败」只闪一帧。
  现在区分来源（`error_from_load`）：取数失败在下次成功取数时消失，写操作失败保留，
  直到下一次写操作替换它。这条直接决定上一条修完之后用户**看不看得见**。
- **错误只有一种长相**：详情面板的「读取规则失败」原本是页面里的一行红字，
  现在与表格上的错误条同一个组件、同一个两行结构。

### Fixed（图标静默画空白：资产源只注册了默认的 106 个）

- **代理屏的「编辑」按钮是一枚看不见的幽灵**：按钮在、点击区在，但图标是空白的 ——
  铅笔用的是 `gpui_kit::assets::IconName::Pencil`，而入口注册的是
  `gpui_kit::assets::Assets`（组件默认的 **106 个**图标），**完整目录里才有铅笔**。
  GPUI 对不在资产源里的名字不报错、不占位，只画一个空白 svg —— 于是"编辑上游代理"
  唯一入口从界面上消失，只剩删除。入口与截图夹具一并改用 `AllAssets`（完整目录，
  1830 个，嵌入 +2.8 MB）：漏图标的缺陷从"看起来少了个按钮"变成不可能发生。
  夹具也跟着改 —— 夹具与产品注册不同的资产源，L1 就查不出这类缺陷（此前正是如此）。

### Fixed（代理设置命令从 URL 中间折断）

- 详情面板的 `export https_proxy=http://… http_proxy=http://…` 按字符折行，
  断点落在 `http://` 与 `/127.0.0.1:17103` 之间，读起来像两条命令。
  现在按**赋值边界**拆成多行（`display::command_lines`，纯函数 + 单测），
  行内不折行；复制按钮改成图标钮，把宽度让给命令本身（带文字时最后几位端口会被挤出去）。

### Changed（最小窗口宽度 880 → 1000：旧值是用不满的界面量出来的）

- 环境表的固定列合计 596、说明列下限 100，加侧栏 248 与内边距 32 得 976 —— 也就是
  **最小窗口必须容得下"操作列"这唯一入口**。880 时「删除」会被裁出视口（启停与重启
  只露出半个），最小尺寸等于没有约束。取 1000 留 24 余量；`--size 1000x560`
  的 L1 截图已复测三枚操作图标全部可见。

### Fixed（`--state-dir` 在 Windows 上必失败：回退链断在 `$HOME`）

- **Windows 上不给 `--state-dir` 就起不来**，报
  `invalid_config: cannot determine a state dir; pass --state-dir`。
  根因是回退链的第 3 级写死了 `$HOME/.envboard`，而 **`HOME` 不是 Windows 的
  约定** —— 原生进程通常根本没有这个变量（本机实测：`cmd /c set HOME` 只有
  `HOMEDRIVE`/`HOMEPATH`，没有 `HOME`）。于是链在 Windows 上直接断掉：用户明明
  在 Windows 上，程序却要求他显式传路径。
  修法：第 3 级改为**平台默认目录** —— Windows `%LOCALAPPDATA%\envboard`
  （本机应用数据的约定位置），其它平台 `$XDG_STATE_HOME/envboard`，未设时回落
  `~/.local/state/envboard`（XDG 基础目录规范）。取不到就取不到（响亮失败），
  不猜一个猜错的目录 —— 猜错的后果是账本写到别处，症状是"环境列表空了"。
- **`--state-dir` 的值不再原样使用**：新增**规范化**（`normalize_state_dir`）——
  `~` 展开成用户主目录、相对路径补成绝对路径、空串按非法配置拒掉。
  三条各自对应一类真实报障：`~` 是 **shell** 的展开规则不是路径规则（直接传进
  程序会得到一个字面目录名 `~`，本仓仓库根真的躺过一个 `~` 目录）；相对路径的
  基准取决于进程 cwd（换个启动方式就指到别处）；空路径不是"默认"而是"没给"。
- **`--state-dir` 的错误文案**补上了第二条出路（`or set ENVBOARD_STATE_DIR`）。
- **`store_failure` 不再是一条没有路径的裸 IO 错**：新增 `Error::io_at`，状态目录
  相关的每一步（建目录、读写 CA、单实例锁、读写账本）都写明它动的是哪个文件。
  原来只打印 `store_failure: io error: 拒绝访问。 (os error 5)` —— 没有路径，
  而"哪个目录被拒"正是排障的唯一线索（本机实测：`icacls <exe>` 显示 exe 所在
  目录带 **Low 完整性标签**，进程继承 Low 完整性后写 `%LOCALAPPDATA%` 一律被拒，
  读却正常）。现在同一情形报
  `cannot create the CA directory C:\…\shared\confdir: 拒绝访问`。

### Removed（命令行参数全部出库：产品是 GUI，不解析参数）

- **`clap` 与全部命令行参数出库**：`--state-dir`、`--port-range`、`--capture-budget`、
  `--log-dir`、`--no-log-file`、`--max-log-bytes` 一并去掉，连同 `--help` / `--version`。
  判据是**删掉之后能力有没有损失**，而不是"入口有几个"：
  - `--state-dir` 删了不损失 —— 平台约定兜底，用户想换还有 `$ENVBOARD_STATE_DIR`。
    它本来就是同一件事的第二种说法（web 宿主与桌面宿主共用一个目录的时代留下的）。
  - `--port-range` 删了也不损失 —— 区间只负责**给新建环境随机分配端口**；某个环境
    端口冲突时在界面里改那一个环境的端口即可。区间的默认值是域常量
    （`engine::domain::DEFAULT_PORT_RANGE`），本来就只定义一次。
  - `--capture-budget` / `--log-dir` / `--no-log-file` / `--max-log-bytes` 同样是
    "藏在看不见的地方的配置"：这是从启动器点开的 GUI，用户永远拿不到这些参数。
    日志跟随状态目录布局落盘（`<state_dir>/logs`），路径信息将来进设置屏。
  - 代价（记在账上）：排障时不能再从终端把日志指到别处、或把预算调大。要恢复这类
    能力，正确的位置是**设置屏**（看得见、能持久化），不是命令行。
- 带参数启动不报错（可能只是从一个带参的快捷方式点开），但在 stderr 给一行
  "不接受命令行参数（已忽略）"的提示 —— 静默忽略会让人以为生效了。
- **配套门禁**：`deps` 第 12 条禁止任何 crate 的**产物依赖**里出现命令行解析库
  （清单封闭：`clap` / `structopt` / `argh` / `pico-args` / `lexopt` / `gumdrop` /
  `bpaf` / `xflags` / `argparse`）。理由与第 11 条（gpui-kit 的体积守卫）同形：
  这条决定只写在注释里，下一个人"顺手加个 `--verbose`"就能无声破掉它。
- `HostOptions` 的字段全部**保留**（`state_dir` / `port_range` / `log_dir` /
  `no_log_file` / `max_log_bytes` / `capture_budget`）：组合根仍要能被"直接给值"地
  使用，测试与示例靠的就是它们，只是本入口一律用默认值。`parse_port_range`
  （文本形状解析）随之删除——它只服务命令行。
- **环境变量空串等同未设置**：`$ENVBOARD_STATE_DIR` 现在是唯一的外部输入，
  `ENVBOARD_STATE_DIR=${SOME_UNSET}` 这种写法很常见，照空值用会得到一个空 `PathBuf`。
  错误文案也从 `pass --state-dir or set ENVBOARD_STATE_DIR` 收敛成
  `set ENVBOARD_STATE_DIR`。

### Changed（平台默认目录：补齐 macOS，并把读环境变量的两条规范纪律落地）

- **macOS 有了自己的默认位置**：`~/Library/Application Support/envboard`。
  之前只有「Windows / 其它」两支，macOS 会掉进 Linux 的 XDG 规则拿到
  `~/.local/state/envboard` —— 那里没有任何 macOS 原生工具会去找东西，
  而症状是"起了个空工作台"（账本在另一个目录），从界面上看不出来。
  macOS 是唯一没有 `state` 档的平台（`FileManager.SearchPathDirectory` 里既无
  `state` 也无 `logs`），所以这一档只能落在 Application Support。
- **三个默认位置的共同轴是"机器相关的应用数据"**，不是"用户的文档"，也都**不在主目录根**
  （`~/.envboard` 那种放法 Windows 明文禁止，macOS/Linux 同理）：
  Windows `%LOCALAPPDATA%`（定义即 nonroaming，不选 `%APPDATA%` 是因为状态里含端口
  分配与单实例锁，漫游会让两台机器打架，CA 私钥更没有漫游的理由）、
  macOS `~/Library/Application Support`、Linux `$XDG_STATE_HOME`。
- **空串等同未设置**：三个平台变量取到空值时继续往下回退。原实现会把空值
  `join("envboard")` 成一个**相对路径**，等于把状态写进进程 cwd。
- **相对路径的 `$XDG_STATE_HOME` 视为无效并忽略**（XDG 原文
  `should consider the path invalid and ignore it`），回落到 `$HOME/.local/state`。
- 现状由**纯函数** `default_state_dir_for(platform, …)` 决策，三平台规则因此在
  任意宿主上都能被单测覆盖（本仓没有 Mac 构建机）。
- **应用目录名不再硬编码**：`<基础目录>/<名字>` 里那一层改由宿主二进制从
  `Cargo.toml` 的 `[[bin]] name` 取（`env!("CARGO_BIN_NAME")`）再传进
  `HostOptions::app_dir_name`。理由是它属于**产品身份**而不是路径规则 ——
  `envboard-host` 是通用组合根，没有资格知道产品叫什么。连带效果：
  `HostOptions::default()` 现在**没有**平台默认目录（宁可响亮失败，也不编一个
  名字把账本写进 `%LOCALAPPDATA%\unknown`）；显式给了状态目录时不需名字。
  命令行程序名也改引同一个常量，改名只需动 manifest。

### Changed（状态目录布局：路径规则收口成一份，轨迹不再跟着日志目录走）

- **布局字面量收口到一处**：`crates/manager/src/state.rs` 新增 `layout` 模块，
  登记状态目录里的全部路径（账本、界面偏好、锁、事件、规则目录、日志目录、
  轨迹目录、共享 CA 目录、v2 遗留的 agent 目录）。此前**有两份实现** ——
  `state.rs` 派生一套，桌面形态的 `ui_prefs.rs` 又自己拼了一套 `"ui.json"`；
  两份独立演化，改一处忘另一处的症状是"文件在、路径对，就是读不出来"，从外部
  几乎无法归因。界面形态现在从组合根拿**算好的路径**（`Prepared::ui_prefs_file` /
  `Host::ui_prefs_file`），不再自己 `join`。
- **请求轨迹从 `logs/trajectories/` 移到 `trajectories/`**（状态目录下与 `logs/`
  平级）：轨迹是数据面事件流（自己的字节偏移游标与生命周期），不是实例日志。
  原来它挂在 `log_dir` 下面，于是 `--log-dir` 改址会**顺带搬走轨迹** ——
  一条与调用方意图无关的副作用。轨迹现在**位置固定**（跟状态目录走）、
  **开关跟 `--no-log-file` 走**（要不要落盘），两件事各归各。
  此前**代码与文档不一致**：代码写 `logs/trajectories/`，而 `README`、
  `spec/events.md`、`spec/protocol.md`、`spec/capabilities.md` 一律写
  `trajectories/` —— 现在代码对齐文档。
- **删掉两个死字段**：`ManagerConfig::runtime_dir`（web 形态的 `runtime/api.json`
  随 web 宿主出库后就没人用了，全仓零引用）与 `ManagerConfig::rotated_log_file`
  （轮转路径的**第二份实现**，用 `with_extension` 而实际轮转走
  `logs::rotated_path` 的 `with_file_name` —— 两份实现的语义还不一样；零调用方）。
  v2 遗留的 agent 目录改为 `ManagerConfig::legacy_agent_dir`：它只为**清扫**
  存在（删环境 / 改名时带走残留），产品路径永不创建它。

### Added（状态目录布局门禁：布局字面量只许出现在一处）

- 新增 `tests/policy-tests/tests/state_layout.rs`（两条判据）：
  ① **布局字面量只许出现在 `state.rs`** —— 别的文件里出现即红；判据按
  "**它被拼进了路径**"判（只看 `.join("…")`），而不是"字符串出现了" ——
  `"rules"` 这种**领域字段名**（环境绑定的规则集名）到处都用，它不是路径，
  按字面量判会红在毫无关系的地方（红在无关处，门禁就会被当成噪声忽略）；
  ② **布局表必须登记齐全** —— 少一项即红，防止"新加一个目录忘了登记"，
  那等于给布局开了个没锁的口子。
  门禁上线时当场抓到五处真违规（桌面形态的 `ui.json` 两处、管理器测试的
  `state.json`、引擎实机组两处），都已改为引用布局表或换成不涉布局的路径。

### Fixed（L0 主题用例的假红：上一轮的 `ui.json` 污染下一轮）

- **L0 的 `theme_dropdown_applies_and_persists` 会稳定假红**：测试的临时状态目录
  用固定名字，上一轮写下的 `ui.json`（界面偏好）留在那儿，下一轮就从
  "偏好已经是深色"起步 —— 于是测的变成"本来就是深色 → 皮肤却是浅的"。
  一次失败之后这条用例就再也绿不回来（HEAD 同形，本轮才定位到）。
  修法两条：① 每次跑用**带纳秒时间戳的新目录**（进程号会被复用）；
  ② 临时目录改用 `Drop` 守卫，**断言失败（panic）时也清掉 `ui.json`** ——
  之前的清理写在测试体末尾，panic 就把它跳过了。
  为什么不能"逐个删旧目录"：Windows 上文件被进程攥着时 `remove_file` 会成功
  但文件仍留在那儿（删除挂起），"清干净"这件事做不到。

### Changed（桌面形态 UI 打磨：层级、状态与交互细节）

- **状态过滤从四枚独立 ghost 钮改成一段分段控件**（`ButtonGroup` + `outline`）：
  四枚彼此独立的按钮看上去像四个可随手点的标签，而不是"从四档里选一档"。
  选中态仍靠组件自己的填充说话（不另加左侧色条 —— 那是 web 模板习惯）。
- **工具条的「新建」不再用 `primary`**（环境 / 规则集 / 上游代理三屏）：`primary`
  留给决策区里那个明确的提交动作，工具条里的"新增一条"只是普通命令。
- **错误统一成 `Alert` 提示条**（环境 / 规则集 / 代理 / 调试 / 配置表单五处）：
  一行淡红字没有形状，读者要靠猜。图标 + 表面 + 边框把"这是错误"说清楚，
  服务端消息仍原样转述。
- **空态分两种说**：「还没有 X」与「没有匹配」的下一步不一样（新建 vs 改搜索词），
  三张表各自给出对应文案；调试屏的空态改为居中一列（原来偏上）。
- **健康列加了形状标记**：状态不再只靠颜色（色觉障碍与灰度打印读不出来）——
  文字前一个同色圆点，文字仍写中文标签。
- **行内操作的 tooltip 与无障碍名都说清对象**（"停止 beta"而不是"停止"），
  指针停在某一行的钮上时能知道它管的是哪个环境。
- **状态栏的装配警告有了形状**：原来是一段纯文字（警告数为 0 时还是空串占位），
  现在是带三角图标的警告色；侧栏底部改放端口区间，状态目录回到状态栏
  （同一件事只说一处，路径也才读得全）。
- **「新建环境」的校验原因写进弹窗里**：原来写到表格上方的错误条，而弹窗是
  模态 —— 那条错误被弹窗挡住，等于没人看见。现在弹窗自己有一行原因。
- **配置页签的密码框打码**（`InputState::masked` + 掩码切换钮 + `Password`
  内容类型）：凭据仍然只进不出，但输入时旁人看不见（此前是明文输入，
  代码里记着"缺 mask 构造器"的已知缺口，实际 `gpui-base` 里就有）。
- **高级配置区改用组件库的 `Collapsible`**：展开/收起的内容挂载交给组件，
  触发器仍是普通按钮。
- **CA 屏的"已复制"改成轻提示**（`Notification`）：原来是一行常驻文字，
  永远不消失，下一次复制时分不清是上一次的反馈。
- **状态栏与工具条的计数不再各说一遍**：环境屏那一行改成"可见 / 总数 + 选中项"。

### Added（L0 覆盖分段过滤 + 截图工具能拍任意一屏）

- 新增 `the_status_filter_switches_to_the_clicked_tier`：按 `debug_selector`
  锚点拿到分段控件的矩形 → 点第三档 → 断言过滤档真的切到「已停止」。
  为此 `Workbench::environments()` 与 `Environments::filter()` 两个只读入口
  （视图层的状态本来就该可观察），`Filter` 也从 crate 根导出。
- `examples/screenshot.rs` 加第二个参数（屏名）：`cargo run -p envboard-desktop
  --example screenshot -- <输出路径> <environments|rules|proxies|debug|ca|settings>`。
  切屏走**真实路径**（派发鼠标 down/up 命中组件自己的 on_click），不是直接改
  字段 —— 摆拍出来的图不能证明界面真的能切过去。派发必须等第一帧画完：
  命中测试靠的是已画出的命中框，首帧之前派发等于点空气。

### Added（CA 独立成屏 + 界面验收三层落地）

- **CA 屏**（`crates/desktop/src/views/ca.rs`，侧栏「CA」，图标 `CircleCheck`）：
  共享根证书从设置页里搬出来单独成屏 —— 摘要（`DescriptionList`，标签列 120 px 以免
  「指纹 (SHA-256)」折行）、证书文件（路径 + 复制路径 / 复制 PEM + "所有环境共用这一张"
  脚注）、如何使用（三步）。私钥纪律不变：`CaBundle` 只有证书 PEM 与摘要。
- **L0 无头界面测试扩到两条新链路**（`crates/desktop/tests/ui_l0.rs`）：
  点侧栏「CA」真的切屏；进设置屏 → 按 `debug_selector` 锚点找到主题下拉 → 真点开 →
  真点「深色」→ 断言**偏好全局、组件皮肤、`ui.json` 落盘**三处同时变。
  侧栏项由组件绘制、不注册 selector，坐标只能按位置点，故集中在常量里并写明"改侧栏必须重探"。

### Changed（设置页规范化）

- 设置页版面换成组件库的设置几何：每组一个 `GroupBox` **描边**表面，组内一行一项
  （标题与说明在左、控件在右，`SettingItem` 同一横向布局），"共享证书"段整段搬去 CA 屏。
  为什么是描边而不是填充：暗色主题下 `group_box` 填充色与窗口底色几乎同色，
  分组边界直接消失（浅色看不出问题）—— 判据与截图见 spec/desktop-parity.md。
- 主题档位文案统一为**浅色 / 深色 / 系统**（原「明亮 / 暗色 / 跟随系统」）。
  只改显示文案：`ui.json` 里仍是 `light` / `dark` / `system`，旧文件照读不误。
- 侧栏目的地从 5 个变成 6 个；`every_screen_in_the_sidebar_is_shipped` 改为遍历
  `Destination::ALL`，加屏不再需要两处各列一遍清单。

### Added（P2 部分落地：调试/抓包屏、设置屏（证书与主题））

- **调试屏**（`crates/desktop/src/views/debug.rs`，侧栏「调试」）：目标环境从环境快照里
  选（没在跑的环境列出但不可选），开始即开工作区级抓包单例；记录表给
  时间（**UTC**）/ 方法 / 主机 / 路径 / 状态 / 大小；500ms 增量取数按 `request_id`
  续游标（清空时一起归零）；选中一行开 `Sheet`，请求 / 响应两页签各给起始行、头与
  正文（二进制标 base64、超限标 omitted、超长正文截断并注明行数）；「清空」「停止」
  都与门面语义一致（停止 = 停记录 + 清记录）。
- **设置屏**（`views/settings.rs`，侧栏「设置」）：外观、共享证书、关于三段。
  证书段读组合根交付的 `CaBundle`（摘要 + 证书 PEM），证书路径由
  `envboard_engine::ca::cert_path` 单点给出（新增 `Prepared::ca_dir`，界面不自己拼
  文件名）；「复制路径」「复制 PEM」走剪贴板；私钥仍然不在任何交付给界面的结构里。
- **主题三档：明亮 / 暗色 / 跟随系统**（下拉框，`Select`；`crates/desktop/src/ui_prefs.rs`）：
  偏好写入 `<state_dir>/ui.json`（坏文件回落到"跟随系统"，**永不拦启动**；写失败在
  设置屏显示原因）；启动时在开窗之前落实，窗口外观变化时只有"跟随系统"这一档
  重新落实。`gpui-component` 的 `ThemeMode` 只有两档，"跟随系统"由
  `Theme::sync_system_appearance` 承担。
- **标题栏的深色开关已移除**：一个两态开关表达不了"跟随系统"，两处都能改主题还会
  互相打架。标题栏现在只有标题（仍是拖拽区），主题的唯一入口在「设置」。
- 状态栏右侧增加当前主题，切换后立刻可见。

### Changed（P2 顺带）

- 外观段的主题控件从三档单选（`RadioGroup`）换成**下拉框**（`Select`）：触发器显示
  当前档位，展开后三档带勾选；`ThemePreference::from_index` 随之换成 `from_label`
  （下拉项的值就是显示文案）。
- `Host` 增加 `ca()` / `ca_cert_path()`（只给目录与按引擎规则算出的路径，不给私钥）。

### Removed（web 形态整体下线：只留桌面形态）

P1 执行，判据与代价见 docs/adr-003-retire-web-frontend.md，行为对账前置证据见
spec/desktop-parity.md。删除规模：40 个文件、12,545 行；`Cargo.lock` 848 → 837 包。

- 出库：`frontend/`（Vite + React 19 + HeroUI v3 + Tailwind v4，7,313 行）、
  `crates/web`（axum 路由 + SSE 三流 + token/CSRF/CSP 中间件 + `include_dir!` 内嵌 +
  二维码，1,540 行）、`crates/server`（web 宿主二进制与它的 `smoke`/`artifact`/
  `live_workbench` 三套测试，3,026 行）、`scripts/systemd/envboard.service`、
  `spec/ui.md` + `spec/design.md` + `ui_style` 门禁。
- 依赖出库：`axum` / `tokio-stream` / `futures-util` / `include_dir` / `qrcode`。
- **接受的损失（明确记录，不是待办）**：远程访问（浏览器从别的机器管理工作台）、
  无头常驻服务（systemd 用户服务）、脚本化管理面（`curl` 打 HTTP API）、
  以及 CA 的「下载 pem / 扫码装证书」两条端点。
- 唯一发布物更名：`crates/desktop` 的 `[[bin]]` 由 `envboard-desktop` 改为 `envboard`
  （旧宿主出库后这个名字空出来），`clap` 的 command name 与 `app_id` 同步。
- 门禁同步：`deps` 允许表与装配盲区表删掉 `envboard-web` / `envboard-server` 两行
  （想带回来必须改表 = 评审点）；`toolchain` 门禁语义从"圈禁前端工具链"改为
  **回归守卫**（仓库里已无合法容身处，任何 Node/TS 面出现即红）。
- 发布物判据拆两半各归其位：仓库文件面（必需文件、历史残留为零、`target/` 被忽略、
  四件 CA 测试资产必须在库）→ 新门禁 `tests/policy-tests/tests/artifacts.rs`；
  二进制内容（产物里真的带着进程内引擎的线程名标记）→
  `crates/desktop/tests/artifact.rs`（只有产物 crate 的集成测试拿得到
  `CARGO_BIN_EXE_envboard`）。
- 验证分层收窄：`scripts/verify.py` 去掉 `frontend` 与 `artifact` 两层，默认层变为
  policy + rust + contract；`live` 层只剩引擎侧 `live_manager`（走 HTTP 的 29 条
  实机断言随宿主出库，其覆盖的行为改由门面直连验证）。
  `mise.toml` 去掉 node/pnpm 工具钉与 `dev`/`build`/`typecheck`/`verify-frontend` 任务。
- 文档改写：`spec/admin-api.md` 从 HTTP 端点表改写成 **AdminService 方法契约**（v2.0，
  方法语义与形状一条没改）；`spec/protocol.md` 从"前后端通信协议"改为"界面边界词汇"，
  SSE 帧语义保留为界面的取数节奏（游标域与 advisory 语义不变），HTTP 安全三档
  （Host 校验 / token / CSRF 头）标注为"要把传输面带回来时必须一起回来"；
  `spec/events.md` / `spec/capabilities.md` / `spec/errors.md` 里的端点指称改指门面方法
  （errors 的 HTTP 列降级为非契约的历史刻度）；README 全面改写（构建不再有两条工具链、
  快速开始去掉 curl、启动参数删 `--listen`/`--token`、"运维（systemd）"整节退场）。

### Added（桌面形态四屏齐平：详情面板、配置与规则、规则/代理目录）

P0.5–P0.8 四个里程碑一起补上，逐屏对账见 spec/desktop-parity.md。

- **详情面板（P0.5）**：`Sheet`（可拖宽、非遮罩、Esc 关闭）+ `TabBar` 三页签，
  概览用 `DescriptionList::horizontal()`。面板是独立实体，读同一份表数据，
  写操作仍回到环境屏发（一处真相）。凭据永不回显与"代理命令由服务端回显"
  两条纪律照搬。
- **配置页签（P0.6）**：`ConfigForm` 实体，字段与 web 一一对应并逐项标注热生效/
  停机生效；脏状态取"任一输入动过"的粗粒度，放弃修改 = 丢实体按当前环境重建。
  保存经 `hop` 回 tokio，成没成都补一次快照。
- **规则页签（P0.7）**：正文按行渲染（等宽），未绑定/缺失/失败三态分开说；
  规则集与上游绑定用 `Button::dropdown_menu` + `PopupMenuItem::checked()`
  —— 这里要的是"可清空的单选"，不是集合搜索。
- **规则集目录屏 + 上游代理目录屏（P0.8）**：`crates/desktop/src/views/catalog.rs`。
  引用反查与"生效条数"都从**同一份**环境快照派生（`EnvTable::referenced_by_rules` /
  `rules_count_of`），不另算一套；代理被引用时删除钮禁用并说明原因；规则集允许删，
  但确认文案把正在引用它的环境名列出来。侧栏三屏自此全部落地，
  `every_screen_in_the_sidebar_is_shipped` 测试守住"点了不许是空白"。
- **走查抓到的两个上游坑（都已修，判据写进 `crates/desktop/src/lib.rs`）**：
  ① 普通 `Dialog` 不画按钮 —— 页脚必须自己用 `DialogFooter` + `DialogClose` +
  `DialogAction` 组，缺页脚的弹窗只有右上角 ✕，**用户根本无法提交**（P0.4 的
  「新建环境」弹窗带着这个缺陷入库了两轮，本轮弹窗截图才抓到）；
  ② `Context::on_next_frame` 的闭包第一个参数就是 `&mut 本实体`，在里面再
  `update` 同一个实体直接 panic。
- **走查抓到的三个功能缺陷**：① 规则集缺「查看原文」（web 是指向 `/api/rules/<name>`
  的链接，桌面没浏览器 → 改成只读弹窗走 `rules_read`）；② 代理缺「编辑」入口；
  ③ `proxy_put` 请求体形状错 —— `port` 发成了字符串、`user`/`password` 永远发空串，
  而服务端要求 `port` 是数字、凭据 all-or-nothing，等于新建一个不配鉴权的代理都会失败。
  现在由 `build_proxy_body` 组装（3 条单测盯住），编辑弹窗在 `has_auth` 时把按钮
  文案改成「保存（将清除鉴权）」，把留空的后果写在能看见的地方。
- 密码输入改用 `InputState::masked(true)`：桌面没有浏览器的 `type=password`，
  打码要自己声明（顺带把值挡在剪贴板选择之外）。
- 两屏各补搜索/计数（规则屏「N 个规则集」，过滤后给「x / y · 无匹配项」），
  与环境屏同一写法。
- **本机验证边界（如实记）**：输入注入仍被 OS 拒，弹窗与二屏靠临时探针
  （`ENVBOARD_PROBE_SCREEN` / `ENVBOARD_PROBE_DIALOG`）截图，**探针不入库**；
  最终二进制只跑默认路径并复测。工作区测试：仅 `crates/engine/tests/backend.rs`
  （2 项）与 `data_plane.rs`（11 项）因本机拒绝访问当前用户证书存储失败，HEAD 同形。

### Added（桌面形态列表交互：搜索、过滤、行选中、启停/重启/删除、新建）

- 工具条：搜索 `Input`（`clean_on_escape`，覆盖名称/说明/规则集/上游）+ 四枚状态
  chips（`Button.ghost().selected()`，计数与表格同源）+ 每屏唯一的主操作放最右。
- 行内操作列：启停（按 `desired` 翻转）/ 重启 / 删除。按钮自己拿环境名，不依赖
  "当前选中行"；悬停色用 `ButtonCustomVariant` 的 secondary 而不是 `.ghost()`
  —— ghost 悬停色与表格行悬停色同色，指针移上去看起来毫无反应。
- 在途标记：写操作发出到回执落地之间该行按钮转圈并禁用（防连点），"哪一行在等回执"
  只有一处真相（记在 delegate 里）。
- 删除走 `open_alert_dialog().confirm()` + Danger 变体；新建走表单弹窗，
  名字形状先拦一道、裁决仍在服务端。

### Added（桌面形态接上数据：tokio ↔ GPUI 桥与环境列表出数）

- `crates/desktop/src/host.rs`：`Host`（GPUI Global，持 `Arc<AdminService>` +
  tokio `Handle` + 宿主信息）与 `hop()` —— 全仓唯一一条跨界通道。用例在 tokio 上跑，
  结果经 `oneshot` 送回主线程（`oneshot` 是传输原语不是 reactor 原语，可在 GPUI 的
  主线程执行器上 poll）；返回的 `Task` 存进视图字段，drop 即取消。
- `crates/desktop/src/views/environments.rs`：环境列表整体交给现成的
  `TableState<EnvTable>` + `DataTable`（虚拟滚动、可排序表头、可拖拽列宽、空态与
  骨架加载态都是组件自带的），本文件没有一行手搓表格度量。1s 快照轮询器跑在 tokio 上，
  经 `mpsc(1)` + `try_send` **丢帧**（快照全量，积压无意义；与 web 的 `/api/events`
  同一语义），只在数据真变时 `refresh` + `notify`。
- `crates/desktop/src/display.rs`：展示层派生（健康中文/色调、"需处理"判定、监听地址、
  字节短表示）逐字移植自 `frontend/src/data.ts`，**纯函数 + 6 条单测**；视图不许自算。
- `main` 现在持有 tokio 运行时并在退出时按超时收尾（日志照看循环是无界的，
  靠默认 Drop 会一直等）；宿主编排 `startup()` 在运行时里 spawn，**不等它完成**，
  界面先出 Loading 态。
- 端到端实测（本机，release）：用 web 宿主往临时状态目录写三个环境后停掉它，
  再开桌面形态 —— 表格画出三行（`beta` / `yunshu-beta` / `kad-offline`，
  健康列「已停止」、规则「未绑定」、上游「直连」），状态栏显示状态目录与端口区间，
  `PrintWindow` 抓取 150 色非黑帧。首轮截图暴露「说明」列被裁出视口，
  按最小窗口内容宽（1192 逻辑 − 侧栏）重排固定列宽后复测已完整可见。
- 桌面产物体积随功能长到 25,033,216 B（骨架那次是 23,930,880 B）；
  构建计时是单次测量，不作基线（同配置抖动见工作区记忆）。

### Added（桌面形态骨架 `envboard-desktop`：GPUI 入库）

- 新 crate `crates/desktop`（lib + bin `envboard-desktop`）：现成件搭起的窗口骨架 ——
  `TitleBar`（含深色开关）+ `Sidebar`（环境/规则/代理）+ 内容区 + `StatusBar`
  （状态目录、端口区间、当前屏）。未落地的屏显示"属于 P0.8"而不是假数据。
- 依赖纪律破例并写死：`gpui-kit = "0.8"`（内含 gpui-pre 0.3.9 + gpui-component 0.8.0），
  **只允许出现在 `crates/desktop`** —— `deps` 门禁新增第 11 条在 manifest 产物依赖面上判。
  理由、候选对比与实测代价见 docs/adr-001-gpui-desktop-form.md。
- dev profile 逐包点名四个框架包 `opt-level = 3`（GPUI 未优化不可用），**不**用
  `package."*"` 通配、**不**动 `[profile.dev]`：本仓日常是跑引擎与管理器的测试。
- `mise run desktop` / `mise run verify-desktop` 两个任务入口。
- 实测（P0.2）：`Cargo.lock` 157 → 848 包，gpui 血脉单条，tokio/ring/rustls 版本
  **零漂移**；`envboard-desktop.exe` release 23,930,880 B；框架包首次 release 构建 109 s；
  web 宿主 `envboard.exe` 与 HEAD 同 dist 对照 +1.03%（thin LTO 自身有约 ±40 KB 抖动，
  故只支撑量级结论）。截图验收：`PrintWindow(PW_RENDERFULLCONTENT)` 110 色非黑帧，
  中文与三个侧栏图标都真的画出来了。
- **未验证项（如实记）**：本机 agent 会话被 OS 拒了输入注入
  （`SetCursorPos 拒绝访问 0x80070005`），而框架的无头 UI 测试又依赖
  `gpui-kit/test-support` —— 该 feature 在 0.8.0 上**不可解析**
  （`cocoa =0.26.0` vs `^0.26.0`，强钉后接着撞 `unicode-properties`），
  所以标题栏内控件的点击（坑 #1 的 `stop_propagation`）目前没有自动证据。
  接替分工：纯逻辑单测 + 实体级测试 + 人工点击与 `PrintWindow` 截图，
  详见 docs/adr-001-gpui-desktop-form.md 的「本机验证边界」。

### Changed（引擎装配收进唯一组合根 `envboard-host`）

- 新增 `crates/host`（lib）：原来写在 `crates/server/src/main.rs` 的引擎装配、共享 CA
  物化、管理器构造（含单实例锁）与宿主编排（reconcile + 日志照看）整体搬进来，
  web 宿主与将来的桌面形态共用**同一条构造路径**。决策与迁移表见
  docs/adr-002-composition-root-in-envboard-host.md。
- 出口收窄：`prepare(HostOptions)` / `prepare_with(HostOptions, Arc<dyn ProxyEngine>)`
  （后者是测试注入口，真管理器 + 假引擎，走生产那条路径）/ `Prepared::startup()`
  返回 `ReconcileReport`（报告是数据，打印由调用方决定）。内部持有的
  `Arc<Manager>` 是 `pub(crate)` —— 界面向外界只拿到 `Arc<AdminService>`。
- CA 重新物化的两条警告从装配中途的 `eprintln!` 提升为 `Prepared::warnings`：
  宿主照旧打到 stderr（文本逐字未改），界面形态将来可以渲染成提示。
- 门禁 `deps` 第 8 条改成三条判据：装配符号必须真的出现在 `crates/host/src/**`
  （搬回界面就红）、盲区表（server / web / admin / desktop）出现即红、
  新 crate 消费门面却未登记进盲区表也红。原来的"只许 `src/main.rs` 一个文件"
  是脆约束 —— 第二个二进制出现时它会静默失效。
- `envboard-server` 依赖边收窄为 host / web / engine（词汇），不再直依 admin 与
  manager；`serde_json` 降为 dev 依赖（只有 `live_workbench.rs` 解析帧）。
- HTTP / SSE 行为**零变化**：`crates/server/tests/` 与 `tests/contract-tests/` 逐条钉住，
  `Cargo.lock` 只新增 `envboard-host` 一个包（无版本漂移）。

### Fixed（HAR 导入两处缺陷：大文件被请求体上限拦截 + 会话列表字段错位）

- **大 HAR 文件导入失败**：axum 默认 2 MiB 请求体上限把大于该值的导入请求
  拦成 413 纯文本（前端只显示笼统的「需为合法 HAR 1.2 JSON」）。web router
  显式放开到 512 MiB（导入会话库 256 MiB 预算按 entries 体积估算，裸 HAR
  JSON 会更大）；前端导入失败提示按原因区分（非法 JSON / 超限 / 服务端错误）。
- **导入会话显示 `#0 0 条`**：`HarPanel` 按不存在的 `{session:{id},captured}`
  形状读取 `GET /api/har` 的列表项（真实形状 `{id,name,imported_at,entries}`），
  永远兜底为 0。改按真实形状渲染（会话 id / 文件名 / 条数），点击会话行
  `GET /api/har/:id` 拉取记录窗口并以抓包同款表格展示；导入成功后自动展开
  新会话。前端类型新增 `HarSession` / `HarSessionView`。

### Changed（工程整理：验收材料出库 + CI 编排跨平台化 + mise 工具链锁定）

- **实机验收转录出库**：`frontend/verify/round{1,2,3}-conclusion.md` 是 bsk 验收
  过程记录（工作材料），从仓库移除、归档到本机工作材料目录；`spec/design.md`
  附录 A 的截图存放指引同步改为「截图不入库」的表述（原先指向的
  `verify/shots/` 本就不在仓库里）。
- **CI 编排 bash → Python**：`ci/verify.sh` 与 `frontend/verify-frontend.sh`
  合并为 `scripts/verify.py`（仅标准库，Linux/macOS/Windows 原生通用），分层
  语义与子命令不变（`all`/`policy`/`rust`/`contract`/`artifact`/`frontend`/`live`，
  默认 all 以 frontend 层最前置）；frontend 层的 pnpm 探测、SKIP 与响亮失败
  指引逻辑原样收编。`ci/` 目录退场。
- **工具链门禁重划（toolchain 门禁）**：Python 从「全仓禁止」改为「圈禁在
  `scripts/` 编排位」；可执行面（`scripts/`、`mise.toml`、`*.service`）里前端
  工具链（pnpm/node）只许同行点名 frontend 边界；npm/yarn/bun/deno/vite 与
  Python 工具（pip/pytest 等）仍整词禁用；迁移白名单机制随之清空退场。
- **文本自包含门禁（doc_scope）**：路径存在性前缀清单随 `ci/` 退场移除 `ci/`。

### Added（工程整理）

- **根目录 `mise.toml`**：工具版本锁定（python 3.14.7 / node 22.20.0 /
  pnpm 10.18.2，rust 仍由 `rust-toolchain.toml` 钉死）+ 跨平台任务入口
  （`mise run dev|build|typecheck|test|verify|package`，recipe 单命令不用
  `&&`，PowerShell 原生可执行）。

### Removed（工程整理）

- `frontend/verify/round{1,2,3}-conclusion.md`（验收转录，出库归档到本机
  工作材料目录）。
- `ci/verify.sh`、`frontend/verify-frontend.sh`（被 `scripts/verify.py` 取代）。

### Changed（v4 架构升级：前端工程化 + Admin API 门面 + manager 按域拆分）

- **前端工程化（破坏性：资产形态）**：新增 `frontend/`（Vite 7 + React 19 +
  HeroUI v3 + Tailwind v4，pnpm 管理），工作台 UI 转正自验证 demo
  （`examples/envboard-workbench-heroui`）并接真实 Admin API；补建「活动」视图
  （`/api/history` 审计事件时间线）与主题切换（浅色/深色/跟随系统，localStorage 持久化）。
  旧原生三件套 `crates/web/assets/` 退场；`web` 改用 `include_dir!` 内嵌
  `frontend/dist`（hashed 资产走 `/assets/*` + immutable 缓存，token 豁免面从
  两个固定路径改为 `/assets/` 前缀）。URL 与响应形状不变，前端零回归。
- **构建顺序纪律：先 `pnpm build` 后 `cargo build`**。`frontend/dist/` 是构建
  产物、不入库（与 `target/` 同类）；`crates/web/build.rs` 缺 dist 时编译期响亮
  失败；`ci/verify.sh` 的 frontend 层（typecheck + build）成为默认 all 的最前置，
  无 pnpm 但 dist 在位时以既有产物通过。
- **工具链纪律重划**：Node/TS/Vite/pnpm 从「全仓禁止」改为「圈禁在 `frontend/` 边界内」；
  可执行面仍禁前端命令（cargo 永不驱动前端工具链），衔接发生在 verify 编排层。
- **新增 `envboard-admin`（Admin 管理 API 门面）**：控制面用例的传输无关门面 ——
  类型化入口校验（`EnvCreateReq` / `EnvPatchReq` / `ProxyPutReq` / `RulesImportReq` /
  `DebugStartReq`，`deny_unknown_fields`）+ 统一错误信封；`web` 依赖从 `manager`
  改为 `admin`（web 降级为纯传输绑定），契约成文为 `spec/admin-api.md`。
  `envboard-protocol` 同步补请求 DTO（patch 的「未提及/显式 null/给值」三态保真）。
- **`manager.rs` 按域拆分**（行为不变，48 个公开方法签名逐字未动）：单一 `Manager`
  类型的 impl 拆入 lifecycle / rules_store / proxies / reconcile_health / ledger /
  observability / debug_capture / projection 八个模块文件；跨域共享私有项留
  manager.rs 改 `pub(crate)`。
- **UI 契约升 v2.0**：机检档从「三件套资产扫描」改为「frontend 源码扫描」
  （UI-1 色值零容忍 / UI-2 禁内联样式 / UI-3 禁手写 dark: / UI-4 禁裸 px 任意值 /
  UI-5 交互形态 / UI-6 锚点对账）；新增 `spec/design.md` v1.0 设计规范
  （12 组设计语言 + bsk 走查检查表，转正自 demo 的 REVIEW_NOTES P1–P14）。
- 文档修正：README 版本号 0.2.0→0.3.0、crate 树补 events/admin/frontend、
  架构图与工具链纪律改写。

### Changed（UI crate 拆分：protocol + web，前后端协议成文）

- **新增 `envboard-protocol`（叶子 crate）**：UI 边界词汇的唯一定义处 —— 视图 DTO
  （`EnvView` / `ProxyView` / `DebugView` / `ReconcileReport`，自 manager 收拢）与
  抓包词汇（`CaptureView` / `CaptureDelta` / `SessionInfo`，自 engine 收拢）迁移至此，
  两侧以原名 re-export 保兼容；新增推送流帧类型（`TrajectoryWindow` /
  `SnapshotFrame` / `DebugSnapshot` / `DebugEvents` / `ErrorFrame`）与
  `Cursor`（Byte / Request / Generation）。纯 serde、零传输依赖 —— 未来桌面形态
  （`envboard-desktop`）与 web 平级共用。
- **新增 `envboard-web`（web 形态 crate）**：`crates/server` 的资产（index.html /
  app.css / app.js，git mv 保历史）、axum 路由/handler/安全中间件、`WebConfig` 与
  CSP、qrcode 端点整体平移；只做传输绑定，整 crate 不认识引擎装配（源文件面门禁
  随拆分扩展到本 crate）。
- **`envboard-server` 瘦身为宿主**：只剩 `[[bin]] envboard` 的组合根（引擎装配 /
  SharedCa / flock）与 serve 编排（reconcile、日志照看循环从 web 层归还宿主）；
  lib 面随拆分取消。
- **推送流帧统一**（行为变更，契约见新增的 spec/protocol.md）：三条 SSE 流的每帧
  都带显式 `cursor`，帧名统一 `snapshot / baseline / events / error`。轨迹流
  `baseline` 帧补齐游标（= 窗口末端字节偏移），REST `GET …/trajectory` 响应体改
  为同形 `{cursor, events}`（修复前端此前拿事件 seq 硬凑字节偏移的错位）；快照流
  帧新增控制面账本代次作 advisory 游标；调试实时流无会话首帧补 `cursor: 0`。
  前端 baseline 直读游标续传、快照帧记录代次。
- **门禁**：`policy-tests` 依赖边表登记 protocol / web 并更新 server 边；
  `ui_style` 门禁资产路径改指 `crates/web/assets/`；artifact 校验同路径。

### Added（上游代理 / 二级代理：一等实体 + 按环境热绑定）

- **代理账本**：`state.json` 增 `proxies[]`（`name/host/port/user/password`），manager 为
  唯一真相。凭据与环境 `proxy_user`/`proxy_password` 同规（同生共死、不含 `:`/空白、
  ≤64/≤128 字符），明文只落 0600 状态文件，API / 视图 / 日志**永不回显**，只暴露
  `has_auth` 布尔；同名保存即整体替换并热应用引用环境。
- **环境第 8 字段 `upstream`**：按名引用代理账本（`null` = 直连），与 `rules` 绑定同构、
  **热字段**（运行中 PATCH 即生效，上游连接每请求新建无残留）；绑定不存在的名字
  `invalid_config`，删除被引用的代理 `conflict` 并点名全部引用方，状态文件手改出的
  悬空引用加载即拒启 —— 三条同证「禁止静默降级」。
- **引擎链式建连**：兑现 `ConnectTarget.chained_proxy` 扩展位（基础层从配置播种、
  插件链保留修订权；`ProxySpec` 增出向 Basic 鉴权位）。TLS 目标：TCP 连代理 →
  `CONNECT`（带 `Proxy-Authorization`）→ 非 2xx 返 502 带代理状态码 → 隧道内按既有
  `TlsPolicy` 做 rustls 握手（**内层 TLS 与直连一致，`insecure_hosts` 语义不变**；
  CONNECT 目标 = 规则改写后的终址）。明文 http 经代理按 absolute-URI 转发（Host 头
  透传）。`config_hash` 覆盖代理 host/port/user 与密码摘要。`auth.rs` 补
  `base64_encode`（RFC 4648 向量互验），不引新依赖。
- **API 面**：`GET/POST /api/proxies`、`GET/DELETE /api/proxies/:name`（清单带
  `references[]` 供删除确认）。
- **工作台第五视图「上游代理」**：代理清单（地址 / 鉴权位 / 引用环境）+ 保存表单 +
  破坏性确认删除；环境配置表单与创建弹窗增「上游代理」下拉（热字段不进停机锁）；
  概览增「上游代理」行（直连显式写出）。
- **契约同步**：环境模型 7→8 字段（capabilities.md「上游代理（chained proxy）」新节 +
  热/停机矩阵补行）；errors.md 补 invalid_config / conflict 触发场景；契约 fixture
  66 → 79（新增 `proxy.validate` 能力与环境/合并 upstream 用例）；README 能力表、
  PATCH 示例、热矩阵与 HTTP 端点表同步。
- **明确不做**：SOCKS5、`https://` 代理（与代理本身的 TLS）、PAC / 透明代理、
  按域名分流、代理链。

### Fixed（明文 http 经带鉴权二级代理不吃 407）

- 二级代理的出向 Basic 凭据此前只在 CONNECT（TLS 目标）那半注入：明文 http 走
  absolute-URI 转发时凭据缺失，要求认证的上游代理对每个明文请求回 407 且被逐跳头
  过滤掉 `Proxy-Authenticate`，客户端只见一个没有挑战头的 407（实测
  yunshu-beta → 127.0.0.1:11088 即此病；https 不受影响，故 live 用例漏网）。
  现在转发请求头同样携带 `Proxy-Authorization: Basic`，与 CONNECT 两半对齐；
  客户端自己的 `proxy-authorization` 维持不透传。契约 `spec/capabilities.md`
  「建连语义」明文条款同步钉死；data_plane 新增带认证明文链路用例（负向验证：
  撤修复即红），live 16c 的二级代理改为要求鉴权、真正覆盖出向凭据。

### Added（UI 设计规范契约化：入库契约 + policy 门禁）

- **新契约 `spec/ui.md`**：把工作台三资产（index.html / app.css / app.js）的
  UI 纪律固化为入库契约，分机检档（UI-1…UI-8，违反即 `ci/verify.sh` 红）与评审档
  （等宽字体 / 异常三通道 / 语义色专用 / 破坏性操作模态二次确认 / `--muted-2`
  表面限制 / WCAG AA 底线等门禁不可达项）。令牌值唯一权威仍是 app.css 第 ① 区。
- **新 policy 门禁 `tests/policy-tests/tests/ui_style.rs`**：机检色值出没域
  （只许在 `:root` / `[data-theme]` 令牌区）、字号 / 圆角 / 阴影 / 间距的令牌化
  （间距例外登记 1px / 2px 发丝档）、纯黑禁令、令牌存在性、禁原生弹窗与内联
  script / 内联事件属性，以及契约锚点 `UI-n` 与门禁规则的双向对账（缺锚点或
  多规则号都判红，契约与门禁必须同提交同步）。三个资产现状已全量合规，门禁
  零豁免上线；README「运维」节与契约互指。

## [0.3.0] - 2026-09-18

### Added（调试实时流：抓包记录自动上屏）

- **`GET /api/debug/stream`（SSE）**：调试页不再靠进页时一次性拉取 ——
  连接即发 `snapshot` 帧（整幅会话视图），此后 500ms 一轮询内存缓冲、有增量发
  `events` 帧 `{records, cursor, captured, dropped}`；会话换代或目标消失重发
  `snapshot`。游标（request_id）由流自己维护：淘汰只移除游标之前的记录，增量
  无缺口，断线重连自愈。前端在「调试」视图维持一条连接（离开即断），记录追加
  就地渲染、原本贴底才自动跟随；「开启 / 停止 / 清空」按钮即时反馈。
  契约见 `spec/events.md`「调试实时流」。

### Fixed（续）

- **`GET .../captures/:request_id` 全缓冲查找**：曾经的实现是「取尾部 1 条再
  比对」—— 只有最新一条查得到，其余一律 404。改走引擎缓冲的按 id 查找。

### Fixed（调试页恒空的两个根因：capture 不热应用 + 轨迹窗口被账本读者整份拒绝）

- **`capture` 真正热应用**：`capture` 是契约声明的热字段（调试页「开启 / 切换」
  驱动的就是它），但更新路径的热更触发集合漏了它 —— 开关只落盘，引擎手里的
  快照永远 `capture=false`，抓包计数恒 0，页面忠实地渲染这个 0。现在
  `capture` 变更与 `insecure_hosts` 同路：一次同步热应用，并计入审计事件的
  变更字段清单。
- **轨迹按窗口契约读**：轨迹文件从写下第一行起就是无头行、`seq` 每个实例会话
  从 1 重启的**窗口化日志**，而两个读者按控制面账本契约（`parse_log`：要求
  版本头 + seq 连续）读它 —— `GET .../trajectory` 恒 `BadHeader`，SSE 基线被
  吞错置空、流在第一次增量处死掉（`SeqGap`）。新增 `parse_window`（无头、
  seq 不校验、撕裂行逐行跳过、未知类型仍 fail-closed），两个读者改用它；
  两档契约成文于 `spec/events.md`「存储格式（两档）」。

### Changed（v4：独立工具形态 + DSH 式事件轨迹）

- **移除插件抽象**：`Plugin` trait、能力注册表（`CAPABILITIES`）、装配期拓扑
  校验、钩子执行器与 `debug-inject` 整体退场。hosts 改写下沉为引擎 connect
  路径的内部直调（`hosts::apply`，语义不变：脏规则 fail-closed 502），请求
  终局日志直投有界总线（行格式不变）。`EngineReport.bypass_counts` 删除。
- **目录重排为独立工具形态**：`core/spec/` → `spec/`；`core/rs/crates/*` →
  `crates/{engine,engine-fake,manager,server}`；契约/门禁测试 →
  `tests/{contract-tests,policy-tests}`。core-api + core + domain + rules 四
  crate 合并为 `crates/engine`（api/rules/domain 变模块边界，deps 门禁改为
  模块面判据）；`envboard-web` → `envboard-server`（bin 仍为 `envboard`）。
- **控制面审计事件**：`<state_dir>/events.jsonl`（append-only，权威仍是
  state.json）；`Manager::commit` 单一发射点（先 save 后 emit，门禁钉死）；
  `GET /api/history` + 工作台「活动」视图；事件写失败丢弃 + 计数
  （`/api/status` 的 `events_dropped`）不阻断控制面。契约见 `spec/events.md`。
- **抓包（capture）**：`capture` 环境字段（默认关、热开关）；请求/响应详情存
  实例内存会话（停止/重启即丢弃，清空走 `POST .../capture/clear`），满即淘最旧
  （`--capture-budget`，默认 256 MiB），body 超 256 KiB 标 omitted（缺失而非截断）；
  `GET .../captures` 详情/清空/导出（HAR 1.2 / JSONL）+ 工作台轨迹页点开详情与导出。
  设计参考 mitmproxy（成对落盘、raw/decoded 分离、缺失而非截断、View.clear）。
- **调试页 + 会话双轨**：抓包独立为「调试」视图；调试会话是**工作区级单例**，
  切换环境 = 原环境抓包停止并清空 + 新环境开启（`POST /api/debug`）；支持导入
  HAR 会话（`POST /api/har/import`，多个并存只读，最多 8 个 / 256 MiB，超限拒收）。
- **数据面请求轨迹**：`trajectories/<env>.jsonl`（request/start→upstream→
  body→response/head→end，`request_id` 贯穿；正文与头部不入轨迹）；独立有界
  总线 + `EngineReport.trajectory_drops`；`GET /api/environments/:name/trajectory`
  + SSE 实时流（baseline + 增量 + cursor 续传）+ 工作台「轨迹」页签。

## [0.2.0] - 2026-09-17

### Fixed + Changed（第三轮走查后：配置页签回显、动作行收敛、设置页结构件补齐 + 按设计规范全量 UI 走查）

- **配置页签 = 常驻编辑器**：选中环境即回显全部当前值（幂等重填，SSE 快照不冲掉
  输入中的改动）；「保存」按钮监听表单改动，与基线一致时禁用；运行中照旧锁停机字段
  并亮「运行中」提示徽章。
- **动作行收敛为三键**（停止/启动 · 重启 · 删除）：右上角「日志」快捷按钮删除
  （日志已是页签）、代理命令复制按钮删除、「更多操作」平铺行退场；
  重分配端口移入 port_conflict 的原因条。
- **设置页结构件补齐**：`.ca-card / .ca-head / .ca-title / .spanel / .notice.ok`
  此前 HTML 用了类名而 CSS 从未定义，证书面板塌成无结构裸块（用户截图所示排版错误）；
  页签样式与详情页 `.tab` 共用；标题栏只留「设置」。
- **按「envboard 设计规范 · v2.0 Light」全量走查落地**：
  破坏性操作（删除环境 / 删除规则 / 重分配端口）迁入独立确认模态，写明后果与
  不可逆性、确认按钮显式第二次点击（规范 7.2.3）；模态对齐规范 6.6（520 宽、
  15px/600 标题、32×32 关闭、字段控件 36px + --line-2 描边、必填星号 --bad、
  遮罩冷灰 .45、窄屏竖排按钮确认在上）；模态控制器补焦点陷阱 / ESC 策略
  （submitting 忽略 ESC、取消与关闭保持可点）/ 遮罩点击策略 / 焦点归还；
  创建弹窗补失焦校验 + 提交全量校验 + submitting 态（「创建中…」+ 字段只读），
  校验文案按规范 6.6.5 落地（原因 + 建议动作，409 重名映射回名称栏）；
  toast 移到顶栏下方右侧、最小宽 240、2.5s 淡出（错误 6s，偏差已记录规范 7.3）；
  图标一律 currentColor（删除五处独立配色）；区块标题加 3px 主色竖条；
  日志搜索命中高亮转 25% 底；禁用按钮 pointer-events 关闭；
  设置版本 chip 与「运行中」徽章的语义色归位（装饰不再占用 --accent）。
- **退役**：就地二次确认（.confirm 展开块）、state.confirm / state.editing、
  copyText / flashCopied、.detail-more 样式、--logbar-h 令牌。
- **规范文档同步**：7.3 四项待对齐全部裁决关闭（事件类型过滤、输入 32/36 分层、
  错误 toast 时长、CmdCard 无复制按钮）；附录 A 与 app.css :root 逐字同步。

### Changed（第二轮走查后：UI v3 版式 + 设置·证书 + 全库测试数据脱敏）

- **工作台版式按「envboard 工作台 UI 设计稿 · Light」重排**：视图导航从顶栏移进
  侧栏（环境 / 规则库 / 跨环境对比 / 设置四项）；日志从常驻底栏收进环境详情的
  第 4 个页签（过滤 chip 与搜索/导出/跟随工具条随迁）；新建环境改为居中弹窗
  （只收名称 / 描述 / 端口 / 规则集，对外绑定与代理鉴权留在编辑表单）；规则库
  行内元信息带落盘文件名（`<名字>.hosts`），侧栏在规则库视图切换为「规则集」
  列表并常显「N 环境引用」徽标；跨环境对比行按设计稿改为「环境 → IP + 命中
  来源 chip / 无规则命中」。
- **新增设置视图**：证书页（根证书状态、下载、扫码二维码、证书信息、常见问题）、
  通用页（运行信息只读回显）、关于页（版本信息）。配套端点 `GET /api/ca`、
  `GET /api/ca.pem`、`GET /api/ca/qrcode.svg`（照常受 Host/token/CSRF 门禁管束，
  401 roll-call 断言已扩到 21 端点）。`/api/status` 增加 `version`（工作台自身
  版本）。证书结构读取按仓内纪律走 `envboard-core` 自有 der 模块（不引 x509 库）。
- **依赖**：新增 `qrcode`（纯 Rust，default-features 关闭、只留 svg 渲染）——
  Cargo.lock 增量仅此一个包，离线可构建不受影响。
- **本版未做**：上传根证书 / 重置根证书。两者需要管理器支持 CA 轮换（引擎在跑
  实例的热切换与全量重启编排），超出本次 UI 升级范围，后续单独立项。
- **测试数据脱敏**：契约 fixtures、单测与界面示例文案里的个人测试域名与内网 IP
  统一替换为 RFC 2606 保留域（`example.com` 家族）与通用私网假值；仓库文本零残留。


v2 把 envboard 从「一个 mitmproxy 进程内的运行时开关」改成**多环境管理器**
（一个环境 = 一个独立代理实例 + 一个独立端口，附 Rust 工作台），落地分支
`feat/rust-multi-env-manager`（从 `v0.1.0` 切出）。以下是**已经落地的契约层改动**——
它们先于实现改动，因为契约是两份实现的裁定依据。

### Changed（走查后第二轮：鉴权换默认 + CLI 退役，两条 BREAKING）

- **鉴权按绑定地址定档**：回环监听（127.0.0.1 / ::1）**默认免鉴权** —— 本机即本机
  用户，token 挡不住同机进程、只添摩擦；`--token <T>` 在任何监听上显式启用，裸给
  `--token`（不带值）自动生成 128 bit 随机值并在启动日志打印可点链接；**非回环监听
  不给 token 直接拒绝启动**（`invalid_config`，字段 `web.token`）。`--without-token`
  退役（它的语义成了默认档，选项失去存在理由）。`Host` 校验与 CSRF 头与档位无关、
  永久在场；多用户共享主机建议回环也上 `--token`（README「工作台鉴权与对外暴露」
  有取舍说明）。token 不再落盘：`runtime/api.json` 随 CLI 一起退场。
- **CLI 子命令面整体退役**：`envboard-cli` crate（`status` / `run` / `env` / `rules` /
  `compare` 子命令、直连模式、瘦客户端 `api_client`）删除，`--core`（含 `fake`）、
  `--once`、`--json` 一并退役 —— `envboard` 二进制唯一行为 = 启动工作台；`[[bin]]`
  并入 `envboard-web`（组合根与产品同 crate，一个 crate = 一个发布物）。管理动作
  统一走工作台 UI 或本地 HTTP API（端点不变，README 有 curl 示例）：判定只在服务端
  做一次，界面与脚本是两个翻译面，这正是砍掉第三张面孔后仍成立的理由。
- **测试面随迁**：smoke 重写，钉住"命令面不得再长出子命令"、鉴权四档、非回环
  拒启（负向守卫）、状态单写者；live 工作台铺设从 `cli()` 改 HTTP（`seed_rule` /
  `seed_env`，与脚本用户同一条路）；第 11 组按新档位重写（回环默认免鉴权 200、
  裸 `--token` 自动生成、显式 token 双通道 200 与端面逐个 401 清点、非回环拒启）；
  `thin_client` 层随功能退场。

- **deps 门禁随形态调整（有理由的放宽，负向验证过）**：crate 登记表移除
  `envboard-cli`；`envboard-web → envboard-core` 边放行，但**只给 `src/main.rs` 组合根用**
  —— 工作台 lib 面（api / config / lib.rs）新增源文件面判据：出现 `envboard_core::`
  引用即红（注入探针验证过）。"换引擎实现不动工作台"这条不变量原样在场，只是裁决
  粒度从 manifest 细化到了文件。

### Fixed（实机浏览器走查揪出的三个缺口 + 一个测试盲区）

- **空列表里「新建环境」是死的**：renderDetail 在没有选中环境时把含表单的详情区
  整体隐藏 —— 第一个环境永远建不出来（v2 遗留，live 层从不走 UI 所以从未暴露）。
  前端引入显式 creating 态：新建时详情区盖过空态、动作行收起、取消/创建后回位；
  renderDetailActions 补回 is-hidden 复位（曾经隐藏过的动作行必须能回来）。
- **硬刷新必 401**：前端捕获 token 后立刻抹掉地址栏 query，而 document 请求本身
  需要 token —— F5 得到 401 JSON 页。改为捕获后保留在 URL（可用性优先于观感，
  凭据本就在启动横幅里，换 token 随时可重启）。
- **运行标记行随子进程模型丢失**：契约承诺"每次启动写一行运行标记"，v3 实现漏了
  ——launch 现在经 log writer 写运行标记行（追加不截断；manager_lifecycle 新增
  断言：启动即有文件、重启成两段）。
- **query token 端到端是测试盲区**：live 的 api() helper 一直用 header 通道，
  "URL 携带 token"这条浏览器唯一路径从未被断言覆盖。补 api_query helper 并把
  11a 扩成 header 与 query 双通道断言（实机验证 query 200 ✓）。
- **新增内嵌资产完整性门禁**（本轮修复过程自己的事故换来的）：app.js 曾被
  "读截断 + 写回"弄丢尾部并随 release 部署出去——JS 没有任何编译期检查，默认层
  全绿而页面卡死。artifact 门禁现在钉住 app.js 最小行数与关键符号（envboardReady/
  renderDetail/refreshAll/addEventListener）、index.html 关键 id；已负向验证
  （截断必红、恢复转绿）。
### Removed（v3 重构 M-P6：收尾退场，v2 面清零）

- **全部 Python 产品代码退场**：`adapters/mitmproxy/`（注入器）、
  `envboard-core-mitmproxy` crate（子进程监督 + 内嵌 + 状态文件读侧）、
  FakeCore/StatusMode（v2 替身）、dual_impl 对拍与 `--render` 跨语言入口文档
  ——一次删净。工具链收敛门禁的适配器例外整套翻正：`.py`/`__pycache__`/适配器
  脚本调用现在是**无条件违规**（PENDING 白名单机制保留，表已空 = 收敛完成）；
  artifact 门禁把"注入器自包含"检查换成 v3 反向断言（adapters/ 不得存在 +
  引擎线程名标记在场），并做了负向验证（塞入 .py 必红）。
- ProxyCore-v2 抽象面删除（InstanceSpec/InstanceHandle/InstanceHealth/StatusReport/
  ProcessIdentity/redact_cmdline/config.json 与软链常量）；core-api 的 proxy 模块
  只剩 v3 共享词汇：Listen、CoreInfo、能力表（六项：listen/dynamic_certs/
  rewrite_upstream/per_domain_insecure/shared_ca/http1_only，全部在线声明）。
- 状态面剪枝：PersistedState 不再携带 records/config_seals；ManagerConfig 删
  status_ttl_secs/reload_interval_secs/annotate 与 confdir config.yaml 防线；
  cli 删 `--reload-interval`；reconcile 规划保留**占用探测**（MarkConflict 判据），
  启动路径以绑定为唯一真相。**新增升级契约测试**：v2 的 state.json（含旧键）
  直接加载成立，再保存时旧键整体消失、环境数据原样搬过来。
- 契约文档收口：capabilities.md「实例日志」按 v3 总线形态重写（有界 1024 行 +
  丢弃计数 + copytruncate + 有界尾读，v2 的 64 KiB 管道实测教训作为约束来历保留）。
- README 全量重写为 v3（架构/能力矩阵/已知限制/从 v2 升级/验证分层）；
  systemd 单元同步 v3（去 `--core-bin`/mitmdump/PDEATHSIG 叙述；单进程语义：
  停服务即停一切、无孤儿代理）。
- 版本 0.1.0 → **0.2.0**（v2 多环境形态与 v3 纯 Rust 引擎随本版一并发布）。

### Changed（v3 重构 M-P5 第三步：live 层迁移——一条不减）

- **live/workbench 28 项全绿**：v2 的 24 条断言全部保留（机制触点按映射换），
  新增 4 条：
  - 12b **凭据 argv 审计**：全系统 /proc cmdline 扫描不得出现明文密码（v2 靠
    脱敏契约兜底，v3 结构性成立）；
  - 14b **热生效时延**：PATCH 返回后的第一次请求即已生效（v2 要等 1s 轮询）；
  - 15 **既有 CA 零感知兼容**：confdir 预放 mitmproxy 形状 CA → 引擎原样加载
    （逐字节不变）且真 curl 仅凭该 CA 校验 MITM 链成功；
  - 端点清单扩到 18 个（含 /api/_fault）且全部无 token 401 的清点面不变。
- 组 9（实例崩溃）机制替换：SIGKILL 真子进程 → **注入 failed**（ProxyEngine 的
  故障注入旋钮，真引擎与 fake 同构实现，经 web 的 /api/_fault 转发）；"僵尸
  回收"判据 → "监听端口真的释放"（原端口可再 bind）。组 8 的"写满 64 KiB 管道"
  字节判据 → "全部成功 + 日志逐条落盘"（总线介质不同，丢弃量归 log_drops 面）。
- **live/manager 3 组重写为引擎直驱**（真 openssl 现场 + 真 curl）：①预放 CA
  加载复用 + 回执如实 + curl --cacert 验链；②注入 failed 可见、端口释放、重拉
  回 running；③insecure 名单外 502 → 热 apply 第一次请求即 200 且未点名域名
  仍 502（放宽不传染）。v2 的 options_echo 断言随"静默忽略选项"介质的消失删除。
- live 宿主从 mitmdump/openssl/curl 三件减到 openssl/curl 两件；verify.sh 的
  live/manager 指向 envboard-core。
- 产品侧顺手修一处可用性缺陷：自产 502 的响应体现在回声状态行
  （"502 Bad Gateway: <原因>"），与 v2 失败页同款可辨识。

### Changed（v3 重构 M-P5 第二步：manager 行为测试按 v3 语义重实现）

- manager_lifecycle.rs 以 v3 形态重写（28 条全绿），暂存件删除；v2 的 29 条
  逐条对账，**机制退场者目的由等价断言接替**：
  - stale/missing 状态文件 → unhealthy/failed **来自引擎内存报告**
    （inject_state 旋钮；"TCP 可连≠在跑"反转为"报告是唯一真相，期望保持
    running 等 reconcile 拉回"）；
  - 配置回显比对 / 收敛窗口 / config_error → apply 被拒三连：旧快照继续服务、
    invalid_config 标记进健康原因、修复后标记自动清除；同步生效留痕
    "hot-applied configuration"；
  - 软链断 = 不覆盖 → 账本缺条目 = rules_missing（legacy 状态场景），
    补导入即在跑实例热应用；
  - 孤儿清理 / PID 复用绝不杀 → 结构上不存在可误杀的进程：reconcile 在
    现实与期望一致时保持安静（keep / 无动作）；上一代重启 → **failed 引擎
    被 reconcile 拉回 running**；
  - reconcile 的 MarkConflict：占用探测**只保留在规划路径**（没有它，
    自动端口环境会在 reconcile 里被静默换端口——契约禁止）；启动路径仍以
    绑定为唯一真相。
- FakeEngine 注入语义补强：非 running/starting 的报告**同时释放监听**
  （与真引擎"线程死→运行时散→socket 关"同构），否则 failed-自愈路径测不出来。
- 测试脚手架换共享 RecordingLogger 的 standalone()：单一写者纪律下，
  "改状态文件再观察"的 legacy 场景必须经新管理器读取（这正是纪律的演习）。

### Changed（v3 重构 M-P5 第一步：契约改写与对拍退役）

- 实例生命周期契约改写为 v3（capabilities.md 的「实例生命周期」与「配置下发与热
  应用」两节、errors.md 健康状态表）：判定 = 标记 → desired → 引擎内存报告；
  **config_mismatch 从契约删除**（装配即生效没有中间态；被拒配置以 invalid_config
  标记 + unhealthy 表达）；僵尸/PID 复用/状态文件 TTL/收敛窗口等进程时代判据
  全部退场；"任务级崩溃隔离 + release 保持 unwind"与"视图与权威判定同源"升格
  为契约硬要求。
- domain 的 plan_reconcile 换 v3 模型：InstanceRecord.live 从"进程身份"改为
  **bool（线程存活）**；Action 去掉 Restart（上一代概念消失）；keep 成为清理遍的
  可见决策；warnings 恒空（字段保留给未来的退避/告警面）。契约 fixture 7 张 → 5 张
  （lifecycle 组按新语义重写：starts/stops/order/port-conflict/no-port-start），
  总数 68 → 66（计数断言与 README 同步）。
- 规则语法 BNF 仍是唯一仲裁（fixtures/rules 组不动）；**dual_impl 逐字节对拍退役**
  ——v3 只剩一份解析实现（envboard-rules），第二份 Python 镜像与它需要的宿主解释器
  探测一起删除；ci/verify.sh 的 adapter/dual 层与 PYTHON 旋钮随之移除（默认层 =
  policy + rust + contract + artifact）。
- 「引擎能力矩阵」改写为 v3 表（内核能力 + hosts-rules/request-log 的落点逐项对应
  代码；已知限制——仅 h1、信任库换源——随本表登记，README 收口在 M-P6）。

### Changed（v3 重构 M-P4 第二步：manager/web/cli 接线到 ProxyEngine）

- Manager 换依赖面：构造参数从 ProxyCore 九件套变成 ProxyEngine 八件套
  （**ProcessTable 参数删除**——没有外部进程就没有孤儿清理）。启动 =
  engine.start + 等待绑定定态（running / port_conflict / failed）；端口冲突
  的"新建自动重试一次、已持久化只标记"契约原样保留。健康判定统一走
  verdict（标记 → 期望 → 引擎内存报告），**视图与权威判定同一实现**；
  EnvView.health 升级为 v3 InstanceState（新增 starting 态；config_mismatch
  随同步装配消亡）。
- 进程监督层在管理面的用法全部消失：config.json 通道、固定名规则软链、
  状态文件物化与 TTL/收敛窗、runtime/agent 目录、/proc 身份、信号梯子。
  规则绑定改为**账本 rendered 直接进 EngineSpec**（账本唯一真相不变，少了
  整条文件物化-轮询链）。热更新改为对运行中实例的同步 apply：insecure_hosts、
  rules 绑定、**规则内容**（import_rules 覆盖即热应用）、description；apply
  失败留 invalid_config 标记（health_reason 可见"旧快照继续服务"），成功清标记。
  ProxyEngine::apply 改同步签名（装配即生效没有异步语义；管理面同步入口
  不再 runtime 套 runtime）。
- CLI：--core 取值 engine|fake（默认 engine），--core-bin/--core-python 随
  mitmproxy 退场删除；共享 CA 在 confdir 就绪（兼容既有 mitmproxy CA），
  重新物化时向 stderr 响亮警告"客户端需重装证书"。
- 产物门禁翻档：删除"二进制内嵌注入器"断言（include_str 链已断），改为
  断言 v3 进程内引擎簿记标记在场；注入器文件自检保留至 adapters 退场（M-P6）。
- **v2 测试暂存（断言不减，搬家不丢弃）**：manager_lifecycle.rs（29 条）与
  core-mitmproxy 的 live_manager.rs（3 条）改名为 *.pending-migration 暂存
  ——它们断言的机制（状态文件新鲜度/config 哈希回显/收敛窗/软链完整性/
  mitmdump 子进程）在 v3 已不存在。M-P5 逐条按 v3 语义重实现（引擎直驱、
  注入状态替代崩溃、epoch/hash 回执替代文件比对），清单即这两份文件本身。

### Added（v3 重构 M-P4 第一步：ProxyEngine 接缝与双实现）

- core-api 新增 v3 引擎接缝（engine 模块）：EngineSpec（监听/名单/凭据/规则文本/
  日志线出口——全部一等字段，无透传通道）、EngineHandle（**无进程身份**）、
  EngineReport（state/config_hash/epoch/last_error/bypass_counts/log_drops，
  内存读同步报告）、InstanceState（starting/running/stopped/unhealthy/
  port_conflict/failed；**config_mismatch 消亡**——同步换快照 + hash 回执让
  "生效不一致"没有存在的形态）。LineWriter 端口进 core-api::ports。
- config_hash 单一真相：EngineSpec::hashable_json 的 sha256（log 与展示字段
  不参与）。引擎内部编译哈希只做幂等，绝不作对外回执（两处哈希曾差点成为
  第二真相，测试把它钉回了同源）。
- EngineBackend（envboard-core）：ProxyEngine 真实现——簿记 map + 有界日志
  总线（logsink：try_send 即返回、满则丢弃计数；v2"213 个请求写满 64 KiB
  管道全体挂死"的教训以新形态继续成立）。start 成功 = 已登记；端口占用是
  **报告里的状态**而非启动错误（管理面"新建冲突自动重试一次"契约依赖此）。
- FakeEngine（envboard-core-fake）：同一接口的替身，真绑定端口，故障注入
  旋钮 inject_state/inject_log_drops（live"实例崩溃"类断言的迁移目标）。
  迁移期与 v2 FakeCore 并存；Manager 接线完成后 FakeCore 与 ProxyCore 一起退场。
- 验收：backend 实机 2 组（trait 全生命周期 + 真实转发 + 总线日志 + 幂等
  stop/NotFound/Conflict），fake 单测 4 条，总线非阻塞单测 1 条。

### Added（v3 重构 M-P3：插件宿主——阶段管道与能力注册表）

- 三层能力模型落地：内核能力（listen/proxy-auth/tls-policy/mitm-ca/protocol）
  保持原生快路径；内置插件 hosts-rules 与 request-log 走与扩展插件完全相同的
  接口（产品功能自举验证）。阶段模型写死：connect → request_head →
  request_body →（上游）→ response_head → response_body → log。
- 能力注册表（envboard-core/src/plugin.rs 的 CAPABILITIES，静态只读）：
  只参与装配期校验与自省，不参与请求路径查找。契约表在
  spec/capabilities.md 的「v3 插件与能力注册表」；三方一致性
  （契约表 ↔ 静态注册表 ↔ 内置实现）由新增 policy 门禁判定
  （envboard-policy-tests/tests/registry.rs，负向样本双向验证过会红）。
- 装配期校验：id 未注册 / 内核冒充插件 / 缺依赖 / 依赖环 → invalid_config
  点名双方；执行序 = 声明序，依赖约束 > 声明序（同层同依赖保持声明序）。
  明确不采纳"静默等待依赖"语义。
- 错误契约两档：connect/改写钩子 Err → fail-closed 502（正文带插件名与
  原因）；显式 bypass（request-log 即此档）→ 跳过 + 强制 WARN + 逐插件
  计数（EngineInstance::bypass_counts）；每钩子超时（connect/head 1s、
  body 5s）按 Err；on_log 在类型上就不可外溢（同步、返回 unit）。
- Flow::Early 短路语义（mock 类插件的接缝）：request_head 可整条替换响应，
  response_head 可重写上游响应；两者都不触碰上游连接。
- max_buffered_body 从编译期常量升级为快照字段（EngineConfig 可选，默认
  8 MiB）；超限 413/502 而非静默流式。
- 测试/故障注入旋钮 debug-inject（Extension 层，注册表在册——注入路径走
  真实装配校验，不是旁路后门；产品链恒为空，对齐 envboard-core-fake 先例）。
- 验收：单测 7 条（拓扑与注册表拒绝面）+ tests/plugins.rs 实机 6 条
  （fail-closed 带名、bypass+WARN+计数、钩子超时、Early 不触上游、
  connect 阶段失败、未注册 id 启动即拒）。

### Added（v3 重构 M-P2：数据面——进程内引擎实例）

- envboard-core::engine：**引擎实例 = 一个 OS 线程 + 一个 current-thread
  runtime + 一个监听端口**。状态机 starting → running / port_conflict /
  failed：绑定即真相（EADDRINUSE → port_conflict，不换端口、不试绑）；
  请求任务级崩溃隔离（tokio 逐任务捕获），引擎线程 panic → failed。
  apply_config 同步换入全量快照（ArcSwap），回执 config_hash 与 epoch；
  listen 是停机字段，热改它按 invalid_config 拒绝。v2 的状态文件 TTL、
  收敛窗口、/proc 身份、试绑在这条路径上整体消失（manager 侧删除在 M-P4）。
- 数据面能力：CONNECT 隧道 + MITM 按 SNI 现签 + HTTP/1.1 缓冲转发；
  absolute-URI（http scheme）正向代理；407 鉴权门（CONNECT 与 absolute-URI
  同一条门，凭据只在内存里定长时间比对）；insecure_hosts 经
  ConnectTarget.tls_policy 落到连接执行器的两个档位（严格 = 系统信任库，
  放宽 = 仅精确命中可用；不存在"全局关校验"的表达路径）。改写语义与 v2
  注入器逐项一致：只改连接目标，不动请求内容、不动 Host 头、不动 SNI 基准。
- ConnectTarget（envboard-core::target）：authority / sni / resolved_addr /
  tls_policy / chained_proxy（恒 None 的扩展位）——基础配置与后续插件管道的
  唯一接缝。
- body 默认缓冲（8 MiB 上限，超限 413/502 而非静默流式）；101 升级走透传
  旁路，upgrade 链的请求头原样带走。上游连接一问一答（不复用）：热改规则/
  名单不会被连接池里的旧策略穿越。
- 验收：core/rs/crates/envboard-core/tests/data_plane.rs 六组 hermetic 断言
  （规则命中 200、未覆盖 502、407 门、insecure 对照 + 热翻转不重启、
  port_conflict、隧道保活多轮）。

### Added（v3 重构 M-P1：纯库引擎骨架 envboard-core）

v3 把数据面重构为**纯 Rust 库 + 进程内引擎实例**（分支
`refactor_envboard_v3_rust_engine_20260917`）。本阶段不改运行行为：manager 仍走
mitmproxy，本条目记录的是独立库面的落地。

- 新 crate `envboard-core`（内部依赖只有 `envboard-core-api`，已登记进 `deps`
  门禁边表）：
  - **共享 CA 的加载与物化**：直接读既有 confdir——`mitmproxy-ca.pem`（PKCS#1 RSA
    私钥 + 证书拼接）与 `mitmproxy-ca-cert.pem`。判据沿用 v2 并逐条实现：私钥与证书
    公钥**逐字节配对**、`-ca.pem` 内嵌证书与 `-ca-cert.pem` 一致、basicConstraints
    CA:TRUE、未过期；任一不满足 → 删除并重新物化，且**以 `CaOutcome::Regenerated
    { reason }` 交回上层**——坏 CA 的处置必须可见，禁止静默带病服务。
  - **新生成的 CA 用 ECDSA P-256**（签发库 rcgen 0.14 的 ring 后端不支持生成 RSA
    密钥，文档明示只有 aws-lc-rs 能生成）；**加载既有 RSA CA 并用它现场签发**已由
    进程内握手测试实证（`core/rs/crates/envboard-core/tests/mitm.rs`，fixture 为
    纯测试假值 CA，密钥无外部用途）。
  - **叶子证书按 SNI 现签现缓存**：容量 512、满则整体清空重签；剩余寿命不足 30 天
    即重签；私钥文件写盘 0600。
  - **rustls 接线**：MITM 服务侧配置（客户端 ALPN 只协商 `http/1.1`）与上游客户侧
    严格校验配置（系统信任库 `rustls-native-certs`）。信任库来源从 certifi 换成
    系统库会影响"哪些域名免名单通过"的集合，最终由 v3 README 的已知限制收口。
  - **自写定形 DER 读取**（TLV、PKCS#1↔PKCS#8 封装互转、证书 SPKI/CA 标志/有效期）：
    PKCS#1 兼容面不随上游库的支持摇摆。
- 新增 workspace 依赖：rustls 0.23（ring provider）/ tokio-rustls / rcgen 0.14 /
  rustls-native-certs / rustls-pemfile / time；`Cargo.lock` 已入库（rcgen 首次引入
  需一次联网预热，已完成）。
- **v1 协议面限制**：ALPN 只声明 HTTP/1.1，强制 h2 的客户端（gRPC 等）会失败——
  这是写明的非目标，不是静默降级。

### Added（按域名放宽上游证书校验）

- `Environment.insecure_hosts`：**完整域名清单**（默认空），命中 ⟺ 归一化后的 SNI 与该
  域名**完全相等** —— 无通配符、无子域继承、无后缀匹配；写入时**拒绝 `*` / `?`**：
  放宽上游证书校验是安全控制，必须逐条点名。**没有"整个环境全关"的开关。**
  - 生效方式是注入器接管 `tls_start_server`：名单内域名用 `VERIFY_NONE` 自建上游 context
    （ALPN / cipher / TLS 版本 / ECDH 曲线与不改写时逐项一致），名单外一律走 mitmproxy
    自己的严格校验。无 SNI 时退回上连地址判定；SNI 存在但没命中**不**退回。
  - **热生效**：清单写进每个环境自己的 `config.json`，注入器按轮询间隔（默认 5s）重读，
    运行中改不需要重启。

### Changed (breaking)（配置通道与规则寻址重做）

- **删除 `Environment.options`（任意 core 选项透传）**：它是一条绕过契约的任意通道，
  让"放宽上游证书校验"这类安全控制可以不经一等字段被打开。空对象 / `null` 作为**墓碑**
  接受并忽略（旧版每个环境都写 `options: {}`），**非空即 `invalid_config`** 并指出替代路径。
  `envboard env add/edit --option` / `--no-options`、`InstanceSpec.options`、
  `validate_options` denylist 全家桶、工作台的选项开关与文本框一并删除。
  （这条能力在被删除前**从未发布**，所以这里是直接移除，而不是"废弃再删"。）
- **`Environment.proxy_auth` 拆成 `proxy_user` + `proxy_password`**：同生共死（只有一边
  指向缺失的那一边报错）、字段级错误路径、不含 `:`（mitmproxy 的 `split(":")` 要求恰好
  一个冒号）。凭据明文只落在 0600 状态文件与实例启动参数；视图 / 日志 / SSE 只给
  `proxy_auth_enabled` 布尔，并且**记录进程身份时 cmdline 里的凭据值先脱敏成 `***`**
  （比对存活进程时两侧都脱敏）。旧形状读时**无损迁移**（整体 `trim` 后按第一个 `:` 切开），
  写出不再输出。
- **脚本 `--set` 通道收口**：注入器不再注册任何 `envboard_*` 选项，改读
  `<自身目录>/config.json`；规则也不再经 `--set envboard_rules=<path>` 下发，而是
  `<自身目录>/envboard.rules` 这条**固定名软链**。启动命令行只剩 `-s` / `confdir` /
  `listen_host` / `listen_port` / `proxyauth` —— **没有用户可控的 `--set`。**
- **规则库进账本**：`state.json` 增加 `rules[]`（含渲染后的完整正文），
  `<rules_dir>/<name>.rules` 变为**可再生物化产物**：缺失或被改坏按账本逐字节重建；
  物化目录里有而账本里没有的文件在启动对账时**回填**（升级不丢规则库）。
- **"规则缺失"不再让启动失败**：链不存在/悬空 ⇒ 该环境**不覆盖任何域名**（视图给出
  `rules_missing` 提示并尽量自愈）。绑定一个**不存在**的规则名则在写入时就拒绝 ——
  否则那会变成一次静默失效。
- **热 / 停机矩阵重排**：热 = `description` / `insecure_hosts` / `rules` 绑定 / 规则内容
  （绑定热生效由"原子换链 + 重写 config.json"实现）；停机 = `name` / `listen` /
  `proxy_user` / `proxy_password`。
- **健康判定增加配置哈希回执**：管理器记录期望配置的 sha256 与写入时刻，实例回执
  `config_hash`；不等但在 `reload_interval + 3s` 的**收敛窗口**内算收敛中，超窗才报
  `config_mismatch`；实例热重载失败写 `config_error` → `unhealthy`。状态文件里没有
  `config_hash` 的实例判定为**上一代**，reconcile 走 `restart`。
- 契约 fixture 重新基线化：删除 11 个 options 用例、重写 6 个凭据用例，再加上新增用例，
  总数 **58 → 68**（`environment/` 30 + `merge/` 9 + `rules/` 8 + `ports/` 6 +
  `lifecycle/` 7 + `insecure/` 8）。

### Changed (breaking)

- `spec/capabilities.md` **重写**：领域模型从 v1 的 8 字段一路收到**当前的 7 个**
  （`name` / `listen` / `rules` / `insecure_hosts` / `description` / `proxy_user` /
  `proxy_password`；中间形态是 5 字段 + `options`，再补第 6 个 `proxy_auth`，
  见上面那条 breaking）。
  删除 `dns_servers`、`hosts`、
  `domain_suffix`、`color`、`labels` 与 mapping / annotate / activate / resolve
  全部语义 —— v2 的切换模型是"一个环境 = 一个独立实例 + 一个独立端口"，
  不再有"当前环境"这个全局状态。
- `spec/errors.md`：删除 `dns_failure`、`disabled` 与 `rcode` 表；
  新增 `port_conflict`、`port_range_exhausted`、`config_mismatch`，并补上
  环境健康状态表（`stopped` / `starting` / `running` / `port_conflict` /
  `config_mismatch` / `unhealthy` / `failed`）。
- 契约 fixture **重新基线化**：v1 的 11 个 `environment/` + 2 个 `merge/` 全部重写
  （它们测的是被删除的字段），新增 `ports/` 与 `lifecycle/` 两组。
  当前总数与分目录见上面最后一条 breaking（**68** 个）。

### Added

- `spec/rules.md`：hosts 规则语法的 **BNF**，作为 Rust 实现与 Python 注入器的
  共同仲裁（v1 只有散文式描述，无法支撑"两侧输出逐字节一致"这条要求）。

### Added

- **Rust 核心（v2 M0 + M1）**：多环境管理器的实现落地，由 7 个 crate 组成，
  依赖方向由 `scripts/rust_dependency_lint.py` 强制（core-api 是根，纯逻辑 crate
  不得依赖运行时）。已实现的部分：
  - `envboard-core-api`：`ProxyCore` trait、`InstanceSpec`、`StatusReport`、统一错误码、
    `ClockPort`/`LoggerPort`（`InstanceSpec.options` 与其 denylist 已删除）。
  - `envboard-domain`：环境模型（校验/归一化/PATCH 合并；字段数随后续 breaking 变动，
    当前 7 个）、端口选择（`port.allocate` 的全部规则）、reconcile 决策
    （孤儿清理与 PID 复用保护）。
  - `envboard-rules`：hosts 解析与**确定性渲染**，与 Python 侧在 63 个用例上逐字节一致。
  - `envboard-core-fake`：只监听端口的测试替身 core —— **管理器测试因此完全不依赖
    mitmproxy**（"没装 mitmproxy 的机器也能跑默认流水线"这条验收条件）。
  - `envboard-manager`：环境 CRUD、随机端口分配与持久化、状态存储（原子写 + 0600）、
    单实例锁（flock）、健康判定（状态文件为主 + 契约回显比对）、reconcile、规则导入。
  - `envboard-cli`：`envboard` 二进制（`env`, `rules`, `run`, `status`）。
  - `envboard-contract-tests`：**消费 `spec/fixtures` 全部 golden case**。
- `scripts/verify_dual_impl.py`：跨语言对拍门禁 —— 63 个用例里 Rust 与 Python 输出必须
  逐字节一致，**已知分歧必须显式声明**（声明了却不再分歧也会失败，防止白名单掩盖新分歧）。
- `scripts/rust_dependency_lint.py`：Rust 侧的依赖方向与纯度门禁（含"纯逻辑 crate 不得
  依赖 tokio/libc"、workspace 根唯一、`Cargo.lock` 与 `rust-toolchain.toml` 在位）。
- `ci/verify.sh` 新增步骤：`rust-dep-lint` / `rust-fmt` / `rust-clippy` / `rust-check` /
  `rust-build` / `rust-test` / `contract-dual`，全部带 `--locked --offline`。

### Added（M2 — mitmproxy core + 注入器）

- `adapters/mitmproxy/envboard_mitmproxy.py`：**单文件、纯标准库**的注入器 ——
  读 `<自身目录>/config.json` 与固定名规则软链、在 `server_connect` 里改写上连目标、
  按域名放宽上游证书校验、回写状态文件。它只认自己目录旁边的固定路径，
  不碰别的状态目录 —— 所以随时可以丢掉。既是 addon（`-s`），也有 `--render` 模式
  供跨语言对拍（该模式**不需要 mitmproxy**）。
- `envboard-core-mitmproxy`：`ProxyCore` 的真实实现 —— 按启动契约拼 `--set` 参数、
  spawn `mitmdump`（带 `PR_SET_PDEATHSIG`）、**预物化并校验共享 CA**、
  物化注入器、等状态文件而非等进程、SIGTERM→SIGKILL 停止、按身份探活。
- `core.python` 的推导规则落地：shebang → uv-tool 布局 → 同目录，**每一步都真跑一次
  `import mitmproxy` 自检**，并要求与 `core.bin --version` 的版本一致。
  裸命令名（默认 `mitmdump`）先经 PATH 解析 —— 否则读不到 shebang，
  实测会报"找不到带 mitmproxy 的解释器"而它其实就在 PATH 上。
- **契约回显自检机制**：注入器核对"管理器下发的每个 `--set`"是否被宿主接受
  （`options_echo`），管理器据此判定 `config_mismatch`。宿主对拼错的 `--set`
  是静默忽略的，只有让注入器去 `ctx.options` 里查才知道。当前下发的键只剩
  `proxyauth`（安全控制，被静默忽略等于裸奔），外加 `config_hash` 这条更硬的回执。
- 实机测试 `live_manager.rs`：真管理器 + 真 mitmdump + 真改写（200 vs 502 判别性对照）
  + 代理凭据下发后 `options_echo` 必须回 "ok"；没有 mitmdump 时**跳过**而不是失败。

### Changed（日志通道与存活判定：从"读管道"改成"文件直写"）

- **实例日志默认改为文件直写**（`<state_dir>/logs/<env>.log`）：子进程的 stdout/stderr
  用 `Stdio::from(File)` 直接接文件（两路 `try_clone()` 共享 offset），**没有管道、
  没有读线程**。原来靠"两个读线程把管道读走"来避免子进程阻塞；实测管道容量 64 KiB、
  默认详细度约 307 字节/请求 → **213 个请求就能写满**，写满后 mitmdump 阻塞在自己的
  事件循环里、所有客户端一起挂住（不读管道的对照实验：第 212 个请求开始失败，读走
  65116 字节后立刻恢复）。文件由内核写盘，我们进程不在链路上，这条路径从设计上消失；
  顺带换来"日志跨重启留存、崩溃现场可追溯"。
  - 每次启动追加一行运行标记分段落；`PYTHONUNBUFFERED=1` 两种形态都保留
    （Python 对非 TTY 的文件同样是块缓冲）。
  - 新增 `--no-log-file`（退回"管道 + 内存环形缓冲"，磁盘零写入）与
    `--max-log-bytes`（默认 8 MiB，`0` = 不轮转，下限 64 KiB）；`--log-dir` 由
    "可选落盘目录"变为"覆盖默认目录"。
- **轮转是 copytruncate**（尾部搬去 `<env>.log.1` 后 `set_len(0)`）：**不能 rename** ——
  子进程还持着那个 inode 的 fd，改名之后它会继续写被移走的文件，新文件永远是空的。
- **读尾部改为有界**：只从文件末尾读 256 KiB 窗口再切行，并可跨 `.1` 往前拼；
  删掉原来整个文件 `read_to_string` 的兜底（日志一大就会把管理器拖住）。
- **判活看 `/proc/<pid>/stat` 的状态位**：僵尸（`Z`/`X`）不再算活着。原来只比
  PID + starttime，而僵尸这两个量都没变 → 判成"活着"。新增每 500 ms 的**回收任务**
  用 `try_wait()` 收尸（tokio 1.53.1 该方法是公开同步的）。一处修好，`health`
  / `reconcile` / 孤儿清理 / `terminate` 全部同时受益。
- **视图层与权威判定同源**：`health_blocking()` 原来"只要句柄表里有这个环境就返回
  `running`"，与带状态文件 TTL 的 `health()` 分叉；实测 `kill -9` 掉实例后工作台
  **60 秒仍报 `health=running, reason=None`**。现在两条路径共用同一套判据
  （进程存活 → 状态文件时效 → 契约回显），并且"由本管理器拉起、又没人叫它停、
  进程却没了"报 `failed`（带退出码或信号）而不是 `stopped`。
- 管道路径（`--no-log-file`）硬化：单行 8 KiB 上限、写盘不再每行 open/close、
  `Mutex` 中毒不再 `unwrap()`（一个读线程 panic 会连带把另一个也弄死，管道就再没人读了）。
- 工作台日志面板跟随 SSE 快照刷新（原来只在点"日志"时取一次，之后永远停在那一次快照上）。

### Added（文本自包含门禁）

- **`scripts/doc_scope_lint.py`**：包内文本不得引用本仓之外的本地文档（设计文档、开发计划、
  流程规范、工作区规范文件）：不给路径/链接/章节号、不点名字；指向本仓的路径必须真实存在；
  含 `§` 的行必须写明是哪份本仓文件（或"本文件"）。纳入 `ci/verify.sh` 默认层
  （`doc-scope-lint` 步骤），判据与豁免见 README 的「文本自包含」一节。
- 按同一条规则清理了包内既有引用：README / CHANGELOG / `spec/**` / 验收文档与
  **代码注释**里对外部文档的章节号引用、风险表编号（如 `R14`）一律改成自包含表述或指向
  本仓内真实存在的文件与标题；顺带修掉两处失效引用（对拍入口指向已删除的脚本、
  清单里"断言已不存在"的路径改为显式标记）。

### Added（部署工件）

- `scripts/systemd/envboard.service`：用户级 systemd unit（常驻管理器 + 工作台），
  附 headless（`run`）变体。设计取舍都在文件内注释：`StateDirectory=` 建状态目录
  （`ReadWritePaths=` 要求路径预先存在，首次安装会以 `226/NAMESPACE` 失败 —— 实测踩到）、
  `ExecStart` 全用绝对路径（systemd 的 PATH 不含 `~/.local/bin`）、`Restart=always` 的
  安全性来自持久化的期望状态、`KillMode=mixed` 避免孤儿实例占端口。
  已在真机跑完一轮启停：实例随服务停止而退出、重启后 reconcile 自动拉起、经代理的请求 200。

### Added（日志通道与存活判定）

- `envboard-manager/src/logs.rs`：日志文件的有界尾部读取（含跨轮转拼接）与 copytruncate
  轮转；日志文件落在管理器的状态目录里，所以**读与轮转都只有这一个实现**，
  core 侧不重复一份。
- `ProxyCore::last_exit`：进程**自己退出**时的退出原因（信号/退出码），供同步的视图层
  在不 `await` core 的前提下给出准确结论。
- 实机断言：`verify_live_v2.py` 新增 check 8（连打 320 个请求——足以越过旧的 64 KiB
  卡死阈值——全部成功，日志文件 97 KB 为证）与 check 9/9b/9c（实例被 SIGKILL 后工作台
  不再报 running、说清是被信号杀死、子进程被回收、日志仍可读、能重新拉起）；
  `live_manager.rs` 新增同款崩溃用例并覆盖 reconcile 重新拉起。证据与实测数字见
  `docs/acceptance/log-channel.md`。

### Fixed（日志通道与存活判定）

- `view_of` 有调用点是在**持有 `state` 锁**时调用 `health_blocking` 的，而
  `std::sync::Mutex` 不可重入 —— 判定路径里再取一次状态锁会让 `env add` 直接挂死。
  现在环境定义由调用方传入，判定路径不碰状态锁。

### Added（M3 — 工作台）

- `envboard-web`：axum 路由 + **内嵌前端**（`index.html` / `app.css` / `app.js`，
  全部 `include_str!`，零前端构建链、零 CDN）。
- REST 面：环境 CRUD/启停/重启/**显式重分配端口**/日志尾部、规则导入/查看/删除、
  **跨环境静态对比**（承接 v1 `all_envs` 的用例意图，且不发任何请求）、SSE 快照。
- 安全（三条都不可省）：默认只监听回环、Host 头校验防 DNS rebinding、
  变更类路由要求自定义头防 CSRF；非回环监听必须给 token。
- 严格 CSP（无 `unsafe-inline`），因此**样式与脚本必须外置**；验收断言三件事：
  JS 真的跑了 + 样式真的生效（读计算样式）+ 控制台无 CSP 报错。

### Fixed

- 端口探测改为**惰性**（找到第一个空闲端口即停）。本机实测单次 `bind` 约 **21ms**
  （WSL2 mirrored 模式，连绑定端口 0 也一样），按原设计"先把整个区间探一遍"会在
  16000–16999 上花约 21 秒；现在常见情形只探 1 次，最坏仍是 `max_attempts` 次。
- 注入器**不能**写 `from __future__ import annotations`：mitmdump 用 `-s` 加载脚本时
  不登记 `sys.modules`，而 `@dataclass` 解析字符串注解会去查它 → 启动即崩。
- mitmproxy 的选项系统**不支持 `float`**（启动即失败），间隔类选项一律用整秒。
- axum 0.7 的路径参数语法是 `:name`（`{name}` 是 0.8 的），写错的表现是**全 404**；
  另给 API 加了 JSON 404 兜底，避免前端只能报"响应不是 JSON"。

### Added（编辑已有环境）

- **工作台的「编辑」**：每行一个入口，把「新建环境」表单切到编辑模式（名字 / 端口 / 规则
  绑定 / 实例选项 / 描述五个字段全可改），提交走 `PATCH /api/environments/:name`。之所以
  复用同一个表单而不是行内编辑：字段集本来一样，两套 DOM 迟早不一致（v1 的教训）。
  实例**运行中**会把名字 / 端口 / 绑定 / 选项四个输入框锁住并说明原因，只放开描述 ——
  服务端仍会拒绝，但界面先把这条路封住，免得用户改完再吃一个红字。
- **规则库的「载入」**：把已导入的规则文件原文取回表单再改。原来只有「导入」，
  改一份规则等于盲覆盖（实测 10.8 KB 的规则文件就是这么被当成"没法编辑"的）。
- **CLI `env edit`**：`--rename` / `--port` / `--rules` / `--no-rules` / `--option` /
  `--no-options` / `--description`，空 patch 响亮失败（而不是"成功但什么都没改"）。
  与工作台**共用同一份** PATCH 构造，避免两处字段映射漂移。`ApiClient` 因此补了 `patch()`。
- **CLI `rules show`**：取回规则文件原文（改之前先看，同样是防盲覆盖）。
- 实机断言：`verify_live_v2.py` 新增 check 10 —— 运行中换绑定返回 409、描述热改 200、
  停止后补绑规则并改名换端口后，**真 mitmdump 按新配置把域名送到上游**（未绑定时是 502）。
  管理器侧新增 4 条用例（描述热改 / 绑定需停机 / 改名搬迁 / 旧标记作废），
  `thin_client.rs` 覆盖 `env edit` 走 HTTP 的往返。证据见
  `docs/acceptance/edit-environment.md`。

### Changed（编辑语义）

- **运行中换规则绑定改为拒绝**（`conflict`）。原来 `update()` 的注释写"改绑定是热更新"，
  与事实不符：实例在启动时通过 `--set envboard_rules=<path>` 固定规则**路径**，运行中换绑定
  它根本看不见，结果是"配置说绑了 A、实例还按 B 干活"。热重载的**是内容**（mtime 轮询
  同一个路径），这条能力不变。描述仍然随时可改。
  这个分叉是被一条单元测试逮到的：断言"热改绑定后仍是 running"，实际得到 `config_mismatch`
  —— 状态文件的 `rules_count` 回显如实报出了不一致。
- 改了 `listen` 或 `rules` 绑定即作废该环境旧的失败标记（`port_conflict` /
  `config_mismatch`）：标记陈述的是"上一个配置失败了"，留着它会让界面拿**新**端口号报旧冲突。
- `update()` 补一条审计日志（`updated <env>: changed name, listen, …`），
  系统日志里能看到"谁在什么时候动了哪个环境的哪几项"。
- **CLI 的本地 API 发现不再跨状态目录兜底**：显式给了 `--state-dir` 时只认该目录下的
  `runtime/api.json`，不再退回默认端口。原来 `--state-dir /tmp/scratch` 会被 8900 上
  一个跟它毫无关系的常驻实例接管 —— 命令看着成功了，改的却是**别人的**状态
  （本机实测撞上：`env add` 打到了正在运行的工作台上）。

### Fixed（编辑已有环境）

- **改名之后日志面板指向旧名字**：面板的目标名字不会跟着改，之后每秒一次的尾部读取都是
  404；而那个 404 会让整次 `refresh()` 失败，把"已保存。"顶成一条红字错误提示
  （真浏览器验收时踩到：用户会以为改动没生效）。现在面板目标跟着环境列表自愈，
  日志读取的失败也只落在日志面板里。
- **名字输入框的 `pattern` 在 `v` 标志下是非法正则**：Chrome 校验 `pattern` 用 `v`
  （不是 `u`），而 `v` 模式里裸的 `-` 是非法字符类成员 —— 于是整个 pattern 被浏览器
  **静默忽略**，只在控制台留一行 `SyntaxError`，原生校验形同不存在。转义成 `\-` 后
  两种模式都合法（`9bad` / `Beta` 现在真的会被前端挡下）。这类故障 curl 断言看不出来，
  只有真浏览器的控制台会说话。
- `doc-scope-lint` 漏掉**无后缀文本文件**：按后缀筛文件时 `.gitignore` 整类不在扫描范围内，
  而它的注释同样可能指向本仓之外的文档（实测确实有一条）。现在把 `.gitignore` / `.dockerignore`
  显式列入扫描集合（文件数 111 → 112）。

### Changed（文档脱敏）

- `docs/acceptance/v0.1.0.md`（v1 实机验收转录）里的家目录用户名与当时读到的**真实系统 DNS 地址**
  换成通用值：家目录统一写成 `/home/u/`（形状不变），DNS 地址改为文档专用网段 `192.0.2.53`
  （RFC 5737）。断言、时间戳、退出码一并保持原样 —— 脱敏只改"能指回某个具体人/某台真实机器"
  的部分。文件开头写明了这次替换，避免读者以为它就是当时的原始输出。
- 同一轮排查确认**真实 hosts 文件从未入库**（`examples/hosts.txt` 一直在忽略之列，
  入库的是脱敏示例 `examples/hosts.sample.txt`，用的是 RFC 5737 地址与 `example.com`）。

### Removed（M4 — 收敛与迁移，breaking）

- **v1 的 Python 实现整块移除**：`src/envboard/`（model / registry / mapping / resolver /
  rules / ports / errors / addon / web）、`addons/envboard.py`、`web/*`、v1 的 81 个单测，
  以及 `pyproject.toml`（本包**不再发布 Python 分发物** —— Python 这一侧只剩被二进制内嵌的
  单文件注入器）。`git tag v0.1.0` 保留 v1 全貌。
- 随之退休的门禁脚本：`dependency_lint.py`（分层检查已由 `rust_dependency_lint.py` 覆盖）、
  `pack_check.py`（改校验二进制产物）、`verify_live.sh`（55 条断言里能脚本化的部分迁到
  `verify_live_v2.py`）。
- 命名门禁**收紧**：没有分发物之后，`slothtron` 这个 token 在整棵树里（路径与代码内容）
  **零命中**，唯一例外是规则脚本自身。

### Notes

- `scripts/artifact_check.py` 取代 `pack_check.py`：校验发布物（二进制 + 内嵌注入器 +
  必需产物 + 零 v1 残留），并断言注入器源码真的被 `include_str!` 嵌进了二进制。
- `ci/verify.sh` 的 `smoke` 换成 v2 形态：二进制能跑 + `--core fake` 建环境 + 注入器
  `--render` 在**没有 mitmproxy**的解释器下也能跑。
- 实机层脚本化（`scripts/verify_live_v2.py`）：两个环境同时可用且结果不同、规则热重载、
  Host/CSRF 防护、CSP 形态、日志落盘、崩溃恢复 —— 补上了 v1 立下的
  "断言必须可重跑"这条规矩在 v2 上的缺口。

### Changed（工作台 UI 升级：明亮主题三视图）

- 前端三资产（`core/rs/crates/envboard-web/assets/` 下的 `index.html` / `app.css` /
  `app.js`）整体重写：从"单表格 + 深色"升级为明亮主题的三视图工作台 —— 环境 /
  规则库 / 跨环境对比三个视图、统计卡、环境详情（概览 / 配置 / 规则三页签）、
  可折叠日志栏（按事件类型过滤 + 搜索 + 跟随开关）。**REST API、SSE 与 CSP 形态
  零改动**，这是纯前端层的重写。
- 设计令牌收进 `app.css` 第 ① 区：表面三级、边框两级、文本四级灰阶（带对 `--panel`
  的实测对比度）、语义色「原色 + bg(10%) + border(30%)」三件套、4px 间距基数、五档
  圆角。组件层不写十六进制色值，主题切换只改令牌区。规范与理由见 `docs/ui-spec.md`。
- 破坏性操作（删除环境）改为**就地二次确认**（后果说明 + 确认/取消），不再用原生
  `confirm()` —— 与"无内联脚本"的 CSP 约束一致，且样式可控。
- 图标改用同文档 SVG `<symbol>` 精灵 + `<use>`：不引字体、不发请求，
  `stroke: currentColor` 使同一份图标随语境变色。
- 可访问性：`prefers-reduced-motion` 降级；`[hidden]` 加 `display:none!important` 修正
  （作者样式会盖过 UA 默认值）；文本四级灰阶全部达到可读对比度。
- 日志栏过滤维度是**事件类型**（请求/响应/连接/运行/异常）而不是日志级别：实测日志行
  不带 INFO/WARN 前缀，按级别过滤会是个假功能。

### Fixed（工作台 UI 升级）

- **引导链隔离**：`renderLogs` 排在 `connectEvents()` / `refreshAll()` 之前且无保护，
  一个写错的容器 id（`log-filters` 改名只落在 CSS/JS 两侧，HTML 未跟上）就能让
  `renderLogFilters` 抛空指针，进而中断整条引导链 —— 数据请求根本不发、页面停在空壳、
  `envboardReady` 永远停在 `"pending"`。现在每个引导步骤过 `step()` 壳：坏掉的步骤记进
  `dataset.envboardError`、ready 明确置 `"no"`，数据加载照常继续。
- `index.html` 注释里写出的脚本标签字面量会命中 `scripts/verify_live_v2.py` 的
  「页面源码里无内联脚本」断言（实机层 4a FAIL）—— 注释改写为不出现该字面量。

### Fixed（工作台 UI 合规修复）

三份走查/审计记录（`docs/acceptance/ui-design-conformance.md`、
`envboard-ui-compliance-audit.md`、`envboard-ui-audit-report.md`）合并后的修复。
第三份的对照基准写的是 v1.0，与仓内现行的 v2.0 规范版本错配，其 6 项量化结论经
实测反证后未采纳（如"0.8px 边框"其实是 `1px ÷ dpr 1.25` 的设备像素换算、
"`.info-grid` 无分隔线"其实已有 1px 缝隙网格）。

**可用性**

- **顶栏在 641–720px 溢出且刷新按钮不可达**：顶栏固有最小宽度 721px 而没有横向
  滚动，`body` 的 `overflow: hidden` 会把溢出部分裁掉且滚不到 —— 实测 720px 溢出
  1px、681px 起刷新按钮完全移出视口。新增 720 断点（隐藏版本徽章、视图切换只留
  图标、图标按钮放大到 36px），并让两枚状态徽章可收缩省略，使顶栏宽度不再随
  SSE 文案变化（断线时连接徽章会变长约 80px）。实测 641–900px 全区间零溢出。
- **日志时间戳对比度 3.91:1 低于 WCAG AA**：`.log-ts` / `.log-peer` 原用 `--muted-2`，
  而日志栏底色是 `--bg-2`，`--muted-2` 只对白系表面达标。改用 `--muted`（实测
  4.66:1）。侧栏的空态文案与日志的 `k-conn` / `k-raw` 标记同病同治。
- **重分配端口没有二次确认**：原先点一下就直接 `POST /reallocate`，而端口一变客户端
  此前的 `export https_proxy=…` 立即失效。改为就地二次确认并写明该后果。

**规范承诺但未落地的部分**

- 规则库补「N 条 · M 个 IP」（优先读规则文件头的 `# entries: N ip: M`，缺失时按
  数据行现算），列表不再需要逐个「载入」去数。
- 日志栏补「导出」与「清空」：导出当前视图（尊重过滤与搜索）为 `.log`；清空只清
  视图缓冲并同时暂停跟随（磁盘日志文件不动）—— 不暂停的话下一个快照就把它拉回来。
- 表单补**字段级错误文本**：`markInvalid` 在输入框下方插一行原因（原先只有描边变色，
  原因只走会自己消失的 toast）。
- 状态徽章改为**中文标签**，原文移入 `data-health`（`config_mismatch` 这种原始值写在
  徽章上等于没写）；概览的「状态 / 期望」改为「实际状态 / 期望状态」，不等时标红。
- 复制成功后按钮原地反馈「已复制」（不整块重渲染，否则会被每秒的快照冲掉）。
- 日志栏无日志时给空态文案，区分"本来没有"与"过滤后为空"。

**设计系统收敛**

- 字号收进 `--text-xs…3xl` 九档并转换组件层 48 处声明，消灭 10 / 10.5px；
  新增 `--input-h` / `--btn-h` / `--btn-h-sm`。
- 组件尺寸对齐规范：徽章 20→24、统计卡图标 34→38、统计值 20→22、输入框 30→32、
  日志类型标记 46→44、命令文本 12→12.5、信息值 13.5→14。
- 消灭规范外的第四种按钮高度：`.btn.icon-btn.sm` 26×26 → 30×30。
- 间距孤立值收进令牌：输入框 `padding: 6px`（规范点名禁止值）、徽章 `gap: 5px`、
  信息格 `gap: 3px`、环境项 `gap: 2px`、视图切换 `padding: 3px`。
- Tab 条底色 `--bg-3` → `--bg-2`；空态图标 48px 色块 → 40px @50%；
  新增计数 chip 激活态；切换类控件补 `:active`；禁用按钮不再响应 hover。
- 统计卡的「环境总数」不再默认高亮：它代表"不筛选"，不是被选中的筛选项。
- 侧栏计数改为跟随当前可见条数（原先筛到空列表时右侧仍挂着总数）。
- 侧栏页脚改为展示**访问地址 + 暴露面 + 数据新鲜度**（原来显示状态目录）。

**规范与文档对齐**

- `docs/ui-spec.md`：圆角纪律改按实际层级（卡片 10px，不是 lg；原措辞与
  `docs/design/ui-spec-full.md` 冲突）、补 `--muted-2` 只在白系表面的纪律、
  补字号尺度与尺寸三档、响应式断点补 720 并说明它为何是硬需求、删掉未实现的
  "抽屉 / 日志栏加高"描述。
- `docs/design/ui-spec-full.md`：删除 Dropdown 与 ActivityList 两个组件并写明取舍
  （CSP 禁内联 style，浮层代价高）、日志过滤维度改为事件类型、Toast 位置改为右下、
  日志行时间戳说明"允许为空"、`--purple` 补上"日志分段标记"这第二个固定用途、
  基础集去掉没有界面的「设置」与「文件」、头部去掉不可解析的路径引用。
- `docs/design/tokens-light.css` 按 `app.css` 第 ① 区重新生成（新增 `--radius-xs`
  等 4 项缺失令牌）；深色块只在令牌文件保留一份，`app.css` 不再重复。
- `app.css` 注释补上 1px 缝隙网格与 Tab `-1px` 负边距的意图说明（它们看着像间距违规）。

### Added（安全增强 — URL token / 代理鉴权 / 对外服务开关）

- **工作台 URL token 鉴权（默认启用）**：token 鉴权**开箱即开** —— 不给 `--token`
  时自动生成 128 bit 随机 token（`/dev/urandom`；异常环境退化到时间+PID 混合），
  启动日志打印 `dashboard: http://<host>:<port>/?token=<值>`
  （通配监听地址用 `127.0.0.1` 展示），点击直达。显式 `--without-token` 才能关闭，
  且**非回环监听拒绝关闭**（无鉴权对外不允许）；与 `--token` 同给是配置冲突。
  生成的 token 写入 `<state_dir>/runtime/api.json`（0600），CLI 瘦客户端自动发现并
  带上，不再依赖用户手抄。`guard()` 在 header（`x-envboard-token`）优先之外接受
  URL `?token=`（浏览器直接打开工作台、SSE 的 EventSource 都带不了自定义头），
  比较改为常量时间；前端捕获后立即 `history.replaceState` 抹掉地址栏中的 token，
  之后全部请求走 header。`/app.css` / `/app.js` 内嵌静态资产豁免 token（浏览器子资源请求带不了凭据），API 与页面本体不豁免。
- **环境字段 `proxy_auth`**（契约 4 字段 → 5）：`user:password` 形态（恰好一个冒号、
  两段非空、无空白/控制字符、总长 ≤128），`null` = 不启用。启用后实例以
  `--set proxyauth=<user:password>` 启动并纳入 `envboard_expect` 回显自检；
  `proxyauth` 加入两级 options denylist（凭据唯一来源是环境字段）；运行中修改返回
  `conflict`（实例启动时读取）。`InstanceSpec` 增加 `proxy_auth`；
  `EnvView` 增加 `proxy_auth_enabled` 布尔，视图（含代理命令）**不回显凭据**。
  状态存储本就原子写 + 0600，凭据入库沿用该通道。fixtures：`environment/` 新增
  7 个（2 valid + 5 invalid），总数 40 → **47**（`valid-listen-all-interfaces`
  钉住 `listen.host = 0.0.0.0` 的合法性）。
- **工作台「对外服务」开关与访问鉴权表单字段**：环境表单新增
  「对外服务（绑定 0.0.0.0）」复选框与「访问鉴权」（password 输入）字段；
  `formPayload` 修复"编辑只发 `{port}` 导致 host 被整体替换语义重置回默认"的隐患
  （host 或端口任一变化时两个字段都发全）；勾选 0.0.0.0 而未启用鉴权时表单给
  红色警示；概览新增「访问鉴权」格、非回环监听地址加警示色；运行中锁定
  新增的两个输入框。创建时勾选对外服务则端口必填（自动分配只支持默认监听地址）。