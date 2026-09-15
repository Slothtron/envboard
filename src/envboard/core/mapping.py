"""host -> ip 映射索引（按环境分片，带 TTL 与来源标记）。

**只有正向**。全环境共用同一个域名时，本索引用"环境"这一维来区分它们各自解析到哪个 IP。
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass
from typing import Any

from .model import INFINITE_TTL
from .model import normalize_host
from .ports import Clock

SOURCE_STATIC = "static"
"""来自环境定义里的 hosts 覆盖，永不过期。"""

SOURCE_PASSIVE = "passive"
"""被动观测：mitmproxy 以 --mode dns 运行时从 DNS 响应流里采集。"""

SOURCE_ACTIVE = "active"
"""主动解析：envboard 向该环境的 DNS 服务器发起查询。"""

SOURCES = (SOURCE_STATIC, SOURCE_PASSIVE, SOURCE_ACTIVE)

#: 静态配置的优先级最高，被动观测次之，主动解析再次。
_PRIORITY = {SOURCE_STATIC: 0, SOURCE_PASSIVE: 1, SOURCE_ACTIVE: 2}


@dataclass
class Mapping:
    env: str
    host: str
    ip: str
    source: str
    resolved_at: float
    ttl: int = INFINITE_TTL

    @property
    def expires_at(self) -> float | None:
        if self.ttl == INFINITE_TTL:
            return None
        return self.resolved_at + self.ttl

    def expired(self, now: float) -> bool:
        if self.ttl == INFINITE_TTL:
            return False
        return now >= self.resolved_at + self.ttl

    def to_json(self, now: float) -> dict[str, Any]:
        return {
            "env": self.env,
            "host": self.host,
            "ip": self.ip,
            "source": self.source,
            "resolved_at": self.resolved_at,
            "ttl": self.ttl,
            "expires_at": self.expires_at,
            "age": round(max(0.0, now - self.resolved_at), 3),
        }


class MappingIndex:
    """按环境分片的 ``host -> {ip: Mapping}`` 表。

    同一个 host 在多套环境里可以有各自的 IP —— 这正是本插件存在的意义。
    """

    def __init__(self, clock: Clock, max_entries: int = 50_000) -> None:
        self._clock = clock
        self._max_entries = max_entries
        self._fwd: dict[tuple[str, str], dict[str, Mapping]] = {}
        self._hits = 0
        self._misses = 0

    # -------------------------------------------------------------- 写入

    def record(
        self,
        env: str,
        host: str,
        ip: str,
        source: str,
        ttl: int = INFINITE_TTL,
    ) -> Mapping:
        now = self._clock.now()
        host = normalize_host(host)
        entry = Mapping(env=env, host=host, ip=ip, source=source, resolved_at=now, ttl=ttl)
        bucket = self._fwd.setdefault((env, host), {})
        existing = bucket.get(ip)
        if existing is not None:
            if _PRIORITY[existing.source] < _PRIORITY[source]:
                # 已有更权威的来源：既不覆盖，也不刷新它的 TTL。
                # （否则一次低优先级的观测会把永不过期的 static 记录变成会过期。）
                return existing
            if _PRIORITY[existing.source] == _PRIORITY[source]:
                existing.resolved_at = now
                existing.ttl = ttl
                return existing
            # 落到下面：新来源更权威，替换旧条目。
        bucket[ip] = entry
        self._enforce_limit()
        return entry

    def record_many(
        self,
        env: str,
        host: str,
        ips: Iterable[str],
        source: str,
        ttl: int = INFINITE_TTL,
    ) -> list[Mapping]:
        return [self.record(env, host, ip, source, ttl) for ip in ips]

    def forget_host(self, env: str, host: str) -> None:
        self._fwd.pop((env, normalize_host(host)), None)

    def forget_env(self, env: str) -> None:
        for key in [k for k in self._fwd if k[0] == env]:
            del self._fwd[key]

    # -------------------------------------------------------------- 读取

    def by_host(self, env: str, host: str) -> list[Mapping]:
        """返回某环境里该 host 当前有效的全部 IP 记录。"""
        now = self._clock.now()
        entries = list(self._fwd.get((env, normalize_host(host)), {}).values())
        live = [e for e in entries if not e.expired(now)]
        if live:
            self._hits += 1
        else:
            self._misses += 1
        return live

    def envs_for_host(self, host: str) -> dict[str, list[str]]:
        """跨环境正查：这个 host 在每套环境里分别解析到哪些 IP。"""
        host = normalize_host(host)
        now = self._clock.now()
        out: dict[str, list[str]] = {}
        for (env, entry_host), bucket in self._fwd.items():
            if entry_host != host:
                continue
            ips = sorted(ip for ip, e in bucket.items() if not e.expired(now))
            if ips:
                out[env] = ips
        return out

    def snapshot(
        self,
        *,
        env: str | None = None,
        query: str = "",
        source: str | None = None,
        limit: int = 500,
    ) -> list[dict[str, Any]]:
        now = self._clock.now()
        needle = query.strip().lower()
        rows: list[dict[str, Any]] = []
        for (entry_env, host), bucket in self._fwd.items():
            if env and entry_env != env:
                continue
            if needle and needle not in host:
                continue
            for entry in bucket.values():
                if entry.expired(now):
                    continue
                if source and entry.source != source:
                    continue
                rows.append(entry.to_json(now))
        rows.sort(key=lambda r: (r["env"], r["host"], r["ip"]))
        return rows[: max(1, limit)]

    # -------------------------------------------------------------- 维护

    def prune(self) -> int:
        now = self._clock.now()
        removed = 0
        for key in list(self._fwd):
            bucket = self._fwd[key]
            for ip in [ip for ip, e in bucket.items() if e.expired(now)]:
                del bucket[ip]
                removed += 1
            if not bucket:
                del self._fwd[key]
        return removed

    def clear(self) -> None:
        self._fwd.clear()

    def stats(self) -> dict[str, Any]:
        now = self._clock.now()
        live = sum(
            1
            for bucket in self._fwd.values()
            for entry in bucket.values()
            if not entry.expired(now)
        )
        return {
            "hosts_per_env": len(self._fwd),
            "entries": live,
            "hits": self._hits,
            "misses": self._misses,
            "max_entries": self._max_entries,
        }

    def _enforce_limit(self) -> None:
        if len(self._fwd) <= self._max_entries:
            return
        self.prune()
        while len(self._fwd) > self._max_entries:
            oldest = min(
                self._fwd.items(),
                key=lambda kv: max(e.resolved_at for e in kv[1].values()),
            )
            del self._fwd[oldest[0]]
