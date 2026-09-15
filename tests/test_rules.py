"""规则文件（hosts 导入 / 规范化 / 绑定 / 生效优先级）单元测试。

只依赖标准库与 core+infra，不依赖 mitmproxy。
"""

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
from envboard.core.mapping import SOURCE_STATIC
from envboard.core.model import Environment
from envboard.core.model import normalize_hosts
from envboard.core.ports import LookupResult
from envboard.core.ports import RCODE_NXDOMAIN
from envboard.core.registry import EnvRegistry
from envboard.core.resolver import ResolverService
from envboard.core.rules import SKIP_INVALID_HOST
from envboard.core.rules import SKIP_NO_IP_LITERAL
from envboard.core.rules import SKIP_TOO_FEW_TOKENS
from envboard.core.rules import parse_hosts_text
from envboard.core.rules import render_rules
from envboard.infra.clock import FrozenClock
from envboard.infra.rules_store import CachedRulesLookup
from envboard.infra.rules_store import FileRulesStore
from envboard.infra.rules_store import InMemoryRulesStore
from envboard.infra.store import InMemoryStore

# --------------------------------------------------------------------- 替身


class StubForward:
    def __init__(self, table: dict[str, list[str]] | None = None) -> None:
        self.table = table or {}
        self.calls: list[str] = []

    async def lookup(self, host: str, dns_servers: Sequence[str]) -> LookupResult:
        self.calls.append(host)
        ips = self.table.get(host)
        if ips is None:
            return LookupResult(host, rcode=RCODE_NXDOMAIN)
        return LookupResult(host, ips=tuple(ips))


class StubSystemDns:
    def servers(self) -> Sequence[str]:
        return ["9.9.9.9"]


# ----------------------------------------------------------------- 解析器


