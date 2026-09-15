"""正向解析（host -> ip）：包装 ``mitmproxy_rs.dns.DnsResolver``。

为什么用它而不是自己写 DNS 客户端：

* 它是 Rust 实现（hickory-dns），可显式指定 ``name_servers``，天然支持"每环境一套 DNS"。
* ``mitmproxy_rs.dns.DnsResolver`` 只接受**裸 IP 字面量**（``ip:port`` / 主机名会被
  ``ValueError: invalid IP address syntax`` 拒绝）—— 这是本设计明确记录的约束。
* 它**没有** PTR 接口，但本插件只做正向解析，不需要 PTR。
"""

from __future__ import annotations

import asyncio
import socket
from collections.abc import Sequence

from ..core.ports import LookupResult
from ..core.ports import RCODE_ERROR
from ..core.ports import RCODE_NODATA
from ..core.ports import RCODE_NXDOMAIN
from ..core.ports import RCODE_SERVFAIL
from ..core.ports import RCODE_TIMEOUT
from ..core.ports import RCODE_UNSUPPORTED


class MitmproxyRsForwardResolver:
    def __init__(
        self,
        *,
        timeout: float = 5.0,
        use_hosts_file: bool = False,
        cache_size: int = 64,
    ) -> None:
        self._timeout = float(timeout)
        self._use_hosts_file = bool(use_hosts_file)
        self._cache_size = max(1, int(cache_size))
        self._cache: dict[tuple[str, ...], object] = {}
        self._system: object | None = None

    async def lookup(self, host: str, dns_servers: Sequence[str]) -> LookupResult:
        try:
            import mitmproxy_rs  # noqa: F401
        except ImportError:  # pragma: no cover
            return LookupResult(
                host, rcode=RCODE_UNSUPPORTED, error="mitmproxy_rs is not installed"
            )

        try:
            resolver = self._resolver_for(tuple(dns_servers))
        except ValueError as exc:
            return LookupResult(host, rcode=RCODE_ERROR, error=f"invalid dns server: {exc}")

        try:
            ips = await asyncio.wait_for(resolver.lookup_ip(host), self._timeout)
        except asyncio.TimeoutError:
            return LookupResult(host, rcode=RCODE_TIMEOUT, error="lookup timed out")
        except socket.gaierror as exc:
            return LookupResult(host, rcode=_rcode_for(exc), error=_describe(exc))
        except ValueError as exc:
            return LookupResult(host, rcode=RCODE_ERROR, error=str(exc))
        except Exception as exc:  # noqa: BLE001 - 任何解析异常都降级为结果，不抛出
            return LookupResult(host, rcode=RCODE_SERVFAIL, error=repr(exc))

        addresses = [str(ip) for ip in (ips or [])]
        if not addresses:
            return LookupResult(host, rcode=RCODE_NODATA)
        return LookupResult(host, ips=tuple(addresses))

    def _resolver_for(self, servers: tuple[str, ...]):
        import mitmproxy_rs

        if not servers:
            if self._system is None:
                self._system = mitmproxy_rs.dns.DnsResolver(use_hosts_file=True)
            return self._system

        cached = self._cache.get(servers)
        if cached is None:
            cached = mitmproxy_rs.dns.DnsResolver(
                name_servers=list(servers), use_hosts_file=self._use_hosts_file
            )
            if len(self._cache) >= self._cache_size:
                self._cache.pop(next(iter(self._cache)))
            self._cache[servers] = cached
        return cached

    def clear(self) -> None:
        self._cache.clear()
        self._system = None


def _rcode_for(exc: socket.gaierror) -> str:
    code = exc.args[0] if exc.args else None
    if code == socket.EAI_NONAME:
        return RCODE_NXDOMAIN
    if code == socket.EAI_NODATA:
        return RCODE_NODATA
    if code == socket.EAI_AGAIN:
        return RCODE_TIMEOUT
    return RCODE_SERVFAIL


def _describe(exc: socket.gaierror) -> str:
    code = exc.args[0] if exc.args else None
    name = {
        socket.EAI_NONAME: "EAI_NONAME",
        socket.EAI_NODATA: "EAI_NODATA",
        socket.EAI_AGAIN: "EAI_AGAIN",
        socket.EAI_FAIL: "EAI_FAIL",
    }.get(code, str(code))
    return f"{name}: {exc}"
