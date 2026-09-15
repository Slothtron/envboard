#!/usr/bin/env bash
# 实机端到端验证：真起一个 mitmweb + 本插件，验证 Dashboard / REST / 环境切换 / 真 DNS 解析。
#
#   bash scripts/verify_live.sh
#   MITMWEB=/path/to/mitmweb bash scripts/verify_live.sh
#
# 依赖：mitmweb 可执行文件、curl、以及可用的本机 DNS。
# 全部构件都放在临时目录，退出时清理；不会碰你的 ~/.mitmproxy。
set -uo pipefail

# mitmproxy 在非 TTY 下会缓冲日志；不关缓冲就 grep 不到启动行。
export PYTHONUNBUFFERED=1

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MITMWEB="${MITMWEB:-$(command -v mitmweb || true)}"
if [[ -z "$MITMWEB" && -x "$HOME/.local/bin/mitmweb" ]]; then
  MITMWEB="$HOME/.local/bin/mitmweb"
fi
if [[ -z "$MITMWEB" ]]; then
  echo "verify-live SKIPPED: mitmweb not found (set MITMWEB=/path/to/mitmweb)"
  exit 0
fi

WEB_PORT="${WEB_PORT:-18081}"
PROXY_PORT="${PROXY_PORT:-18080}"
TOKEN="envboard-live-$$"
TMP="$(mktemp -d)"
CONFDIR="$TMP/confdir"
JAR="$TMP/cookies.txt"
LOG="$TMP/mitmweb.log"
PASS=0
FAIL=0

