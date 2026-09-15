"""envboard —— mitmproxy 多环境 DNS 工作台。

分层（依赖单向，见 AGENTS.md §1）：

* ``core/``   纯逻辑 + 端口协议，只依赖标准库；禁止 import mitmproxy / mitmproxy_rs。
* ``infra/``  端口实现：mitmproxy_rs 正向解析、JSON 持久化、系统 DNS/时钟。
* ``adapter/`` mitmproxy 接线：addon 入口、hook、命令、tornado 路由。

加载：``mitmweb -s addons/envboard.py``
"""

from __future__ import annotations

__version__ = "0.1.0"

__all__ = ["__version__"]