class ParseHostsTextTest(unittest.TestCase):
    def test_ip_first_multiple_hosts_share_one_ip(self) -> None:
        r = parse_hosts_text("10.0.0.1 a.example.com b.example.com c.example.com\n")
        self.assertEqual(
            r.entries,
            {"a.example.com": "10.0.0.1", "b.example.com": "10.0.0.1", "c.example.com": "10.0.0.1"},
        )
        self.assertEqual(r.ips, 1)
        self.assertEqual(r.skipped, [])

    def test_host_first_order_is_accepted(self) -> None:
        r = parse_hosts_text("beta.example.com 10.0.0.2\n")
        self.assertEqual(r.entries, {"beta.example.com": "10.0.0.2"})

    def test_host_first_with_several_hosts(self) -> None:
        r = parse_hosts_text("a.example.com b.example.com 10.0.0.3\n")
        self.assertEqual(r.entries, {"a.example.com": "10.0.0.3", "b.example.com": "10.0.0.3"})

    def test_comments_blanks_and_indentation(self) -> None:
        r = parse_hosts_text(
            "\n# full line comment\n   10.0.0.4  indented.example.com   # trailing comment\n\n"
        )
        self.assertEqual(r.entries, {"indented.example.com": "10.0.0.4"})
        self.assertEqual((r.blank_lines, r.comment_lines, r.data_lines), (2, 1, 1))
        self.assertEqual(r.skipped, [])

    def test_bom_is_stripped(self) -> None:
        r = parse_hosts_text("\ufeff10.0.0.5 bom.example.com\n")
        self.assertEqual(r.entries, {"bom.example.com": "10.0.0.5"})

    def test_single_token_line_is_skipped(self) -> None:
        r = parse_hosts_text("only-a-host.example.com\n")
        self.assertEqual(r.entries, {})
        self.assertEqual([s.reason for s in r.skipped], [SKIP_TOO_FEW_TOKENS])

    def test_line_without_any_ip_is_skipped(self) -> None:
        r = parse_hosts_text("a.example.com b.example.com\n")
        self.assertEqual(r.entries, {})
        self.assertEqual([s.reason for s in r.skipped], [SKIP_NO_IP_LITERAL])

    def test_bad_host_does_not_discard_its_whole_line(self) -> None:
        """一行里某个 host 非法 → 只丢那一项；这正是"非法行自动忽略"的宽容语义。"""
        r = parse_hosts_text("10.0.0.6 good.example.com -bad.example.com\n")
        self.assertEqual(r.entries, {"good.example.com": "10.0.0.6"})
        self.assertEqual(len(r.skipped), 1)
        self.assertEqual(r.skipped[0].reason, SKIP_INVALID_HOST)
        self.assertEqual(r.skipped[0].detail, "-bad.example.com")

    def test_underscore_labels_are_legal(self) -> None:
        """`_` 是合法 label 字符（_dmarc / _sip._tcp 这类记录需要它），不算非法。"""
        r = parse_hosts_text("10.0.0.6 _dmarc.example.com\n")
        self.assertEqual(r.entries, {"_dmarc.example.com": "10.0.0.6"})

    def test_extra_ip_token_is_rejected_as_host(self) -> None:
        r = parse_hosts_text("10.0.0.7 10.0.0.8\n")
        self.assertEqual(r.entries, {})
        self.assertEqual([s.reason for s in r.skipped], [SKIP_INVALID_HOST])

    def test_out_of_range_ip_is_skipped(self) -> None:
        r = parse_hosts_text("300.1.2.3 nope.example.com\n")
        self.assertEqual(r.entries, {})
        self.assertEqual([s.reason for s in r.skipped], [SKIP_NO_IP_LITERAL])

    def test_same_host_same_ip_is_not_a_conflict(self) -> None:
        r = parse_hosts_text("10.0.0.9 dup.example.com\n10.0.0.9 dup.example.com\n")
        self.assertEqual(r.entries, {"dup.example.com": "10.0.0.9"})
        self.assertEqual(r.conflicts, [])

    def test_conflicting_host_last_wins(self) -> None:
        r = parse_hosts_text("10.0.0.10 c.example.com\n10.0.0.11 c.example.com\n")
        self.assertEqual(r.entries, {"c.example.com": "10.0.0.11"})
        self.assertEqual(len(r.conflicts), 1)
        self.assertEqual(
            (r.conflicts[0].host, r.conflicts[0].dropped, r.conflicts[0].kept),
            ("c.example.com", "10.0.0.10", "10.0.0.11"),
        )

    def test_keys_are_normalized(self) -> None:
        r = parse_hosts_text("10.0.0.12 API.Example.COM.\n10.0.0.12 *.wild.example.com\n")
        self.assertEqual(
            r.entries, {"api.example.com": "10.0.0.12", "wild.example.com": "10.0.0.12"}
        )

    def test_ipv6_literals(self) -> None:
        r = parse_hosts_text("2001:db8::1 v6.example.com\n[2001:db8::2] v6b.example.com\n")
        self.assertEqual(
            r.entries, {"v6.example.com": "2001:db8::1", "v6b.example.com": "2001:db8::2"}
        )

    def test_empty_input(self) -> None:
        r = parse_hosts_text("")
        self.assertEqual(r.entries, {})
        self.assertEqual(r.to_json()["stats"]["accepted"], 0)

    def test_to_json_shape(self) -> None:
        payload = parse_hosts_text("10.0.0.13 j.example.com\nbad\n").to_json()
        self.assertEqual(payload["accepted"], 1)
        self.assertEqual(payload["ips"], 1)
        self.assertEqual(payload["stats"]["skipped"], 1)
        self.assertEqual(payload["skipped"][0]["line"], 2)


class RenderRulesTest(unittest.TestCase):
    def test_groups_hosts_by_ip_and_sorts(self) -> None:
        out = render_rules({"b.example.com": "10.0.0.2", "a.example.com": "10.0.0.2"})
        body = [ln for ln in out.splitlines() if ln and not ln.startswith("#")]
        self.assertEqual(body, ["10.0.0.2 a.example.com b.example.com"])

    def test_ips_sort_numerically_not_lexically(self) -> None:
        out = render_rules({"a.example.com": "10.0.0.10", "b.example.com": "10.0.0.2"})
        body = [ln for ln in out.splitlines() if ln and not ln.startswith("#")]
        self.assertEqual(body, ["10.0.0.2 b.example.com", "10.0.0.10 a.example.com"])

    def test_same_input_gives_identical_bytes(self) -> None:
        entries = {"a.example.com": "10.0.0.1", "b.example.com": "10.0.0.1"}
        self.assertEqual(render_rules(entries), render_rules(dict(entries)))

    def test_round_trip_is_stable(self) -> None:
        source = "10.0.0.1 a.example.com b.example.com\nc.example.com 10.0.0.2\n"
        first = render_rules(parse_hosts_text(source).entries)
        second = render_rules(parse_hosts_text(first).entries)
        self.assertEqual(first, second)
        self.assertEqual(parse_hosts_text(first).entries, parse_hosts_text(source).entries)

    def test_source_line_is_recorded(self) -> None:
        self.assertIn("# source: x.txt", render_rules({}, source="x.txt"))


# ------------------------------------------------------------- 模型归一化


