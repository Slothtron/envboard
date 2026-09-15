"""统一错误码（见 core/spec/errors.md）。

适配器只做翻译，不重新判定错误类别 —— 与 AGENTS.md §5.4 一致。
"""

from __future__ import annotations

from typing import Any


class EnvBoardError(Exception):
    """所有 envboard 错误的基类。"""

    code: str = "internal_error"
    http_status: int = 500

    def __init__(self, message: str, *, field: str | None = None) -> None:
        super().__init__(message)
        self.message = message
        self.field = field

    def to_json(self) -> dict[str, Any]:
        error: dict[str, Any] = {"code": self.code, "message": self.message}
        if self.field:
            error["field"] = self.field
        return {"error": error}


class InvalidConfig(EnvBoardError):
    """配置非法（环境定义、DNS 服务器、端口等）。加载即失败，禁止静默降级。"""

    code = "invalid_config"
    http_status = 400


class NotFoundError(EnvBoardError):
    code = "not_found"
    http_status = 404


class ConflictError(EnvBoardError):
    """状态冲突，例如删除当前激活环境、重名。"""

    code = "conflict"
    http_status = 409


class DnsError(EnvBoardError):
    code = "dns_failure"
    http_status = 502


class StoreError(EnvBoardError):
    """持久化文件读写失败。"""

    code = "store_failure"
    http_status = 500


class DisabledError(EnvBoardError):
    code = "disabled"
    http_status = 409


class UpstreamError(EnvBoardError):
    """宿主能力缺失，例如未运行 mitmweb 时访问 Dashboard。"""

    code = "upstream_unavailable"
    http_status = 503
