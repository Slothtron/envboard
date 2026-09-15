"""core + infra 单元测试（仅标准库，不依赖 mitmproxy，可用任意 Python 3.12 跑）。"""

from __future__ import annotations

import os
import stat
import tempfile
import unittest
from collections.abc import Sequence

from envboard.core.errors import ConflictError
from envboard.core.errors import InvalidConfig
from envboard.core.errors import NotFoundError
from envboard.core.mapping import MappingIndex
from envboard.core.mapping import SOURCE_ACTIVE
from envboard.core.mapping import SOURCE_PASSIVE
from envboard.core.mapping import SOURCE_STATIC
from envboard.core.model import Environment
from envboard.core.ports import LookupResult
from envboard.core.ports import RCODE_NXDOMAIN
from envboard.core.registry import EnvRegistry
from envboard.core.resolver import ResolverService
from envboard.infra.clock import FrozenClock
from envboard.infra.store import InMemoryStore
from envboard.infra.store import JsonFileStore

# --------------------------------------------------------------------- 替身


class StubForward:
    """按 (dns_servers, host) 返回不同结果 —— 用来模拟"同一域名在不同环境解析到不同 IP"。"""

    def __init__(self, table: dict[str, list[str]] | None = None) -> None:
        self.table = table or {}
        self.calls: list[tuple[str, tuple[str, ...]]] = []

    async def lookup(self, host: str, dns_servers: Sequence[str]) -> LookupResult:
        self.calls.append((host, tuple(dns_servers)))
        ips = self.table.get(host)
        if ips is None:
            return LookupResult(host, rcode=RCODE_NXDOMAIN)
        return LookupResult(host, ips=tuple(ips))


class StubSystemDns:
    def __init__(self, servers: Sequence[str]) -> None:
        self._servers = list(servers)

    def servers(self) -> Sequence[str]:
        return list(self._servers)


def make_service(
    *,
    clock: FrozenClock | None = None,
    forward: StubForward | None = None,
    system: Sequence[str] = ("9.9.9.9",),
    store: InMemoryStore | None = None,
    default_ttl: int = 300,
) -> tuple[ResolverService, EnvRegistry, MappingIndex, StubForward]:
    clock = clock or FrozenClock()
    store = store or InMemoryStore()
    registry = EnvRegistry(store, clock)
    registry.load()
    index = MappingIndex(clock)
    fwd = forward or StubForward({})
    service = ResolverService(
        registry, index, fwd, StubSystemDns(system), clock, default_ttl=default_ttl
    )
    return service, registry, index, fwd


# --------------------------------------------------------------------- 模型


class EnvironmentModelTest(unittest.TestCase):
    def test_accepts_bare_ipv6_and_normalises_name(self) -> None:
        env = Environment.from_json({"name": "Prod", "dns_servers": ["::1"]})
        self.assertEqual(env.name, "prod")
        self.assertEqual(env.dns_servers, ("::1",))

    def test_rejects_ip_with_port(self) -> None:
        with self.assertRaises(InvalidConfig) as ctx:
            Environment.from_json({"name": "prod", "dns_servers": ["10.0.0.1:53"]})
        self.assertEqual(ctx.exception.field, "environment.dns_servers[0]")
        self.assertIn("bare IP", ctx.exception.message)

    def test_rejects_duplicate_servers(self) -> None:
        with self.assertRaises(InvalidConfig) as ctx:
            Environment.from_json({"name": "prod", "dns_servers": ["10.0.0.1", "10.0.0.1"]})
        self.assertEqual(ctx.exception.field, "environment.dns_servers[1]")

    def test_rejects_unknown_field(self) -> None:
        with self.assertRaises(InvalidConfig) as ctx:
            Environment.from_json({"name": "prod", "nope": 1})
        self.assertEqual(ctx.exception.field, "environment.nope")

    def test_rejects_bad_host_value(self) -> None:
        with self.assertRaises(InvalidConfig) as ctx:
            Environment.from_json({"name": "prod", "hosts": {"a.example.com": "nope"}})
        self.assertEqual(ctx.exception.field, "environment.hosts['a.example.com']")

    def test_merged_keeps_untouched_fields(self) -> None:
        base = Environment.from_json(
            {"name": "gray", "dns_servers": ["10.1.0.1"], "domain_suffix": "gray.example.com"}
        )
        merged = base.merged({"dns_servers": ["10.2.0.1"]})
        self.assertEqual(merged.dns_servers, ("10.2.0.1",))
        self.assertEqual(merged.domain_suffix, "gray.example.com")
        self.assertEqual(base.dns_servers, ("10.1.0.1",))  # 原对象不变