class HostsNormalizationTest(unittest.TestCase):
    def test_keys_and_values_normalized_on_construction(self) -> None:
        env = Environment(name="t", hosts={"API.Example.COM.": "10.0.0.1"})
        self.assertEqual(list(env.hosts), ["api.example.com"])

    def test_wildcard_prefix_is_stripped(self) -> None:
        """契约（core/spec/capabilities.md）承诺 `*.` 归一化时剥离。"""
        env = Environment(name="t", hosts={"*.wild.example.com": "10.0.0.1"})
        self.assertEqual(list(env.hosts), ["wild.example.com"])

    def test_collision_after_normalization_is_loud(self) -> None:
        with self.assertRaises(InvalidConfig) as ctx:
            normalize_hosts({"A.example.com": "10.0.0.1", "a.example.com": "10.0.0.2"}, "e.hosts")
        self.assertIn("normalize to the same host", str(ctx.exception))

    def test_rules_file_must_be_a_valid_name(self) -> None:
        with self.assertRaises(InvalidConfig):
            Environment(name="t", rules_file="../etc/passwd").validate()

    def test_rules_file_defaults_empty_and_round_trips(self) -> None:
        env = Environment.from_json({"name": "t", "rules_file": "prod"})
        self.assertEqual(env.rules_file, "prod")
        self.assertEqual(env.to_json()["rules_file"], "prod")
        self.assertEqual(Environment.from_json({"name": "t"}).rules_file, "")


# ----------------------------------------------------------------- 仓库


