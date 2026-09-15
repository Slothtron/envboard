from __future__ import annotations

from collections.abc import Sequence


class MitmproxyRsSystemDns:
    """通过 ``mitmproxy_rs.dns.get_system_dns_servers()`` 动态读取操作系统 DNS。

    这是"动态读取 DNS 服务器"的第一来源；失败时返回空列表，由上层决定回退策略。
    """

    def __init__(self) -> None:
        self._cached: list[str] | None = None

    def servers(self) -> Sequence[str]:
        if self._cached is None:
            self._cached = self._probe()
        return list(self._cached)

    def refresh(self) -> Sequence[str]:
        self._cached = None
        return self.servers()

    @staticmethod
    def _probe() -> list[str]:
        try:
            import mitmproxy_rs
        except ImportError:  # pragma: no cover - 仅在缺依赖时触发
            return []
        try:
            return [str(s) for s in (mitmproxy_rs.dns.get_system_dns_servers() or [])]
        except Exception:  # noqa: BLE001 - 宿主 API 抛错时不得影响插件加载
            return []


class StaticSystemDns:
    """测试替身。"""

    def __init__(self, servers: Sequence[str]) -> None:
        self._servers = [str(s) for s in servers]

    def servers(self) -> Sequence[str]:
        return list(self._servers)

    def refresh(self) -> Sequence[str]:
        return self.servers()