# ------------------------------------------------------------------ 注册表


class RegistryTest(unittest.TestCase):
    def test_bootstraps_default_env(self) -> None:
        store = InMemoryStore()
        registry = EnvRegistry(store, FrozenClock())
        registry.load()
        self.assertEqual(registry.names(), ["local"])
        self.assertEqual(registry.active, "local")
        self.assertEqual(store.writes, 1)

    def test_create_update_rename_delete(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        registry.create({"name": "prod", "dns_servers": ["10.0.0.53"]})
        self.assertEqual(sorted(registry.names()), ["local", "prod"])

        renamed = registry.update("prod", {"name": "prod-2", "domain_suffix": "p.example.com"})
        self.assertEqual(renamed.name, "prod-2")
        self.assertNotIn("prod", registry.names())
        self.assertEqual(registry.get("prod-2").dns_servers, ("10.0.0.53",))

        registry.activate("prod-2")
        with self.assertRaises(ConflictError):
            registry.delete("prod-2")  # 不能删激活环境

        registry.activate("local")
        registry.delete("prod-2")
        self.assertEqual(registry.names(), ["local"])

    def test_rename_active_env_follows(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        registry.create({"name": "beta"})
        registry.activate("beta")
        registry.update("beta", {"name": "beta-2"})
        self.assertEqual(registry.active, "beta-2")

    def test_refuses_duplicate_and_unknown(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        registry.create({"name": "beta"})
        with self.assertRaises(ConflictError):
            registry.create({"name": "beta"})
        with self.assertRaises(NotFoundError):
            registry.get("nope")

    def test_refuses_deleting_last_env(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        with self.assertRaises(ConflictError):
            registry.delete("local")

    def test_observers_fire(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        seen: list[str] = []
        registry.on_change(seen.append)
        registry.create({"name": "beta"})
        registry.activate("beta")
        self.assertEqual(seen, ["environments", "active"])

    def test_activate_is_idempotent(self) -> None:
        registry = EnvRegistry(InMemoryStore(), FrozenClock())
        registry.load()
        seen: list[str] = []
        registry.on_change(seen.append)
        registry.activate("local")
        self.assertEqual(seen, [])

    def test_roundtrip_through_store(self) -> None:
        store = InMemoryStore()
        first = EnvRegistry(store, FrozenClock())
        first.load()
        first.create({"name": "prod", "dns_servers": ["10.0.0.53"]})
        first.activate("prod")

        second = EnvRegistry(store, FrozenClock())
        second.load()
        self.assertEqual(second.active, "prod")
        self.assertEqual(second.get("prod").dns_servers, ("10.0.0.53",))

    def test_state_version_is_enforced(self) -> None:
        store = InMemoryStore({"version": 99, "environments": []})
        registry = EnvRegistry(store, FrozenClock())
        with self.assertRaises(InvalidConfig):
            registry.load()


# ------------------------------------------------------------------ 映射索引


class MappingIndexTest(unittest.TestCase):
    def test_forward_and_ttl(self) -> None:
        clock = FrozenClock()
        index = MappingIndex(clock)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)

        self.assertEqual([m.ip for m in index.by_host("prod", "api.example.com")], ["10.0.0.1"])
        self.assertEqual(index.envs_for_host("api.example.com"), {"prod": ["10.0.0.1"]})

        clock.advance(61)
        self.assertEqual(index.by_host("prod", "api.example.com"), [])
        self.assertEqual(index.envs_for_host("api.example.com"), {})

    def test_same_host_differs_per_environment(self) -> None:
        """本插件存在的意义：同一域名在不同环境解析到不同 IP。"""
        index = MappingIndex(FrozenClock())
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        index.record("beta", "api.example.com", "10.9.0.1", SOURCE_ACTIVE, ttl=60)

        self.assertEqual(
            index.envs_for_host("api.example.com"),
            {"prod": ["10.0.0.1"], "beta": ["10.9.0.1"]},
        )
        self.assertEqual([m.ip for m in index.by_host("beta", "api.example.com")], ["10.9.0.1"])

    def test_host_normalisation(self) -> None:
        index = MappingIndex(FrozenClock())
        index.record("prod", "API.Example.COM.", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        self.assertEqual(len(index.by_host("prod", "api.example.com")), 1)

    def test_static_outranks_active_and_never_expires(self) -> None:
        clock = FrozenClock()
        index = MappingIndex(clock)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_STATIC)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=10)

        entry = index.by_host("prod", "api.example.com")[0]
        self.assertEqual(entry.source, SOURCE_STATIC)
        self.assertEqual(entry.ttl, 0)

        clock.advance(10_000)
        self.assertEqual(len(index.by_host("prod", "api.example.com")), 1)

    def test_higher_priority_source_replaces_lower(self) -> None:
        index = MappingIndex(FrozenClock())
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_PASSIVE, ttl=10)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_STATIC)
        self.assertEqual(index.by_host("prod", "api.example.com")[0].source, SOURCE_STATIC)

    def test_same_source_refreshes_timestamp(self) -> None:
        clock = FrozenClock()
        index = MappingIndex(clock)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        clock.advance(50)
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        clock.advance(20)  # 距首次 70s，距刷新 20s
        self.assertEqual(len(index.by_host("prod", "api.example.com")), 1)

    def test_snapshot_filters(self) -> None:
        index = MappingIndex(FrozenClock())
        index.record("prod", "a.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        index.record("beta", "b.example.com", "10.1.0.1", SOURCE_ACTIVE, ttl=60)

        self.assertEqual(len(index.snapshot(env="prod")), 1)
        self.assertEqual(len(index.snapshot()), 2)
        self.assertEqual(len(index.snapshot(query="b.example")), 1)
        self.assertEqual(index.snapshot(query="b.example")[0]["env"], "beta")
        self.assertEqual(len(index.snapshot(source=SOURCE_PASSIVE)), 0)

    def test_prune_removes_expired_hosts(self) -> None:
        clock = FrozenClock()
        index = MappingIndex(clock)
        index.record("prod", "a.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=5)
        index.record("prod", "b.example.com", "10.0.0.2", SOURCE_STATIC)
        clock.advance(6)
        self.assertEqual(index.prune(), 1)
        self.assertEqual(index.stats()["entries"], 1)

    def test_forget_env(self) -> None:
        index = MappingIndex(FrozenClock())
        index.record("prod", "a.example.com", "10.0.0.1", SOURCE_STATIC)
        index.record("beta", "a.example.com", "10.0.0.1", SOURCE_STATIC)
        index.forget_env("prod")
        self.assertEqual(index.by_host("prod", "a.example.com"), [])
        self.assertEqual(len(index.by_host("beta", "a.example.com")), 1)


# ---------------------------------------------------------------- 解析编排


class ResolverServiceTest(unittest.IsolatedAsyncioTestCase):
    async def test_static_override_skips_dns(self) -> None:
        forward = StubForward({"api.example.com": ["10.0.0.9"]})
        service, registry, index, fwd = make_service(forward=forward)
        registry.update("local", {"hosts": {"api.example.com": "10.0.0.11"}})

        rows = await service.resolve_hosts(["api.example.com"])
        self.assertEqual(rows[0]["ips"], ["10.0.0.11"])
        self.assertEqual(rows[0]["source"], SOURCE_STATIC)
        self.assertEqual(fwd.calls, [])  # 静态覆盖命中就不查 DNS

    async def test_forward_uses_env_dns_servers(self) -> None:
        forward = StubForward({"api.example.com": ["10.2.0.11", "10.2.0.12"]})
        service, registry, index, fwd = make_service(forward=forward)
        registry.create({"name": "prod", "dns_servers": ["10.2.0.53"]})

        rows = await service.resolve_hosts(["api.example.com"], env_name="prod")
        self.assertEqual(sorted(rows[0]["ips"]), ["10.2.0.11", "10.2.0.12"])
        self.assertEqual(fwd.calls, [("api.example.com", ("10.2.0.53",))])

        # 空 dns_servers 是显式语义："跟随操作系统 DNS"，
        # 以空元组交给 infra 层选择系统解析器（保留 hosts 文件路径）。
        service2, _, _, fwd2 = make_service(forward=StubForward({}), system=("9.9.9.9",))
        await service2.resolve_hosts(["x.example.com"])
        self.assertEqual(fwd2.calls, [("x.example.com", ())])

    async def test_nxdomain_is_reported_not_raised(self) -> None:
        service, _, index, _ = make_service(forward=StubForward({}))
        rows = await service.resolve_hosts(["missing.example.com"])
        self.assertEqual(rows[0]["ips"], [])
        self.assertEqual(rows[0]["rcode"], "nxdomain")
        self.assertEqual(index.snapshot(), [])

    async def test_passive_capture(self) -> None:
        service, registry, index, _ = make_service()
        recorded = service.record_passive("api.example.com", ["10.0.0.1"])
        self.assertEqual(recorded, 1)
        self.assertEqual(
            index.by_host(registry.active, "api.example.com")[0].source, SOURCE_PASSIVE
        )

    async def test_annotate_confirmed_by_server_ip(self) -> None:
        service, registry, index, _ = make_service()
        registry.create({"name": "prod"})
        index.record("prod", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)

        info = service.annotate("api.example.com", "10.0.0.1")
        self.assertIsNotNone(info)
        self.assertTrue(info["confirmed"])
        self.assertEqual(info["active"]["env"], "prod")

    async def test_annotate_unconfirmed_when_ip_differs(self) -> None:
        """flow 连的是 CDN 边缘 IP，与解析结果不一致 —— 仍应标注，但标 unconfirmed。"""
        service, _, index, _ = make_service()
        index.record("local", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)

        info = service.annotate("api.example.com", "203.0.113.9")
        self.assertIsNotNone(info)
        self.assertFalse(info["confirmed"])
        self.assertEqual(info["active"]["env"], "local")

    async def test_annotate_prefers_confirmed_then_active(self) -> None:
        service, registry, index, _ = make_service()
        registry.create({"name": "prod"})
        index.record("local", "api.example.com", "10.0.0.1", SOURCE_ACTIVE, ttl=60)
        index.record("prod", "api.example.com", "10.0.0.2", SOURCE_ACTIVE, ttl=60)

        # 激活的是 local（默认），但实际连到 prod 的 IP → 应选 prod
        info = service.annotate("api.example.com", "10.0.0.2")
        self.assertEqual(info["active"]["env"], "prod")

        # 没有 IP 线索时回落到激活环境
        info = service.annotate("api.example.com")
        self.assertEqual(info["active"]["env"], "local")

    async def test_annotate_returns_none_when_unknown(self) -> None:
        service, _, _, _ = make_service()
        self.assertIsNone(service.annotate("nope.example.com"))

    async def test_resolve_across_all_envs(self) -> None:
        """多环境对比：同一域名分别向每套环境的 DNS 服务器查一次。"""
        forward = StubForward({"api.example.com": ["10.9.0.1"]})
        service, registry, index, fwd = make_service(forward=forward)
        registry.create({"name": "prod", "dns_servers": ["10.0.0.53"]})
        registry.create({"name": "beta", "dns_servers": ["10.9.0.53"]})

        groups: dict[str, list[dict]] = {}
        for name in registry.names():
            groups[name] = await service.resolve_hosts(["api.example.com"], env_name=name)

        self.assertEqual(
            set(fwd.calls),
            {
                ("api.example.com", ("10.0.0.53",)),
                ("api.example.com", ("10.9.0.53",)),
                ("api.example.com", ()),  # local 跟随系统 DNS
            },
        )
        self.assertEqual(set(groups), {"local", "prod", "beta"})
        self.assertEqual(index.envs_for_host("api.example.com")["prod"], ["10.9.0.1"])

    async def test_report_servers_source(self) -> None:
        service, registry, _, _ = make_service(system=("9.9.9.9",))
        report = service.report_servers()
        self.assertEqual(report["source"], "system")
        self.assertEqual(report["effective"], ["9.9.9.9"])

        registry.create({"name": "prod", "dns_servers": ["10.0.0.53"]})
        report = service.report_servers("prod")
        self.assertEqual(report["source"], "environment")
        self.assertEqual(report["effective"], ["10.0.0.53"])

    async def test_failing_lookup_does_not_abort_batch(self) -> None:
        class Exploding:
            async def lookup(self, host, dns_servers):  # noqa: ANN001, ANN202
                raise RuntimeError("boom")

        service, _, _, _ = make_service()
        service._forward = Exploding()
        rows = await service.resolve_hosts(["a.example.com", "b.example.com"])
        self.assertEqual(len(rows), 2)
        self.assertTrue(all(r["rcode"] == "error" for r in rows))


# -------------------------------------------------------------------- 存储


class StoreTest(unittest.TestCase):
    def test_json_roundtrip_and_permissions(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "nested", "envboard.json")
            store = JsonFileStore(path)
            self.assertIsNone(store.read())
            store.write({"version": 1, "active": "local", "environments": []})
            self.assertEqual(store.read()["active"], "local")
            mode = stat.S_IMODE(os.stat(path).st_mode)
            self.assertEqual(mode, 0o600, f"expected 0600, got {oct(mode)}")

    def test_rejects_non_object(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "envboard.json")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write("[1,2,3]")
            with self.assertRaises(Exception):
                JsonFileStore(path).read()


if __name__ == "__main__":
    unittest.main()
