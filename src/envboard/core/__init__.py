"""纯逻辑层：领域模型、注册表、映射索引、解析编排。

**不变量**：本包只允许 import 标准库。禁止 import ``mitmproxy`` / ``mitmproxy_rs`` /
``tornado`` —— 由 ``scripts/dependency_lint.py`` 强制校验。
"""

from __future__ import annotations

__all__: list[str] = []