cleanup() {
  if [[ -n "${PID:-}" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT

ok()   { PASS=$((PASS + 1)); echo "  ok   $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL $1"; }
check(){ if [[ "$2" == *"$3"* ]]; then ok "$1"; else bad "$1 (got: ${2:0:220})"; fi; }
not()  { if [[ "$2" != *"$3"* ]]; then ok "$1"; else bad "$1 (unexpected: $3)"; fi; }

api()  { # api METHOD PATH [BODY]
  local method="$1" path="$2" body="${3:-}"
  local args=(-sS -X "$method" -b "$JAR" -c "$JAR")
  # tornado 的 xsrf_cookies 对所有非安全方法生效 —— 包括没有 body 的 DELETE。
  if [[ "$method" != "GET" && "$method" != "HEAD" ]]; then
    args+=(-H "X-XSRFToken: $XSRF")
  fi
  if [[ -n "$body" ]]; then
    args+=(-H 'Content-Type: application/json' --data "$body")
  fi
  curl "${args[@]}" "$BASE$path"
}

echo "=== verify-live ==="
echo "mitmweb    : $MITMWEB"
echo "dashboard  : http://127.0.0.1:$WEB_PORT/envboard/"
echo "confdir    : $CONFDIR"

"$MITMWEB" \
  -s "$ROOT/addons/envboard.py" \
  --set confdir="$CONFDIR" \
  --set web_password="$TOKEN" \
  --set web_open_browser=false \
  --set web_host=127.0.0.1 \
  --set web_port="$WEB_PORT" \
  --set listen_host=127.0.0.1 \
  --set listen_port="$PROXY_PORT" \
  >"$LOG" 2>&1 &
PID=$!

BASE="http://127.0.0.1:$WEB_PORT"
for _ in $(seq 1 60); do
  if grep -q "Web server listening" "$LOG" 2>/dev/null; then break; fi
  if ! kill -0 "$PID" 2>/dev/null; then
    echo "mitmweb exited early. log:"; cat "$LOG"; exit 1
  fi
  sleep 0.5
done
if ! grep -q "Web server listening" "$LOG" 2>/dev/null; then
  echo "timed out waiting for mitmweb. log:"; cat "$LOG"; exit 1
fi
echo ""
echo "--- mitmweb log ---"
sed -n '1,12p' "$LOG"

# 1. 静态页 + 强制下发 XSRF cookie
HTML="$(curl -sS -b "$JAR" -c "$JAR" "$BASE/envboard/?token=$TOKEN")"
check "dashboard HTML served" "$HTML" "多环境 DNS 工作台"
XSRF="$(awk '/_mitmproxy_xsrf/ {print $7}' "$JAR" | tail -1)"
if [[ -n "$XSRF" ]]; then ok "xsrf cookie issued"; else bad "xsrf cookie missing"; fi

# 1b. Dashboard 的 JS 必须是同源外部文件。
#     mitmweb 的 CSP 是 default-src 'self'（未给 script-src 开口），内联 <script>
#     会被浏览器静默拒绝 —— HTML 仍然 200、页面看着正常，但一行 JS 都不跑。
check "dashboard references external app.js" "$HTML" 'src="app.js"'
not   "dashboard has no inline <script>"   "$HTML" "<script>"
ASSET_CT="$(curl -sS -o /dev/null -w '%{content_type}' -b "$JAR" "$BASE/envboard/app.js?token=$TOKEN")"
check "app.js served as javascript" "$ASSET_CT" "application/javascript"
ASSET_JS="$(curl -sS -b "$JAR" "$BASE/envboard/app.js?token=$TOKEN")"
check "app.js carries the dashboard logic" "$ASSET_JS" "loadAll()"
MISSING_ASSET="$(curl -sS -o /dev/null -w '%{http_code}' -b "$JAR" "$BASE/envboard/nope.js?token=$TOKEN")"
if [[ "$MISSING_ASSET" == "404" ]]; then ok "unknown asset -> 404"; else bad "unknown asset returned $MISSING_ASSET"; fi

# 2. 默认环境 bootstrap
BODY="$(api GET "/envboard/api/environments?token=$TOKEN")"
check "bootstrap env 'local'" "$BODY" '"name": "local"'
check "bootstrap active=local" "$BODY" '"active": "local"'

# 3. 状态：系统 DNS 被动态读到
STATUS="$(api GET "/envboard/api/status?token=$TOKEN")"
check "status reports system dns source" "$STATUS" '"source": "system"'
check "status reports config path" "$STATUS" 'envboard.json'

# 4. 创建 prod 环境（显式 DNS 服务器）
CREATED="$(api POST "/envboard/api/environments?token=$TOKEN" \
  '{"name":"prod","dns_servers":["'"${LIVE_DNS:-192.0.2.53}"'"],"domain_suffix":"prod.example.com","color":"#f05252"}')"
check "create env prod" "$CREATED" '"name": "prod"'
check "create env prod dns" "$CREATED" '"dns_servers": ['

# 5. 非法输入必须响亮失败（ip:port 是 mitmproxy_rs 的硬约束）
BADREQ="$(api POST "/envboard/api/environments?token=$TOKEN" \
  '{"name":"bad","dns_servers":["10.0.0.1:53"]}')"
check "rejects ip:port with invalid_config" "$BADREQ" '"code": "invalid_config"'
check "rejects ip:port with field path" "$BADREQ" 'dns_servers[0]'

# 6. 切换环境
SWITCHED="$(api PUT "/envboard/api/active?token=$TOKEN" '{"name":"prod"}')"
check "switch active -> prod" "$SWITCHED" '"active": "prod"'

# 6b. 无 body 的 DELETE 也必须带 XSRF 才放行；带上就应当成功
api POST "/envboard/api/environments?token=$TOKEN" '{"name":"tmp-env"}' >/dev/null
DELETED="$(api DELETE "/envboard/api/environments/tmp-env?token=$TOKEN")"
check "delete inactive env succeeds (XSRF on empty body)" "$DELETED" '"deleted": "tmp-env"'

# 7. 删除激活环境必须被拒绝
DELACTIVE="$(api DELETE "/envboard/api/environments/prod?token=$TOKEN")"
check "refuses deleting active env" "$DELACTIVE" '"code": "conflict"'

# 8. 真实正向解析（走 prod 的 DNS 服务器）
RESOLVED="$(api POST "/envboard/api/resolve?token=$TOKEN" \
  '{"env":"prod","hosts":["example.com"]}')"
if [[ "$RESOLVED" == *'"rcode": "noerror"'* ]]; then
  ok "live forward resolution via prod DNS"
else
  echo "  note: live forward resolution did not succeed (offline?). got: ${RESOLVED:0:200}"
fi

# 9. 映射表（按环境过滤 / 全部环境）
MAPS="$(api GET "/envboard/api/mappings?env=prod&token=$TOKEN")"
check "mapping table has example.com" "$MAPS" 'example.com'
ALLMAPS="$(api GET "/envboard/api/mappings?env=*&token=$TOKEN")"
check "mappings env=* returns everything" "$ALLMAPS" 'example.com'
API_BAD="$(api GET "/envboard/api/mappings?env=*&limit=abc&token=$TOKEN")"
check "bad limit is a 400, not a 500" "$API_BAD" '"code": "invalid_config"'
BADBODY="$(curl -sS -X POST -b "$JAR" -c "$JAR" -H "X-XSRFToken: $XSRF" \
  -H 'Content-Type: application/json' --data 'not json' \
  "$BASE/envboard/api/resolve?token=$TOKEN")"
check "malformed JSON body is a 400" "$BADBODY" '"code": "invalid_config"'

# 10. 多环境对比：同一域名分别向每套环境的 DNS 各查一次
api POST "/envboard/api/environments?token=$TOKEN" \
  '{"name":"beta","dns_servers":["'"${LIVE_DNS:-192.0.2.53}"'"]}' >/dev/null
ALLEV="$(api POST "/envboard/api/resolve?token=$TOKEN" '{"hosts":["example.com"],"all_envs":true}')"
check "resolve across all environments" "$ALLEV" '"all_envs": true'
check "all-env result covers local" "$ALLEV" '"local"'
check "all-env result covers prod" "$ALLEV" '"prod"'
check "all-env result covers beta" "$ALLEV" '"beta"'

# 10b. 反向解析整条链路已移除 —— 路由应当不存在
GONE="$(curl -sS -o /dev/null -w '%{http_code}' -b "$JAR" \
  "$BASE/envboard/api/reverse?token=$TOKEN")"
if [[ "$GONE" == "404" ]]; then ok "reverse endpoint removed (404)"; else bad "reverse endpoint returned $GONE"; fi

# 11. 静态 hosts 覆盖优先于 DNS
api PUT "/envboard/api/environments/prod?token=$TOKEN" \
  '{"hosts":{"static.example.com":"203.0.113.7"}}' >/dev/null
STATIC="$(api POST "/envboard/api/resolve?token=$TOKEN" \
  '{"env":"prod","hosts":["static.example.com"]}')"
check "static hosts override wins" "$STATIC" '203.0.113.7'

# 11b. 规则文件：导入（两种写法 + 非法行 + 冲突）→ 绑定 → 生效 → 优先级 → 删除保护
RULES_DIR="$CONFDIR/rules"
SRC="$TMP/hosts.txt"
{
  printf '%s\n' '10.0.0.1 api.example.com auth.example.com'
  printf '%s\n' 'beta.example.com 10.0.0.2'      # 反序写法
  printf '%s\n' '# comment'
  printf '%s\n' 'not-an-ip.example.com'          # 非法行
  printf '%s\n' '10.0.0.3 dup.example.com'
  printf '%s\n' '10.0.0.4 dup.example.com'       # 冲突，后出现者胜
} >"$SRC"
IMPORTED="$(api POST "/envboard/api/rules?token=$TOKEN" \
  '{"name":"live","path":"'"$SRC"'"}')"
check "rules import accepted all valid hosts" "$IMPORTED" '"accepted": 4'
check "rules import counts distinct ips"      "$IMPORTED" '"ips": 3'
check "rules import ignores invalid line"     "$IMPORTED" '"reason": "too-few-tokens"'
check "rules import conflict keeps last"      "$IMPORTED" '"kept": "10.0.0.4"'
if [[ -f "$RULES_DIR/live.rules" ]]; then ok "rules file written to disk"; else bad "rules file missing"; fi
check "normalized file is ip-first"    "$(cat "$RULES_DIR/live.rules" 2>/dev/null)" '10.0.0.4 dup.example.com'
not   "normalized file drops comments" "$(cat "$RULES_DIR/live.rules" 2>/dev/null)" 'not-an-ip'
not   "normalized file drops invalid"  "$(cat "$RULES_DIR/live.rules" 2>/dev/null)" '# comment'

RULES_LIST="$(api GET "/envboard/api/rules?token=$TOKEN")"
check "rules list shows the file" "$RULES_LIST" '"name": "live"'
check "rules list reports ips"    "$RULES_LIST" '"ips": 3'
RULES_SHOW="$(api GET "/envboard/api/rules/live?token=$TOKEN")"
check "rules show returns entries" "$RULES_SHOW" '"api.example.com": "10.0.0.1"'

BOUND="$(api PUT "/envboard/api/environments/prod?token=$TOKEN" '{"rules_file":"live"}')"
check "bind rules file to env" "$BOUND" '"rules_file": "live"'

VIA_RULES="$(api POST "/envboard/api/resolve?token=$TOKEN" \
  '{"env":"prod","hosts":["beta.example.com"]}')"
check "rules file drives resolution" "$VIA_RULES" '"ips": ["10.0.0.2"]'
check "rules hit is a static source"  "$VIA_RULES" '"source": "static"'
check "static_from names the rules file" "$VIA_RULES" '"static_from": "rules:live"'

# 环境 inline hosts 必须压过规则文件（同一个 host 两处都有）
api PUT "/envboard/api/environments/prod?token=$TOKEN" \
  '{"hosts":{"static.example.com":"203.0.113.7","api.example.com":"203.0.113.99"}}' >/dev/null
PRECEDENCE="$(api POST "/envboard/api/resolve?token=$TOKEN" \
  '{"env":"prod","hosts":["api.example.com"]}')"
check "env inline hosts beat the rules file" "$PRECEDENCE" '203.0.113.99'
check "static_from says environment"         "$PRECEDENCE" '"static_from": "environment"'

BOUND_DEL="$(api DELETE "/envboard/api/rules/live?token=$TOKEN")"
check "refuses deleting a bound rules file" "$BOUND_DEL" '"code": "conflict"'
api PUT "/envboard/api/environments/prod?token=$TOKEN" '{"rules_file":""}' >/dev/null
check "unbind then delete succeeds" \
  "$(api DELETE "/envboard/api/rules/live?token=$TOKEN")" '"deleted": "live"'

# 11c. 规则文件也支持直接粘贴文本（Dashboard 走的就是这条路）
TEXT_IMPORT="$(api POST "/envboard/api/rules?token=$TOKEN" \
  '{"name":"pasted","text":"198.51.100.1 pasted.example.com\nbad\n"}')"
check "rules import from pasted text" "$TEXT_IMPORT" '"accepted": 1'
check "pasted import reports skip"    "$TEXT_IMPORT" '"skipped": 1'
api DELETE "/envboard/api/rules/pasted?token=$TOKEN" >/dev/null

# 12. XSRF 缺失必须被 tornado 拒绝
NOXSRF="$(curl -sS -o /dev/null -w '%{http_code}' -X POST -b "$JAR" \
  -H 'Content-Type: application/json' --data '{}' \
  "$BASE/envboard/api/environments?token=$TOKEN")"
if [[ "$NOXSRF" == "403" ]]; then ok "write without XSRF -> 403"; else bad "write without XSRF returned $NOXSRF"; fi

# 13. 未认证必须 403
UNAUTH="$(curl -sS -o /dev/null -w '%{http_code}' "$BASE/envboard/api/environments")"
if [[ "$UNAUTH" == "403" ]]; then ok "unauthenticated read -> 403"; else bad "unauthenticated read returned $UNAUTH"; fi

# 14. 持久化：状态文件权限 0600 且含 prod
STATE="$CONFDIR/envboard.json"
if [[ -f "$STATE" ]]; then
  ok "state file persisted"
  MODE="$(stat -c '%a' "$STATE")"
  if [[ "$MODE" == "600" ]]; then ok "state file mode 0600"; else bad "state file mode is $MODE"; fi
  check "state file contains prod" "$(cat "$STATE")" '"prod"'
else
  bad "state file missing at $STATE"
fi

echo ""
echo "--- addon log lines ---"
grep -i "envboard" "$LOG" | head -10 || true

echo ""
echo "verify-live: $PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]
