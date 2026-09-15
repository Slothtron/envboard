"""持久化：单个 JSON 文件，原子写 + 0600 权限。

对应 AGENTS.md §10：写入凭据/状态文件必须收紧权限。
"""

from __future__ import annotations

import json
import os
import tempfile
from typing import Any

from ..core.errors import StoreError

DEFAULT_FILENAME = "envboard.json"


def default_path(confdir: str) -> str:
    """把 mitmproxy 的 ``confdir``（默认 ``~/.mitmproxy``）展开成状态文件路径。"""
    return os.path.join(os.path.expanduser(confdir or "~/.mitmproxy"), DEFAULT_FILENAME)


class JsonFileStore:
    def __init__(self, path: str) -> None:
        self._path = os.path.abspath(os.path.expanduser(path))

    def path(self) -> str:
        return self._path

    def read(self) -> dict[str, Any] | None:
        try:
            with open(self._path, "rb") as fh:
                raw = fh.read()
        except FileNotFoundError:
            return None
        except OSError as exc:
            raise StoreError(f"cannot read {self._path}: {exc}") from exc
        if not raw.strip():
            return None
        try:
            data = json.loads(raw.decode("utf-8"))
        except (ValueError, UnicodeDecodeError) as exc:
            raise StoreError(f"cannot parse {self._path}: {exc}") from exc
        if not isinstance(data, dict):
            raise StoreError(f"{self._path} must contain a JSON object")
        return data

    def write(self, data: dict[str, Any]) -> None:
        directory = os.path.dirname(self._path) or "."
        try:
            os.makedirs(directory, mode=0o700, exist_ok=True)
            payload = json.dumps(data, indent=2, ensure_ascii=False) + "\n"
            fd, tmp = tempfile.mkstemp(dir=directory, prefix=".envboard-", suffix=".tmp")
            try:
                os.fchmod(fd, 0o600)
                with os.fdopen(fd, "w", encoding="utf-8") as fh:
                    fh.write(payload)
                    fh.flush()
                    os.fsync(fh.fileno())
                os.replace(tmp, self._path)
            except BaseException:
                try:
                    os.unlink(tmp)
                except OSError:
                    pass
                raise
        except OSError as exc:
            raise StoreError(f"cannot write {self._path}: {exc}") from exc


class InMemoryStore:
    """测试替身。"""

    def __init__(self, initial: dict[str, Any] | None = None, name: str = "<memory>") -> None:
        self._data = initial
        self._path = name
        self.writes = 0

    def path(self) -> str:
        return self._path

    def read(self) -> dict[str, Any] | None:
        return None if self._data is None else json.loads(json.dumps(self._data))

    def write(self, data: dict[str, Any]) -> None:
        self._data = json.loads(json.dumps(data))
        self.writes += 1