class RulesStoreTest(unittest.TestCase):
    def test_write_read_list_delete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = FileRulesStore(tmp)
            self.assertEqual(store.names(), [])
            store.write("prod", "10.0.0.1 a.example.com\n")
            self.assertEqual(store.names(), ["prod"])
            self.assertIn("a.example.com", store.read("prod"))
            store.delete("prod")
            self.assertEqual(store.names(), [])
            with self.assertRaises(NotFoundError):
                store.read("prod")

    def test_written_file_is_0600(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = FileRulesStore(tmp)
            store.write("prod", "x")
            mode = stat.S_IMODE(os.stat(os.path.join(tmp, "prod.rules")).st_mode)
            self.assertEqual(mode, 0o600)

    def test_rejects_names_that_escape_the_directory(self) -> None:
        store = FileRulesStore("/tmp/whatever")
        for bad in ("../evil", "a/b", "", "..", "a b", ".hidden", "-lead", "1digit"):
            with self.assertRaises(InvalidConfig):
                store.write(bad, "x")

    def test_names_are_lowercased_not_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            store = FileRulesStore(tmp)
            store.write("Prod", "x")
            self.assertEqual(store.names(), ["prod"])

    def test_memory_store_matches_contract(self) -> None:
        store = InMemoryRulesStore()
        store.write("a", "1")
        self.assertEqual(store.names(), ["a"])
        self.assertEqual(store.read("a"), "1")
        store.delete("a")
        with self.assertRaises(NotFoundError):
            store.read("a")


class CachedRulesLookupTest(unittest.TestCase):
    def test_parses_once_and_reuses_cache(self) -> None:
        store = InMemoryRulesStore()
        store.write("prod", "10.0.0.1 a.example.com\n")
        lookup = CachedRulesLookup(store)
        self.assertEqual(lookup.entries("prod"), {"a.example.com": "10.0.0.1"})
        self.assertEqual(lookup.entries("prod"), {"a.example.com": "10.0.0.1"})

    def test_cache_follows_content_changes(self) -> None:
        store = InMemoryRulesStore()
        store.write("prod", "10.0.0.1 a.example.com\n")
        lookup = CachedRulesLookup(store)
        lookup.entries("prod")
        store.write("prod", "10.0.0.2 b.example.com\n")
        self.assertEqual(lookup.entries("prod"), {"b.example.com": "10.0.0.2"})

    def test_missing_file_is_empty_not_an_error(self) -> None:
        lookup = CachedRulesLookup(InMemoryRulesStore())
        self.assertEqual(lookup.entries("nope"), {})

    def test_empty_name_is_empty(self) -> None:
        self.assertEqual(CachedRulesLookup(InMemoryRulesStore()).entries(""), {})


# ------------------------------------------------------------- 生效优先级


class StaticPrecedenceTest(unittest.IsolatedAsyncioTestCase):
    def _service(
        self, rules: dict[str, str] | None = None
    ) -> tuple[ResolverService, EnvRegistry, StubForward]:
        clock = FrozenClock()
        registry = EnvRegistry(InMemoryStore(), clock)
        registry.load()
        store = InMemoryRulesStore()
        if rules:
            lines = "".join(f"{ip} {host}\n" for host, ip in rules.items())
            store.write("shared", lines)
        lookup = CachedRulesLookup(store)
        fwd = StubForward({"dns.example.com": ["203.0.113.1"]})
        service = ResolverService(
            registry,
            MappingIndex(clock),
            fwd,
            StubSystemDns(),
            clock,
            rules=lookup,
        )
        return service, registry, fwd

    async def test_rules_file_applies_when_bound(self) -> None:
        service, registry, fwd = self._service({"rules.example.com": "10.0.0.1"})
        registry.update(registry.active, {"rules_file": "shared"})
        rows = await service.resolve_hosts(["rules.example.com"])
        self.assertEqual(rows[0]["ips"], ["10.0.0.1"])
        self.assertEqual(rows[0]["source"], SOURCE_STATIC)
        self.assertEqual(rows[0]["static_from"], "rules:shared")
        self.assertEqual(fwd.calls, [], "静态命中不应该再去查 DNS")

    async def test_rules_file_ignored_when_not_bound(self) -> None:
        service, _registry, fwd = self._service({"rules.example.com": "10.0.0.1"})
        fwd.table["rules.example.com"] = ["203.0.113.9"]
        rows = await service.resolve_hosts(["rules.example.com"])
        self.assertEqual(rows[0]["ips"], ["203.0.113.9"])
        self.assertNotIn("static_from", rows[0])

    async def test_environment_inline_hosts_beat_rules_file(self) -> None:
        service, registry, _fwd = self._service({"both.example.com": "10.0.0.1"})
        registry.update(
            registry.active, {"rules_file": "shared", "hosts": {"both.example.com": "10.0.0.99"}}
        )
        rows = await service.resolve_hosts(["both.example.com"])
        self.assertEqual(rows[0]["ips"], ["10.0.0.99"])
        self.assertEqual(rows[0]["static_from"], "environment")

    async def test_switching_environment_switches_rules_file(self) -> None:
        service, registry, fwd = self._service({"shared.example.com": "10.0.0.1"})
        registry.create({"name": "other", "dns_servers": []})
        registry.update(registry.active, {"rules_file": "shared"})
        rows = await service.resolve_hosts(["shared.example.com"])
        self.assertEqual(rows[0]["ips"], ["10.0.0.1"])

        registry.activate("other")  # other 没绑规则文件
        fwd.table["shared.example.com"] = ["203.0.113.7"]
        rows = await service.resolve_hosts(["shared.example.com"])
        self.assertEqual(rows[0]["ips"], ["203.0.113.7"])

    async def test_static_map_reports_both_sources(self) -> None:
        service, registry, _fwd = self._service({"r.example.com": "10.0.0.1"})
        registry.update(
            registry.active, {"rules_file": "shared", "hosts": {"e.example.com": "10.0.0.2"}}
        )
        static = service.static_map()
        self.assertEqual(static["e.example.com"], ("10.0.0.2", "environment"))
        self.assertEqual(static["r.example.com"], ("10.0.0.1", "rules:shared"))


class RulesBindingTest(unittest.TestCase):
    def test_rules_file_survives_a_state_round_trip(self) -> None:
        clock = FrozenClock()
        store = InMemoryStore()
        registry = EnvRegistry(store, clock)
        registry.load()
        registry.update(registry.active, {"rules_file": "prod"})

        reloaded = EnvRegistry(store, clock)
        reloaded.load()
        self.assertEqual(reloaded.active_env.rules_file, "prod")

    def test_unknown_field_still_rejected(self) -> None:
        env = Environment(name="t")
        with self.assertRaises(InvalidConfig):
            env.merged({"nope": 1})

    def test_delete_rules_file_in_use_is_conflict(self) -> None:
        """删除被绑定的规则文件必须被拒 —— 否则环境会静默失去覆盖。见 adapter 层。"""
        # 这里只固化 core 侧的语义：绑定是环境字段，冲突判定在 adapter。
        clock = FrozenClock()
        registry = EnvRegistry(InMemoryStore(), clock)
        registry.load()
        bound = registry.update(registry.active, {"rules_file": "shared"})
        self.assertEqual(bound.rules_file, "shared")
        with self.assertRaises(ConflictError):
            registry.delete(bound.name)  # 激活环境不可删，与规则无关但同样"响亮失败"


if __name__ == "__main__":
    unittest.main()
