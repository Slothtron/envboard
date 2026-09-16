# envboard 规则文件语法（BNF）

语言中立契约，**两份实现的共同仲裁**：Rust 侧（`envboard-rules`，负责导入/规范化/落盘）
与 Python 侧（注入器，负责运行时解析与热重载）都必须对着本文件写。语法分歧一律回到这里裁决，
**不得**以某一侧的实现为准。

> 本文件是 v2 新增的。v1 只有 `capabilities.md` 里的散文式描述，没有形式化语法 ——
> 而 v2 要求"两份实现输出逐字节一致"，没有可引用的判据就不成立（见
> `capabilities.md` 的「多语言实现的仲裁规则」）。

规则文件的使命：把**人手写的、杂乱的 hosts 风格输入**，规范成一份**确定性**的
静态覆盖表 —— 即 `host → ip` 的映射，在建连时用于改写上连目标地址。

---

## 1. 词法与行结构

```bnf
file          = [ bom ] , { line } ;
bom           = "\uFEFF" , { "\uFEFF" } ;        (* 行首的全部 BOM 一并剥掉 *)
line          = { ws } , [ body , { ws } , [ comment ] ] , { ws } , [ eol ] ;
comment       = "#" , { any-char } ;              (* 行内首个 # 之后全部丢弃 *)
body          = { ws } , token , { ws1 , token } , { ws } ;

token         = ip | host-token ;
host-token    = { any-char - ( ws | "#" ) } ;
ws            = " " | "\t" | { unicode-white-space } ;
ws1           = ws , { ws } ;                     (* 至少一个空白字符 *)
eol           = "\n" | "\r\n" | "\r" ;
```

**行分隔**：契约只承认 `"\n"`、`"\r\n"`、`"\r"` 三种行终止符。
（Python 的 `str.splitlines()` 还会在 `\v` `\f` `\x1c` `\x1d` `\x1e` `\x85`
`\u2028` `\u2029` 处断行 —— 那属于 v1 的实现细节，**不在 v2 契约内**；
Rust 与 Python 两份实现都不得依赖它。这条收窄是为了"逐字节一致"可判定。）

**注释**：`#` 的行内截断发生在**原始行**上、取**第一个** `#`。因此域名里不能包含 `#`
（`10.0.0.1 ho#st.com` 会被截成 `10.0.0.1 ho`，然后 `ho` 被当作非法 host 丢掉）。

**空白**：`ws` 指 Unicode 白字符。**已知差异点**（M1 必须用对拍用例钉住）：
Python 的 `str.split()` 还会在 `\x1c`–`\x1f` 上切分，而 Rust 的
`str::split_whitespace()` 不会。若将来需要在这些字符上逐字节一致，两侧要显式实现
同一张白字符表；当前 fixture 只使用空格与制表符。

**分类**（按处理顺序，取第一个匹配）：

| 行 | 判定 | 计入 |
|---|---|---|
| strip 后以 `#` 开头（或截断后 body 为空且原 strip 后以 `#` 开头） | 注释行 | `comment_lines` |
| 截断后 body 为空，且不是注释行 | 空行 | `blank_lines` |
| body 非空 | 数据行 | `data_lines` |

> 注意"截断后为空"与"注释行"的区分依据是**原文** strip 后是否以 `#` 开头，
> 不是截断结果的形状。`   ` 是空行，`   # x` 是注释行，
> `10.0.0.1 a.example.com   # 行尾注释` 是数据行。

---

## 2. 语法

```bnf
entry         = ip-first | host-last ;

ip-first      = ip , ws1 , host , { ws1 , host } ;
host-last     = host , ws1 , { host , ws1 } , ip ;

ip            = ipv4 | ipv6 | "[" , ipv6 , "]" ;
```

判定顺序（只看 token，不看原文位置）：

1. `tokens[0]` 是 IP 字面量 → **ip-first**，`tokens[1..]` 全是 host；
2. 否则 `tokens[-1]` 是 IP 字面量 → **host-last**，`tokens[..-1]` 全是 host；
3. 否则 → 整行跳过，原因码 `no-ip-literal`。

两种写法**等价**：`10.0.0.1 a.example.com` 与 `a.example.com 10.0.0.1` 产出同一条映射。

---

## 3. 归一化

### 3.1 host

```bnf
host          = { label , "." } , label ;
label         = ( alpha | digit | "_" ) , [ { alpha | digit | "_" | "-" } , ( alpha | digit | "_" ) ] ;
```

落地规则（等价表述，实现按此写）：

