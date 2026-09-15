"""端口协议 —— core 只依赖这些抽象，不直接触碰外界。

对应 AGENTS.md §5.2：core 通过端口访问 http/fs/env/net/os/logger/clock，
禁止直接使用 ``socket`` / ``open`` / ``time`` 等全局能力。
"""

from __future__ import annotations

from collections.abc import Mapping
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Any
from typing import Protocol
from typing import runtime_checkable

RCODE_NOERROR = "noerror"
RCODE_NXDOMAIN = "nxdomain"
RCODE_NODATA = "nodata"
RCODE_SERVFAIL = "servfail"
RCODE_TIMEOUT = "timeout"
RCODE_UNSUPPORTED = "unsupported"
RCODE_ERROR = "error"


@dataclass(frozen=True)
class LookupResult:
    """一次正向解析（host -> ip）的结果。"""

    host: str
    ips: tuple[str, ...] = ()
    rcode: str = RCODE_NOERROR
    error: str = ""

    @property
    def ok(self) -> bool:
        return self.rcode == RCODE_NOERROR


@runtime_checkable
class ForwardResolver(Protocol):
    async def lookup(self, host: str, dns_servers: Sequence[str]) -> LookupResult:
        """把 ``host`` 解析成 IP 列表。``dns_servers`` 为空表示跟随操作系统 DNS。"""
        ...


@runtime_checkable
class SystemDnsPort(Protocol):
    def servers(self) -> Sequence[str]:
        """返回操作系统配置的 DNS 服务器（可能为空）。"""
        ...


@runtime_checkable
class EnvStore(Protocol):
    def read(self) -> dict[str, Any] | None:
        """读取持久化状态；不存在时返回 None。"""
        ...

    def write(self, data: dict[str, Any]) -> None:
        """原子写入持久化状态。"""
        ...

    def path(self) -> str:
        """返回持久化文件路径（用于展示）。"""
        ...


@runtime_checkable
class RulesStore(Protocol):
    """规则文件仓库：`<confdir>/rules/<name>.rules`。

    规则文件是**已规范化**的静态覆盖，由 hosts 风格输入解析而来；环境通过
    ``Environment.rules_file`` 绑定其中一个，切换环境即切换规则文件。

    仓库**不理解**文件内容 —— 解析与渲染都在 core（:mod:`envboard.core.rules`）。
    """

    def names(self) -> list[str]:
        """列出全部规则文件名（不含后缀），排序稳定。"""
        ...

    def read(self, name: str) -> str:
        """读取规则文件原文；不存在时抛 ``NotFoundError``。"""
        ...

    def write(self, name: str, text: str) -> None:
        """原子写入规则文件（0600）。"""
        ...

    def delete(self, name: str) -> None:
        """删除规则文件；不存在时抛 ``NotFoundError``。"""
        ...

    def dir_path(self) -> str:
        """返回规则文件目录（用于展示）。"""
        ...


@runtime_checkable
class RulesLookup(Protocol):
    """按规则文件名取 `{host: ip}`。

    与 :class:`RulesStore` 分开：仓库管**字节**，这里管**已解析的映射**（可带缓存）。
    """

    def entries(self, name: str) -> Mapping[str, str]:
        """返回该规则文件的静态覆盖；文件不存在时返回空映射（不抛异常）。"""
        ...


@runtime_checkable
class Clock(Protocol):
    def now(self) -> float:
        ...

@runtime_checkable
class Logger(Protocol):
    def debug(self, msg: str) -> None:
        ...

    def info(self, msg: str) -> None:
        ...

    def warning(self, msg: str) -> None:
        ...

    def error(self, msg: str) -> None:
        ...
