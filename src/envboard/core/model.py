"""环境（Environment）领域模型 —— 不依赖 mitmproxy，仅标准库。"""

from __future__ import annotations

import ipaddress
import re
from dataclasses import dataclass, field
from typing import Any
from typing import Mapping

from .errors import InvalidConfig

NAME_RE = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")
COLOR_RE = re.compile(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")
LABEL_RE = re.compile(r"^(?!-)[A-Za-z0-9_-]{1,63}(?<!-)$")

#: 规则文件名（`<confdir>/rules/<name>.rules`）与数据文件同名空间，必须文件系统安全。
RULES_NAME_RE = NAME_RE

#: 首次启动自动创建的环境名，代表"跟随操作系统 DNS"。
DEFAULT_ENV = "local"

#: 允许通过 PUT 修改的字段。
MUTABLE_FIELDS = frozenset(
    {
        "name",
        "dns_servers",
        "domain_suffix",
        "hosts",
        "labels",
        "color",
        "description",
        "rules_file",
    }
)

INFINITE_TTL = 0
"""TTL == 0 表示永不过期（静态配置项）。"""


def normalize_host(value: str) -> str:
    """host 的规范形式：去空白、转小写、去根点、去 ``*.`` 通配前缀。

    这是全仓**唯一**的 host 归一化定义 —— resolver 查表、环境的 hosts 键、
    规则文件的键都走它。任何一处漏掉归一化，都会造成"配置写了却永不命中"的静默失效。
    """
    host = value.strip().lower().rstrip(".")
    if host.startswith("*."):
        host = host[2:]
    return host


def _validate_ip(value: str, field_path: str, *, what: str) -> str:
    v = value.strip()
    if not v:
        raise InvalidConfig(f"{what} must not be empty", field=field_path)
    # 容忍 [::1] 这种带方括号的 IPv6 字面量
    if v.startswith("[") and v.endswith("]"):
        v = v[1:-1]
    try:
        ipaddress.ip_address(v)
    except ValueError:
        raise InvalidConfig(
            f"{what} {value!r} is not a bare IP literal. "
            "mitmproxy_rs.dns.DnsResolver accepts only bare IPv4/IPv6 literals: "
            "no 'ip:port' and no hostname.",
            field=field_path,
        ) from None
    return v


def _validate_domain(value: str, field_path: str, *, what: str) -> str:
    """校验并返回域名的规范形式（``normalize_host`` 的结果）。"""
    domain = normalize_host(value)
    if not domain:
        raise InvalidConfig(f"{what} must not be empty", field=field_path)
    if len(domain) > 253:
        raise InvalidConfig(f"{what} {value!r} is too long", field=field_path)
    for label in domain.split("."):
        if not LABEL_RE.match(label):
            raise InvalidConfig(
                f"{what} {value!r} contains an invalid label {label!r}",
                field=field_path,
            )
    return domain


def normalize_hosts(raw: Mapping[str, Any], path: str) -> dict[str, str]:
    """归一化并校验一份 hosts 映射，返回可直接使用的 `{normalized_host: ip}`。

    键走 ``_validate_domain``（因此 ``*.`` 前缀与根点会被剥离、转小写），
    值走 ``_validate_ip``。归一化后撞键（如同时写了 ``A.com`` 与 ``a.com``）
    是**响亮失败**，不静默取其一。
    """
    out: dict[str, str] = {}
    origin: dict[str, str] = {}
    for key, value in raw.items():
        raw_key = str(key)
        host = _validate_domain(raw_key, f"{path}[{raw_key!r}]", what="host key")
        if host in out:
            raise InvalidConfig(
                f"hosts keys {origin[host]!r} and {raw_key!r} normalize to the same "
                f"host {host!r}",
                field=f"{path}[{raw_key!r}]",
            )
        out[host] = _validate_ip(str(value), f"{path}[{raw_key!r}]", what="host address")
        origin[host] = raw_key
    return out


def is_ip_literal(value: str) -> bool:
    """是否为裸 IP 字面量（含 ``[::1]`` 形式）。

    规则文件解析器靠它判断 ``ip host`` 还是 ``host ip`` 写法。
    """
    v = value.strip()
    if v.startswith("[") and v.endswith("]"):
        v = v[1:-1]
    if not v:
        return False
    try:
        ipaddress.ip_address(v)
    except ValueError:
        return False
    return True


def validate_ip(value: str, *, what: str = "ip", field_path: str = "ip") -> str:
    """校验并返回裸 IP 字面量（去掉 ``[]``）。不合法则抛 :class:`InvalidConfig`。"""
    return _validate_ip(value, field_path, what=what)


def validate_host(value: str, *, what: str = "host", field_path: str = "host") -> str:
    """校验并返回 host 的规范形式。不合法则抛 :class:`InvalidConfig`。"""
    return _validate_domain(value, field_path, what=what)


@dataclass
class Environment:
    """一套可切换的目标环境。

    ``dns_servers`` 为空表示"跟随操作系统 DNS"（即 :data:`DEFAULT_ENV` 的语义）。
    """

    name: str
    dns_servers: tuple[str, ...] = ()
    domain_suffix: str = ""
    hosts: dict[str, str] = field(default_factory=dict)
    labels: dict[str, str] = field(default_factory=dict)
    color: str = ""
    description: str = ""
    #: 绑定到本环境的规则文件名（`<confdir>/rules/<name>.rules`）；空串 = 不绑定。
    rules_file: str = ""

    def __post_init__(self) -> None:
        # 键归一化必须在构造时完成，不能等到 validate()：resolver 用归一化后的 host
        # 查表，未归一化的键会**静默永不命中**（契约见 core/spec/capabilities.md）。
        self.hosts = {normalize_host(str(k)): str(v) for k, v in self.hosts.items()}

    # ---------------------------------------------------------------- 校验

    def validate(self, *, path: str = "environment") -> None:
        if not NAME_RE.match(self.name):
            raise InvalidConfig(
                f"invalid environment name {self.name!r}: must match {NAME_RE.pattern}",
                field=f"{path}.name",
            )
        seen: set[str] = set()
        for i, server in enumerate(self.dns_servers):
            if not isinstance(server, str):
                raise InvalidConfig(
                    "dns_servers entries must be strings", field=f"{path}.dns_servers[{i}]"
                )
            normalized = _validate_ip(server, f"{path}.dns_servers[{i}]", what="dns server")
            if normalized in seen:
                raise InvalidConfig(
                    f"duplicate dns server {normalized!r}", field=f"{path}.dns_servers[{i}]"
                )
            seen.add(normalized)
        if self.domain_suffix:
            _validate_domain(self.domain_suffix, f"{path}.domain_suffix", what="domain_suffix")
        for key, value in self.hosts.items():
            _validate_domain(key, f"{path}.hosts[{key!r}]", what="host key")
            _validate_ip(value, f"{path}.hosts[{key!r}]", what="host address")
        if self.rules_file and not RULES_NAME_RE.match(self.rules_file):
            raise InvalidConfig(
                f"invalid rules_file {self.rules_file!r}: must match "
                f"{RULES_NAME_RE.pattern}, or be empty for 'no rules file'",
                field=f"{path}.rules_file",
            )
        if self.color and not COLOR_RE.match(self.color):
            raise InvalidConfig(
                f"invalid color {self.color!r}: expected #rgb or #rrggbb",
                field=f"{path}.color",
            )
        for key, value in self.labels.items():
            if not isinstance(key, str) or not isinstance(value, str):
                raise InvalidConfig(
                    "labels must be a mapping of string to string",
                    field=f"{path}.labels",
                )

    # ------------------------------------------------------------ 序列化

    def to_json(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "dns_servers": list(self.dns_servers),
            "domain_suffix": self.domain_suffix,
            "hosts": dict(self.hosts),
            "labels": dict(self.labels),
            "color": self.color,
            "description": self.description,
            "rules_file": self.rules_file,
        }

    @classmethod
    def from_json(cls, data: Mapping[str, Any], *, path: str = "environment") -> Environment:
        if not isinstance(data, Mapping):
            raise InvalidConfig(f"{path} must be an object", field=path)
        unknown = set(data) - MUTABLE_FIELDS
        if unknown:
            raise InvalidConfig(
                f"unknown field(s) {sorted(unknown)}", field=f"{path}.{sorted(unknown)[0]}"
            )
        name = data.get("name")
        if not isinstance(name, str):
            raise InvalidConfig("environment.name is required", field=f"{path}.name")
        raw_servers = data.get("dns_servers") or []
        if isinstance(raw_servers, str) or not isinstance(raw_servers, (list, tuple)):
            raise InvalidConfig(
                "dns_servers must be a list of IP literals", field=f"{path}.dns_servers"
            )
        raw_hosts = data.get("hosts") or {}
        if not isinstance(raw_hosts, Mapping):
            raise InvalidConfig("hosts must be an object", field=f"{path}.hosts")
        raw_labels = data.get("labels") or {}
        if not isinstance(raw_labels, Mapping):
            raise InvalidConfig("labels must be an object", field=f"{path}.labels")
        env = cls(
            name=name.strip().lower(),
            dns_servers=tuple(raw_servers),
            domain_suffix=str(data.get("domain_suffix") or ""),
            hosts=normalize_hosts(raw_hosts, f"{path}.hosts"),
            labels={str(k): str(v) for k, v in raw_labels.items()},
            color=str(data.get("color") or ""),
            description=str(data.get("description") or ""),
            rules_file=str(data.get("rules_file") or "").strip().lower(),
        )
        env.validate(path=path)
        return env

    def merged(self, patch: Mapping[str, Any], *, path: str = "environment") -> Environment:
        """返回应用 patch 后的新实例（不修改 self）。"""
        data = self.to_json()
        for key, value in patch.items():
            if key not in MUTABLE_FIELDS:
                raise InvalidConfig(f"unknown field {key!r}", field=f"{path}.{key}")
            data[key] = value
        return Environment.from_json(data, path=path)
