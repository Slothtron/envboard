"""解析编排：把"环境 + 域名"变成映射索引里的一条条 host -> ip 记录。

设计要点（语义见 core/spec/capabilities.md）：

* 静态 hosts 覆盖优先，命中则不查 DNS。
* 只做正向解析（host -> ip）。IP 反查 host 不在范围内。
* 并发受限，避免一次刷新打出几百个并发查询。
* 单条失败不拖垮整批：失败以结果对象的 ``rcode`` 表达，不抛异常。
"""

from __future__ import annotations

import asyncio
from collections.abc import Iterable
from collections.abc import Sequence
from typing import Any

from .errors import DnsError
from .mapping import INFINITE_TTL
from .mapping import SOURCE_ACTIVE
from .mapping import SOURCE_PASSIVE
from .mapping import SOURCE_STATIC
from .mapping import MappingIndex
from .model import normalize_host
from .ports import Clock
from .ports import ForwardResolver
from .ports import Logger
from .ports import LookupResult
from .ports import RCODE_ERROR
from .ports import RulesLookup
from .ports import SystemDnsPort
from .registry import EnvRegistry

DEFAULT_TTL = 300
MAX_CONCURRENCY = 16


class _NullLogger:
    def debug(self, msg: str) -> None: ...
    def info(self, msg: str) -> None: ...
    def warning(self, msg: str) -> None: ...
    def error(self, msg: str) -> None: ...


