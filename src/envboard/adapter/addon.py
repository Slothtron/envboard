"""mitmproxy addon 接线层。

职责边界（AGENTS.md §6）：本模块只做**宿主翻译** —— 注册选项、实现 hook、
把 core 的能力暴露成 mitmproxy 命令与 Web 路由。业务逻辑一律在 ``core``。

加载方式::

    mitmweb -s addons/envboard.py
    mitmdump -s addons/envboard.py --mode dns
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
from collections.abc import Sequence
from typing import Any

from mitmproxy import command
from mitmproxy import ctx
from mitmproxy import dns as mdns
from mitmproxy import http

from ..core.errors import ConflictError
from ..core.errors import EnvBoardError
from ..core.errors import InvalidConfig
from ..core.errors import NotFoundError
from ..core.mapping import MappingIndex
from ..core.model import Environment
from ..core.model import normalize_host
from ..core.registry import EnvRegistry
from ..core.resolver import ResolverService
from ..core.rules import RULES_SUFFIX
from ..core.rules import parse_hosts_text
from ..core.rules import render_rules
from ..infra.clock import SystemClock
from ..infra.dns_forward import MitmproxyRsForwardResolver
from ..infra.rules_store import CachedRulesLookup
from ..infra.rules_store import FileRulesStore
from ..infra.rules_store import default_rules_dir
from ..infra.store import JsonFileStore
from ..infra.store import default_path
from ..infra.system_dns import MitmproxyRsSystemDns

logger = logging.getLogger(__name__)

ANNOTATION_PREFIX = "[env:"


class EnvBoardAddon:
    """环境工作台 addon。

    设计要点：

    * **L1 观测为默认**：切换环境只改变"如何解读流量"（DNS 归属、flow 注解），
      不改写任何流量。改写属于 L2，本版本不实现。
    * 所有 DNS 查询都走异步路径，绝不在 hook 里做阻塞 IO ——
      mitmproxy 的 ``blocking`` hook 会暂停整个 layer，阻塞 IO 会拖垮事件循环。
    """

    name = "envboard"

    def __init__(self) -> None:
        self.clock = SystemClock()
        self.index = MappingIndex(self.clock)
        self.registry: EnvRegistry | None = None
        self.service: ResolverService | None = None
        self.forward: MitmproxyRsForwardResolver | None = None
        self.system_dns = MitmproxyRsSystemDns()
        self.rules_store: FileRulesStore | None = None
        self.rules: CachedRulesLookup | None = None
        self._config_path = ""
        self._tasks: set[asyncio.Task] = set()
        self._refresh_task: asyncio.Task | None = None
        self._mounted = False
        self._enabled = True
        self._annotate_flow = True
        self._annotate_field = "comment"
        self._passive = True
        self._ttl = 300

    # ------------------------------------------------------------ 选项

    def load(self, loader) -> None:  # noqa: ANN001 - mitmproxy 的 loader 无公开类型
        loader.add_option(
            "envboard_enabled",
            bool,
            True,
            "Enable the envboard multi-environment workbench.",
        )
        loader.add_option(
            "envboard_config",
            str,
            "",
            "Path to the envboard state file. Empty means <confdir>/envboard.json.",
        )
        loader.add_option(
            "envboard_web_prefix",
            str,
            "/envboard",
            "URL path prefix under which the envboard dashboard is mounted (mitmweb only).",
        )
        loader.add_option(
            "envboard_annotate_flow",
            bool,
            True,
            "Tag flows with the environment a host was observed in.",
        )
        loader.add_option(
            "envboard_annotate_field",
            str,
            "comment",
            "Where to write the environment tag: flow comment, flow metadata, or both.",
            choices=["comment", "metadata", "both"],
        )
        loader.add_option(
            "envboard_passive_capture",
            bool,
            True,
            "Collect host/ip mappings passively from DNS flows (requires `--mode dns`).",
        )
        loader.add_option(
            "envboard_dns_timeout",
            int,
            5,
            "DNS query timeout in seconds.",
        )
        loader.add_option(
            "envboard_cache_ttl",
            int,
            300,
            "Default TTL in seconds for cached host/ip mappings.",
        )
        loader.add_option(
            "envboard_max_mappings",
            int,
            50000,
            "Maximum number of tracked hostnames before the oldest entries are evicted.",
        )
        loader.add_option(
            "envboard_watch_hosts",
            Sequence[str],
            [],
            "Hostnames to resolve automatically against the active environment.",
        )
        loader.add_option(
            "envboard_refresh_interval",
            int,
            0,
            "Auto-refresh interval in seconds for `envboard_watch_hosts`. 0 disables it.",
        )
        loader.add_option(
            "envboard_rules_dir",
            str,
            "",
            "Directory holding normalized rules files (`<name>.rules`). "
            "Empty = `<confdir>/rules`.",
        )

    def configure(self, updated: set[str]) -> None:
        self._enabled = bool(ctx.options.envboard_enabled)
        self._annotate_flow = bool(ctx.options.envboard_annotate_flow)
        self._annotate_field = str(ctx.options.envboard_annotate_field)
        self._passive = bool(ctx.options.envboard_passive_capture)
        self._ttl = max(1, int(ctx.options.envboard_cache_ttl))

        config_path = str(ctx.options.envboard_config or "") or default_path(
            str(ctx.options.confdir)
        )
        if self.registry is None or config_path != self._config_path:
            self._build(config_path)
        elif {"envboard_dns_timeout", "envboard_max_mappings"} & updated:
            self._build(config_path)

    def _build(self, config_path: str) -> None:
        timeout = max(1, int(ctx.options.envboard_dns_timeout))
        registry = EnvRegistry(JsonFileStore(config_path), self.clock, logger=logger)
        registry.load()
        self.registry = registry
        self._config_path = config_path
        self.index = MappingIndex(
            self.clock, max_entries=max(100, int(ctx.options.envboard_max_mappings))
        )
        self.forward = MitmproxyRsForwardResolver(timeout=float(timeout))
        rules_dir = str(ctx.options.envboard_rules_dir or "") or default_rules_dir(
            str(ctx.options.confdir)
        )
        self.rules_store = FileRulesStore(rules_dir)
        self.rules = CachedRulesLookup(self.rules_store)
        self.service = ResolverService(
            registry,
            self.index,
            self.forward,
            self.system_dns,
            self.clock,
            logger=logger,
            rules=self.rules,
            default_ttl=self._ttl,
        )
        env_count = len(registry.names())
        bound = sum(1 for e in registry.list() if e.get("rules_file"))
        logger.info(
            f"envboard: loaded {env_count} environment(s) from {config_path} "
            f"(active={registry.active}); rules dir {rules_dir} "
            f"({len(self.rules_store.names())} file(s), {bound} env bound)"
        )

    # ------------------------------------------------------------ 生命周期

    async def running(self) -> None:
        if not self._enabled:
            logger.info("envboard: disabled via envboard_enabled=false")
            return
        self._mount_dashboard()
        self._start_refresh_loop()

    async def done(self) -> None:
        for task in [self._refresh_task, *self._tasks]:
            if task is not None and not task.done():
                task.cancel()
        self._refresh_task = None
        self._tasks.clear()

    def _mount_dashboard(self) -> None:
        from . import web

        try:
            self._mounted = web.mount(self, str(ctx.options.envboard_web_prefix))
        except Exception as exc:  # noqa: BLE001 - Dashboard 挂载失败不得阻断代理
            logger.warning(f"envboard: failed to mount dashboard: {exc!r}")

    def _start_refresh_loop(self) -> None:
        interval = int(ctx.options.envboard_refresh_interval)
        watch = [str(h) for h in ctx.options.envboard_watch_hosts]
        if not watch:
            return
        if interval <= 0:
            logger.info(
                "envboard: envboard_watch_hosts is set but envboard_refresh_interval=0; "
                "resolving once."
            )
            self.spawn(self._refresh_once(watch), "envboard-refresh-once")
            return
        self._refresh_task = self.spawn(
            self._refresh_loop(watch, interval), "envboard-refresh-loop"
        )

    async def _refresh_loop(self, watch: list[str], interval: int) -> None:
        while True:
            try:
                await self._refresh_once(watch)
            except asyncio.CancelledError:
                raise
            except Exception as exc:  # noqa: BLE001
                logger.warning(f"envboard: refresh failed: {exc!r}")
            await asyncio.sleep(interval)

    async def _refresh_once(self, watch: list[str]) -> None:
        assert self.service is not None
        results = await self.service.resolve_hosts(watch)
        ok = sum(1 for r in results if r.get("ips"))
        logger.info(f"envboard: refreshed {ok}/{len(results)} host(s) for env={self.registry.active}")

    def spawn(self, coro, name: str) -> asyncio.Task | None:
        """在当前事件循环上排一个后台任务并纳入生命周期管理。"""
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:  # pragma: no cover - 仅在非事件循环上下文触发
            logger.warning(f"envboard: no running loop, cannot schedule {name}")
            return None
        task = loop.create_task(coro, name=name)
        self._tasks.add(task)
        task.add_done_callback(self._tasks.discard)
        return task

    # ------------------------------------------------------------ hook

    async def dns_response(self, flow: mdns.DNSFlow) -> None:
        """被动采集：mitmproxy 以 ``--mode dns`` 运行时，直接读取 DNS 响应。"""
        if not (self._enabled and self._passive) or self.service is None:
            return
        response = flow.response
        if response is None:
            return
        question = response.question or (flow.request.question if flow.request else None)
        if question is None or question.type not in (mdns.types.A, mdns.types.AAAA):
            return
        ips: list[str] = []
        ttl = self._ttl
        for record in response.answers:
            try:
                if record.type == mdns.types.A:
                    ips.append(str(record.ipv4_address))
                elif record.type == mdns.types.AAAA:
                    ips.append(str(record.ipv6_address))
                else:
                    continue
                ttl = min(ttl, int(record.ttl)) if record.ttl else ttl
            except Exception:  # noqa: BLE001 - 畸形记录跳过
                continue
        if not ips:
            return
        try:
            self.service.record_passive(question.name, ips, ttl=max(1, ttl))
        except EnvBoardError as exc:
            logger.debug(f"envboard: passive capture rejected: {exc}")

    def request(self, flow: http.HTTPFlow) -> None:
        self._annotate(flow)

    def response(self, flow: http.HTTPFlow) -> None:
        self._annotate(flow)

    def _annotate(self, flow: http.HTTPFlow) -> None:
        if not (self._enabled and self._annotate_flow) or self.service is None:
            return
        request = getattr(flow, "request", None)
        if request is None:
            return
        host = normalize_host(getattr(request, "host", "") or "")
        if not host:
            return
        ip = ""
        server_conn = getattr(flow, "server_conn", None)
        address = getattr(server_conn, "address", None)
        if address:
            ip = str(address[0])
        try:
            info = self.service.annotate(host, ip)
        except EnvBoardError as exc:
            logger.debug(f"envboard: annotate skipped: {exc}")
            return
        if not info:
            return
        selected = info["active"]
        env_name = selected["env"]
        if self._annotate_field in ("comment", "both"):
            marker = f"{ANNOTATION_PREFIX}{env_name}]"
            existing = flow.comment or ""
            if marker not in existing:
                flow.comment = f"{marker} {existing}".strip()
        if self._annotate_field in ("metadata", "both"):
            flow.metadata["envboard"] = {
                "env": env_name,
                "ips": selected.get("ips", []),
                "hosts": selected.get("hosts", []),
                "source": selected.get("source", ""),
                "matches": info["matches"],
            }

    # ------------------------------------------------------------ 命令

    def _require_service(self) -> ResolverService:
        if self.service is None or self.registry is None:
            raise InvalidConfig("envboard is not initialised yet", field="envboard")
        return self.service

    # ------------------------------------------------------------ 规则文件

    def _require_rules(self) -> tuple[Any, Any]:
        if self.rules_store is None or self.rules is None or self.registry is None:
            raise InvalidConfig("envboard is not initialised yet", field="envboard")
        return self.rules_store, self.rules

    @staticmethod
    def rules_name_from_path(path: str) -> str:
        """从源文件路径推一个合法的规则名（`hosts.txt` → `hosts`）。"""
        stem = os.path.splitext(os.path.basename(path))[0].strip().lower()
        cleaned = re.sub(r"[^a-z0-9_-]+", "-", stem).strip("-")
        if not cleaned or not cleaned[0].isalpha():
            cleaned = f"rules-{cleaned}" if cleaned else "rules-import"
        return cleaned[:32].rstrip("-") or "rules-import"

    def rules_path(self, name: str) -> str:
        store, _ = self._require_rules()
        return os.path.join(store.dir_path(), (name or "").strip().lower() + RULES_SUFFIX)

    def rules_list(self) -> list[dict[str, Any]]:
        """规则文件清单，附每个文件被哪些环境绑定。"""
        store, lookup = self._require_rules()
        bound: dict[str, list[str]] = {}
        for env in self.registry.list():
            name = str(env.get("rules_file") or "")
            if name:
                bound.setdefault(name, []).append(str(env["name"]))
        rows: list[dict[str, Any]] = []
        for name in store.names():
            entries = lookup.entries(name)
            rows.append(
                {
                    "name": name,
                    "entries": len(entries),
                    "ips": len(set(entries.values())),
                    "environments": bound.get(name, []),
                    "path": self.rules_path(name),
                }
            )
        return rows

    def import_rules_text(self, name: str, text: str, *, source: str = "") -> dict[str, Any]:
        """解析 hosts 风格文本 → 写规范化规则文件 → 返回解析报告。

        非法内容只被忽略并逐条记录，**不会**让整次导入失败 —— 这正是
        "非法行规则自动忽略"的字面语义。文本为空时仍写出一份空规则文件
        （显式清空，而不是静默什么都不做）。
        """
        store, lookup = self._require_rules()
        report = parse_hosts_text(text)
        key = (name or "").strip().lower()
        store.write(key, render_rules(report.entries, source=source))
        lookup.invalidate(key)
        payload = report.to_json()
        payload["name"] = key
        payload["path"] = self.rules_path(key)
        payload["source"] = source
        return payload

    def import_rules_file(self, path: str, name: str = "") -> dict[str, Any]:
        """从磁盘读一份 hosts 风格文件并导入。

        读文件属于**宿主接线**（core 不碰文件系统），所以这一步留在适配层。
        """
        resolved = os.path.abspath(os.path.expanduser(path))
        try:
            with open(resolved, encoding="utf-8") as fh:
                text = fh.read()
        except OSError as exc:
            raise InvalidConfig(
                f"cannot read rules source {resolved}: {exc}", field="path"
            ) from exc
        target = (name or "").strip().lower() or self.rules_name_from_path(resolved)
        return self.import_rules_text(target, text, source=resolved)

    def rules_show(self, name: str) -> dict[str, Any]:
        store, lookup = self._require_rules()
        key = (name or "").strip().lower()
        text = store.read(key)
        entries = lookup.entries(key)
        return {
            "name": key,
            "path": self.rules_path(key),
            "text": text,
            "entries": entries,
            "count": len(entries),
            "ips": len(set(entries.values())),
        }

    def rules_bind(self, env_name: str, rules_name: str = "") -> Environment:
        """把规则文件绑定到环境；``rules_name`` 为空表示解绑。"""
        store, lookup = self._require_rules()
        key = (rules_name or "").strip().lower()
        if key and key not in store.names():
            raise NotFoundError(
                f"unknown rules file {key!r}; import it first (envboard.rules.import)",
                field="rules_file",
            )
        env = self.registry.update(env_name, {"rules_file": key})
        lookup.invalidate(key)
        return env

    def rules_remove(self, name: str) -> None:
        store, lookup = self._require_rules()
        key = (name or "").strip().lower()
        users = [
            str(e["name"])
            for e in self.registry.list()
            if str(e.get("rules_file") or "") == key
        ]
        if users:
            raise ConflictError(
                f"rules file {key!r} is still bound to environment(s) {users}; "
                'unbind first (envboard.rules.bind <env> "")',
                field="name",
            )
        store.delete(key)
        lookup.invalidate(key)

    @command.command("envboard.env.list")
    def env_list(self) -> Sequence[str]:
        self._require_service()
        return [
            f"{'*' if e['active'] else ' '} {e['name']:<16} "
            f"dns={','.join(e['dns_servers']) or '<system>'} "
            f"suffix={e['domain_suffix'] or '-'} hosts={len(e['hosts'])} "
            f"rules={e.get('rules_file') or '-'}"
            for e in self.registry.list()
        ]

    @command.command("envboard.env.current")
    def env_current(self) -> str:
        self._require_service()
        return self.registry.active

    @command.command("envboard.env.switch")
    def env_switch(self, name: str) -> str:
        self._require_service()
        env = self.registry.activate(name)
        return f"active environment is now {env.name}"

    @command.command("envboard.env.create")
    def env_create(self, name: str, dns_servers: Sequence[str]) -> str:
        self._require_service()
        env = self.registry.create({"name": name, "dns_servers": list(dns_servers)})
        return f"created environment {env.name}"

    @command.command("envboard.env.set")
    def env_set(self, name: str, patch_json: str) -> str:
        """用 JSON patch 更新环境，例如 ``{"domain_suffix":"beta.example.com"}``。"""
        self._require_service()
        try:
            patch: Any = json.loads(patch_json)
        except ValueError as exc:
            raise InvalidConfig(f"patch is not valid JSON: {exc}", field="patch") from exc
        if not isinstance(patch, dict):
            raise InvalidConfig("patch must be a JSON object", field="patch")
        env = self.registry.update(name, patch)
        return f"updated environment {env.name}"

    @command.command("envboard.env.remove")
    def env_remove(self, name: str) -> str:
        self._require_service()
        self.registry.delete(name)
        self.index.forget_env(name)
        return f"removed environment {name}"

    @command.command("envboard.rules.list")
    def rules_list_cmd(self) -> Sequence[str]:
        self._require_service()
        rows = self.rules_list()
        if not rows:
            return [f"no rules files in {self.rules_store.dir_path() if self.rules_store else '-'}"]
        return [
            f" {r['name']:<16} entries={r['entries']:<6} ip={r['ips']:<5} "
            f"envs={','.join(r['environments']) or '-'}"
            for r in rows
        ]

    @command.command("envboard.rules.import")
    def rules_import_cmd(self, path: str, name: str = "") -> Sequence[str]:
        """把一份 hosts 风格文件规范化后写入规则文件。

        ``path`` 里的非法行会被自动忽略；``name`` 省略时按文件名推导。
        """
        self._require_service()
        payload = self.import_rules_file(path, name)
        stats = payload["stats"]
        lines = [
            f"imported rules '{payload['name']}' from {payload['source']}",
            f"  accepted={stats['accepted']} host(s) over {payload['ips']} ip "
            f"| skipped={stats['skipped']} | conflicts={stats['conflicts']}",
            f"  written to {payload['path']}",
        ]
        for item in payload["skipped"][:5]:
            lines.append(
                f"  skipped line {item['line']} ({item['reason']}): {item['text'][:70]}"
            )
        if len(payload["skipped"]) > 5:
            lines.append(f"  ... and {len(payload['skipped']) - 5} more skipped")
        return lines

    @command.command("envboard.rules.show")
    def rules_show_cmd(self, name: str) -> Sequence[str]:
        self._require_service()
        payload = self.rules_show(name)
        head = [
            f"{payload['name']}: {payload['count']} host(s) over {payload['ips']} ip",
            f"  {payload['path']}",
        ]
        return head + [
            f"  {host} -> {ip}" for host, ip in sorted(payload["entries"].items())
        ]

    @command.command("envboard.rules.bind")
    def rules_bind_cmd(self, env: str, name: str = "") -> str:
        """把规则文件绑定到环境；``name`` 留空表示解绑。切环境即切规则文件。"""
        self._require_service()
        bound = self.rules_bind(env, name)
        if bound.rules_file:
            return f"environment {bound.name} now uses rules file {bound.rules_file}"
        return f"environment {bound.name} no longer uses a rules file"

    @command.command("envboard.rules.remove")
    def rules_remove_cmd(self, name: str) -> str:
        self._require_service()
        self.rules_remove(name)
        return f"removed rules file {name}"

    @command.command("envboard.dns.servers")
    def dns_servers(self, name: str) -> Sequence[str]:
        service = self._require_service()
        report = service.report_servers(name or None)
        return [
            f"env={report['env']} source={report['source']}",
            f"effective={','.join(report['effective']) or '<none>'}",
            f"system={','.join(report['system']) or '<none>'}",
        ]

    @command.command("envboard.resolve")
    def resolve(self, host: str) -> str:
        """异步解析 host（命令本身是同步的，结果写入映射表）。"""
        self._require_service()
        cached = self.index.by_host(self.registry.active, host)
        self.spawn(
            self.service.resolve_hosts([host]), f"envboard-resolve-{normalize_host(host)}"
        )
        if cached:
            return f"{host} -> {', '.join(sorted({c.ip for c in cached}))} (cached)"
        return f"scheduled lookup for {host}; run `envboard.mappings {host}` shortly"

    @command.command("envboard.mappings")
    def mappings(self, query: str) -> Sequence[str]:
        self._require_service()
        rows = self.index.snapshot(
            env=self.registry.active, query=query or "", limit=200
        )
        return [
            f"{r['env']:<10} {r['host']:<44} {r['ip']:<40} {r['source']}"
            for r in rows
        ] or ["<no mappings>"]

    @command.command("envboard.overview")
    def overview(self) -> Sequence[str]:
        service = self._require_service()
        stats = self.index.stats()
        return [
            f"active={self.registry.active} config={self._config_path}",
            f"environments={','.join(self.registry.names())}",
            f"servers={','.join(service.report_servers()['effective']) or '<none>'}",
            f"index hosts={stats['hosts']} entries={stats['entries']} hits={stats['hits']}",
            f"dashboard={'mounted' if self._mounted else 'unavailable (needs mitmweb)'}",
        ]

    @command.command("envboard.refresh")
    def refresh(self) -> str:
        self._require_service()
        watch = [str(h) for h in ctx.options.envboard_watch_hosts]
        self.spawn(self._refresh_once(watch), "envboard-refresh")
        if not watch:
            return "no watchlist configured (set envboard_watch_hosts)"
        return f"scheduled refresh of {len(watch)} host(s) for env={self.registry.active}"