1. `strip` → 转小写 → 去掉**尾部**所有 `.`（根点）；
2. 若以 `*.` 开头，剥掉这一个前缀（**只剥一次**，且仅限开头）；
3. 结果非空，且长度 ≤ 253；
4. 以 `.` 切分后，每个 label 必须匹配 `^(?!-)[A-Za-z0-9_-]{1,63}(?<!-)$` ——
   即长度 1–63、**不能以 `-` 开头或结尾**、允许 `_`；空 label（`a..b`）不合法。

> **不支持通配符匹配。** `*.wild.example.com` 只是被归一化成 `wild.example.com`，
> 语义是"这一个域名"，不是"该后缀下的所有域名"。这条在 v1 里也是如此，
> 在这里写明是因为它容易被误读成通配。

### 3.2 ip

1. `strip`；若首尾同时是 `[` 与 `]` 则剥掉这一对（仅 IPv6 会用方括号）；
2. 必须能被标准的 IP 字面量解析器解析（等价于 Python `ipaddress.ip_address`
   或 Rust `IpAddr::from_str`）；
3. **拒绝**：`ip:port` 写法、主机名、前导零形式的 IPv4（`010.0.0.1`）、
   越界八位组（`300.1.2.3`）、**带 zone id 的 IPv6**（`fe80::1%eth0`）。

> **注意两个解析器的已知差异，契约按"更严的一侧"取**：Python 的
> `ipaddress.ip_address()` **接受** zone id，而 Rust 的 `IpAddr::from_str` 不接受；
> 反之 Rust 接受 IPv4-mapped 的 `::ffff:1.2.3.4` 而 Python 也接受。契约统一为
> **拒绝 zone id**，两份实现都要显式拒绝（Python 侧不能依赖 `ipaddress` 的默认行为，
> 要自己判 `%`）。

**IP 文本不做规范化 —— 拼法保留。**
剥掉方括号之后，token 的文本就是 entries 里的值：`2001:0db8::1` 不会被改写成
`2001:db8::1`，`[2001:db8::2]` 会变成 `2001:db8::2`。这是 v1 的语义，v2 保持不变，
理由有两条：

* 渲染出来的文件要能被**重新导入**且不产生 diff；改写拼法会在第一次导入时就改动
  用户手写的文件（虽然等值，但 review 起来像"工具乱改我的东西"）；
* M1 期间 Python 侧的对拍参考实现就是 v1 的解析器，**它保留原始拼法**。
  若 Rust 单方面做规范化，两份实现的渲染输出会在含非规范拼写的 fixture 上分叉 ——
  而这正是对拍要抓的东西，不该由契约本身制造假阳性。

排序仍然按 IP **数值**序（先解析再排序），所以拼法不影响次序。

---

## 4. 逐项容错与跳过原因码

**非法内容永远不会让整次导入失败** —— hosts 文件是人手写的，一个笔误不该让其余几百条
一起报废。但每一处忽略都**必须**如实记录（行号 + 原因码 + 原文片段），不允许静默吞掉。

| 原因码 | 触发 | `detail` 字段 | 影响范围 |
|---|---|---|---|
| `too-few-tokens` | 数据行的 token 数 < 2 | 第一个 token | 整行 |
| `no-ip-literal` | 首尾 token 都不是 IP 字面量 | `""`（空串） | 整行 |
| `invalid-host` | host 位置的 token 不合法，或该 token 本身就是 IP 字面量 | 该 token | **只丢该项**，同行其余 host 保留 |

补充约定：

- 原因码是**稳定字面量**，UI 与 fixture 都按它断言，不要改字。
- `invalid-host` 的一条特例：`10.0.0.7 10.0.0.8` —— 第二个 IP 出现在 host 位置，
  记 `invalid-host`（而不是被当成 host 收下）。
- 同一条数据行可能同时产出多条 `invalid-host`（每个坏 token 一条），顺序按 token 出现顺序。
- **被跳过的 host 不影响同一行里其它 host**，也不影响该行的 IP 是否生效。

---

## 5. 冲突

同一 host 出现多次：

| 情形 | 行为 |
|---|---|
| 映射到**不同** ip | **后出现者胜**；记一条冲突 `[host, dropped, kept]`，`kept` = 后出现的（生效的），`dropped` = 先前的 |
| 映射到**相同** ip | 不记冲突（重复写同一行是常见的手写习惯，不是错误） |