class ResolverService:
    def __init__(
        self,
        registry: EnvRegistry,
        index: MappingIndex,
        forward: ForwardResolver,
        system_dns: SystemDnsPort,
        clock: Clock,
        *,
        logger: Logger | None = None,
        rules: RulesLookup | None = None,
        default_ttl: int = DEFAULT_TTL,
        concurrency: int = MAX_CONCURRENCY,
    ) -> None:
        self._registry = registry
        self._index = index
        self._forward = forward
        self._system_dns = system_dns
        self._clock = clock
        self._log: Logger = logger or _NullLogger()
        self._rules = rules
        self._default_ttl = max(1, int(default_ttl))
        self._sem = asyncio.Semaphore(max(1, int(concurrency)))

    # ------------------------------------------------------------ 静态覆盖

    def static_map(self, env_name: str | None = None) -> dict[str, tuple[str, str]]:
        """该环境的全部静态覆盖：``{host: (ip, 来源)}``。

        优先级：**环境自带的 hosts > 环境绑定的规则文件**。两者都属于 static
        （优先于任何 DNS 结果，且永不过期）—— 环境里的条目是"这个环境特有的例外"，
        比共用的规则文件更具体，所以它赢。

        来源字符串会一路带到解析结果里，Dashboard 因此能显示"这条是哪来的"。
        整批只构造一次，避免逐个 host 重复读规则文件。
        """
        env = self._registry.get(env_name or self._registry.active)
        static: dict[str, tuple[str, str]] = {
            host: (ip, "environment") for host, ip in env.hosts.items()
        }
        if env.rules_file and self._rules is not None:
            origin = f"rules:{env.rules_file}"
            for host, ip in self._rules.entries(env.rules_file).items():
                static.setdefault(host, (ip, origin))
        return static

    def static_lookup(self, host: str, env_name: str | None = None) -> tuple[str, str] | None:
        """查单个 host 的静态覆盖，返回 ``(ip, 来源)``；没有则 ``None``。"""
        return self.static_map(env_name).get(normalize_host(host))

    # ------------------------------------------------------------ 服务器选择

    def report_servers(self, env_name: str | None = None) -> dict[str, Any]:
        """该环境实际会用哪套 DNS —— Dashboard 的"动态读取 DNS 服务器"视图。"""
        name = env_name or self._registry.active
        env = self._registry.get(name)
        configured = list(env.dns_servers)
        return {
            "env": name,
            "configured": configured,
            "source": "environment" if configured else "system",
            "system": list(self._system_dns.servers()),
            "effective": configured or list(self._system_dns.servers()),
        }

    # ------------------------------------------------------------ 正向解析

    async def resolve_hosts(
        self,
        hosts: Iterable[str],
        *,
        env_name: str | None = None,
        ttl: int | None = None,
        use_static: bool = True,
    ) -> list[dict[str, Any]]:
        """把一组域名解析成该环境的 IP，并写入映射索引。

        传给 :class:`ForwardResolver` 的是该环境**自己声明的** ``dns_servers``。
        空元组是显式语义："跟随操作系统 DNS"，由 infra 层选择系统解析器
        （并启用 hosts 文件）。这里不做"提前展开成系统 DNS 列表"，
        以免丢掉 hosts 文件这条路径。
        """
        env = self._registry.get(env_name or self._registry.active)
        servers = tuple(env.dns_servers)
        effective_ttl = self._default_ttl if ttl is None else max(1, int(ttl))
        static = self.static_map(env.name) if use_static else {}

        wanted: list[str] = []
        results: list[dict[str, Any]] = []
        for raw in hosts:
            host = normalize_host(raw)
            if not host:
                continue
            hit = static.get(host)
            if hit is not None:
                ip, origin = hit
                self._index.record(env.name, host, ip, SOURCE_STATIC, INFINITE_TTL)
                results.append(
                    {
                        "host": host,
                        "env": env.name,
                        "ips": [ip],
                        "source": SOURCE_STATIC,
                        "static_from": origin,
                        "rcode": "noerror",
                    }
                )
                continue
            wanted.append(host)

        if wanted:
            lookups = await asyncio.gather(
                *(self._forward_one(h, servers) for h in wanted)
            )
            for result in lookups:
                if result.ips:
                    self._index.record_many(
                        env.name, result.host, result.ips, SOURCE_ACTIVE, effective_ttl
                    )
                results.append(
                    {
                        "host": result.host,
                        "env": env.name,
                        "ips": list(result.ips),
                        "source": SOURCE_ACTIVE,
                        "rcode": result.rcode,
                        "error": result.error,
                    }
                )
        results.sort(key=lambda r: r["host"])
        return results

    async def resolve_host(self, host: str, *, env_name: str | None = None) -> dict[str, Any]:
        rows = await self.resolve_hosts([host], env_name=env_name)
        if not rows:
            raise DnsError(f"invalid host {host!r}", field="host")
        return rows[0]

    # ------------------------------------------------------------ 被动采集

    def record_passive(
        self,
        host: str,
        ips: Sequence[str],
        *,
        env_name: str | None = None,
        ttl: int | None = None,
    ) -> int:
        """记录一次被动观测（来自 mitmproxy 的 DNS 流量）。"""
        if not ips:
            return 0
        env = self._registry.get(env_name or self._registry.active)
        effective_ttl = self._default_ttl if ttl is None else max(1, int(ttl))
        self._index.record_many(
            env.name, normalize_host(host), ips, SOURCE_PASSIVE, effective_ttl
        )
        return len(ips)

    # ------------------------------------------------------------ 注解

    def annotate(self, host: str, ip: str = "") -> dict[str, Any] | None:
        """为 flow 找 host 的环境归属。

        ``ip`` 是 flow 实际连到的服务端地址。若某环境的解析结果包含该 IP，
        这条匹配标为 ``confirmed``；否则只说明"该 host 在该环境有已知解析"。
        这样既能防止张冠李戴，又不会因为 CDN 边缘 IP 与解析结果不一致而漏标。
        """
        host = normalize_host(host)
        if not host:
            return None
        per_env = self._index.envs_for_host(host)
        if not per_env:
            return None

        matches: list[dict[str, Any]] = []
        for env_name, ips in per_env.items():
            entries = self._index.by_host(env_name, host)
            matches.append(
                {
                    "env": env_name,
                    "ips": ips,
                    "source": entries[0].source if entries else "",
                    "confirmed": bool(ip) and ip in ips,
                }
            )

        confirmed = [m for m in matches if m["confirmed"]]
        pool = confirmed or matches
        active = self._registry.active
        chosen = next((m for m in pool if m["env"] == active), pool[0])
        return {"active": chosen, "matches": matches, "confirmed": bool(confirmed)}

    # ------------------------------------------------------------ 维护

    async def refresh(self, hosts: Iterable[str] | None = None) -> list[dict[str, Any]]:
        return await self.resolve_hosts(list(hosts or []))

    async def _guarded(self, coro):
        """在并发闸门内执行单个查询；异常降级为 ``None``，由调用方转成错误结果。"""
        async with self._sem:
            try:
                return await coro
            except asyncio.CancelledError:
                raise
            except Exception as exc:  # noqa: BLE001 - 单个查询失败不得拖垮整批
                self._log.warning(f"envboard: lookup failed: {exc!r}")
                return None

    async def _forward_one(self, host: str, servers: Sequence[str]) -> LookupResult:
        result = await self._guarded(self._forward.lookup(host, servers))
        if result is None:
            return LookupResult(host=host, rcode=RCODE_ERROR, error="lookup failed")
        return result