判定顺序：当两个 IP **文本**不同即算冲突。比较的是解析出来的原始文本（见本文件 §3.2
"拼法保留"），所以 `2001:db8::1` 与 `2001:0db8::1` 是**两次不同的映射**，会记一条冲突 ——
它们数值上等价，但契约不做 IP 规范化，因此按文本判等。

---

## 6. 渲染（持久化格式）

渲染必须**确定性**：同一份 `entries` + `source` 必须得到逐字节相同的文本。
`parse(render(x)) == parse(x)`，且重复导入同一份文件不产生 diff。

```bnf
rendered      = header , [ source-line ] , counts-line , blank , { entry-line } ;
header        = "# envboard rules file — normalized `ip host...` lines.\n"
              , "# Generated by envboard; re-import the source instead of editing this by hand.\n" ;
source-line   = "# source: " , source-text , "\n" ;          (* 仅当 source 非空 *)
counts-line   = "# entries: " , N , "  " , "ip: " , M , "\n" ; (* 两个空格，见下 *)
blank         = "\n" ;
entry-line    = ip , " " , host , { " " , host } , "\n" ;
```

规则：

1. **首两行是固定字节** —— 上面引号内的内容，含破折号 `—`（U+2014）、反引号与结尾句点。
   改一个字就是 breaking change，两份实现同时改。
2. `# source: ` 行只在 `source` 非空时输出。
3. 计数行是 `# entries: <N>  ip: <M>` —— `N` 与 `ip:` 之间是**两个空格**。
   `N` = 归一化后的 host 数，`M` = 去重后的 ip 数。
4. 计数行后**恰好一个空行**，然后是数据行。
5. 数据行**每个 ip 一行**；ip 之间按 **IP 数值序**排序（先比地址族，IPv4 全部在
   IPv6 之前；同族按地址字节序）—— 这样 `10.0.0.2` 排在 `10.0.0.10` 之前。
6. 同一行内的 host 按 **Unicode 码点序**排序，空格分隔。
   （UTF-8 的字节序与码点序一致，所以两侧的 `sort` 直接可用。）
7. 文件以**恰好一个** `\n` 结尾。
8. 渲染前的 `entries` 必须已归一化。**若两个键归一化后撞车（如同时有 `A.com`
   与 `a.com`），必须 `invalid_config` 响亮失败**，不得依赖映射的迭代顺序静默取一个。
   （这是相对 v1 的**收紧**：v1 在渲染时是"后写覆盖"。）

### 示例

输入：

```
10.0.0.1 a.example.com b.example.com
b.example.com 10.0.0.2
# 注释 / 空行 / 非法行
300.1.2.3 bad.example.com
```

输出（`source` 为空）：

```
# envboard rules file — normalized `ip host...` lines.
# Generated by envboard; re-import the source instead of editing this by hand.
# entries: 2  ip: 2

10.0.0.1 a.example.com
10.0.0.2 b.example.com
```

注意 `b.example.com` 的后写覆盖生效（`10.0.0.2`），并且产生一条冲突记录；
`300.1.2.3 bad.example.com` 整行被跳过（原因码 `no-ip-literal`）。

---

## 7. 结果载荷（契约测试可比对的字段）

| 字段 | 含义 |
|---|---|
| `entries` | `{归一化 host: 规范化 ip 文本}`，后出现者胜 |
| `accepted` | `entries` 的条目数 |
| `ips` | `entries` 中去重后的 ip 数 |
| `conflicts` | `[[host, dropped, kept], ...]`，按发生顺序 |
| `skipped` | 跳过记录列表：`{line, text, reason, detail}`；`line` 从 **1** 开始，`text` 是该行**截断注释并 strip 后**的 body |
| `stats` | `{total_lines, blank_lines, comment_lines, data_lines, accepted, skipped, conflicts}` —— `skipped` / `conflicts` 是**条数**，不是列表 |

`total_lines` 计所有物理行，且 `blank_lines + comment_lines + data_lines == total_lines`。

---

## 8. 生效语义（解析之外，但同属契约）

1. 规则在**建连时**改写上连目标地址：只改"往哪连"，不改请求内容、不改 `Host` 头、
   不改 SNI 基准（正常路径下）。
2. 只对**请求里给出了域名**的上连生效；客户端直连 IP 时无域名可匹配。
3. 只对**流量指向了该环境端口**的请求生效 —— v1 那种改系统 DNS、全机生效的行为
   不再是本产品的语义。
4. 已知代价：改写后的地址与请求 host 不相等，上游连接池因此**不再复用**该 host 的连接，
   每个请求都会新建上连（正确性不受影响）。
