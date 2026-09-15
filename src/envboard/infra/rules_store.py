"""规则文件仓库：`<confdir>/rules/<name>.rules`，原子写 + 0600。

与 :mod:`envboard.infra.store` 同款做法：先写临时文件、``fsync``、再 ``os.replace``，
避免半截文件被读到。规则文件是纯文本，内容由 core 负责生成。
"""

from __future__ import annotations

import os
import tempfile
from collections.abc import Mapping

from ..core.errors import EnvBoardError
from ..core.errors import NotFoundError
from ..core.errors import StoreError
from ..core.model import RULES_NAME_RE
from ..core.rules import RULES_SUFFIX
from ..core.rules import parse_hosts_text

DEFAULT_DIRNAME = "rules"


def default_rules_dir(confdir: str) -> str:
    """`<confdir>/rules`。confdir 默认与 mitmproxy 一致（``~/.mitmproxy``）。"""
    return os.path.join(os.path.expanduser(confdir or "~/.mitmproxy"), DEFAULT_DIRNAME)


class FileRulesStore:
    def __init__(self, root: str) -> None:
        self._root = os.path.abspath(os.path.expanduser(root))

    def dir_path(self) -> str:
        return self._root

    # ------------------------------------------------------------------ 内部

    def _check_name(self, name: str) -> str:
        """校验并返回规则名。名字进文件名，必须挡住路径穿越。"""
        value = (name or "").strip().lower()
        if not RULES_NAME_RE.match(value):
            from ..core.errors import InvalidConfig

            raise InvalidConfig(
                f"invalid rules name {name!r}: must match {RULES_NAME_RE.pattern}",
                field="name",
            )
        return value

    def _path(self, name: str) -> str:
        return os.path.join(self._root, self._check_name(name) + RULES_SUFFIX)

    # ------------------------------------------------------------------- 契约

    def names(self) -> list[str]:
        try:
            entries = os.listdir(self._root)
        except FileNotFoundError:
            return []
        except OSError as exc:
            raise StoreError(f"cannot list {self._root}: {exc}") from exc
        return sorted(
            entry[: -len(RULES_SUFFIX)]
            for entry in entries
            if entry.endswith(RULES_SUFFIX) and not entry.startswith(".")
        )

    def read(self, name: str) -> str:
        path = self._path(name)
        try:
            with open(path, encoding="utf-8") as fh:
                return fh.read()
        except FileNotFoundError:
            raise NotFoundError(f"unknown rules file {name!r}", field="name") from None
        except OSError as exc:
            raise StoreError(f"cannot read {path}: {exc}") from exc

    def write(self, name: str, text: str) -> None:
        path = self._path(name)
        directory = os.path.dirname(path) or "."
        try:
            os.makedirs(directory, mode=0o700, exist_ok=True)
            fd, tmp = tempfile.mkstemp(dir=directory, prefix=".envboard-", suffix=".tmp")
            try:
                os.fchmod(fd, 0o600)
                with os.fdopen(fd, "w", encoding="utf-8") as fh:
                    fh.write(text)
                    fh.flush()
                    os.fsync(fh.fileno())
                os.replace(tmp, path)
            except BaseException:
                try:
                    os.unlink(tmp)
                except OSError:
                    pass
                raise
        except OSError as exc:
            raise StoreError(f"cannot write {path}: {exc}") from exc

    def delete(self, name: str) -> None:
        path = self._path(name)
        try:
            os.unlink(path)
        except FileNotFoundError:
            raise NotFoundError(f"unknown rules file {name!r}", field="name") from None
        except OSError as exc:
            raise StoreError(f"cannot delete {path}: {exc}") from exc


class InMemoryRulesStore:
    """测试替身。"""

    def __init__(self, initial: dict[str, str] | None = None, root: str = "<memory>") -> None:
        self._files = dict(initial or {})
        self._root = root

    def dir_path(self) -> str:
        return self._root

    def names(self) -> list[str]:
        return sorted(self._files)

    def read(self, name: str) -> str:
        try:
            return self._files[(name or "").strip().lower()]
        except KeyError:
            raise NotFoundError(f"unknown rules file {name!r}", field="name") from None

    def write(self, name: str, text: str) -> None:
        self._files[(name or "").strip().lower()] = text

    def delete(self, name: str) -> None:
        try:
            del self._files[(name or "").strip().lower()]
        except KeyError:
            raise NotFoundError(f"unknown rules file {name!r}", field="name") from None


class CachedRulesLookup:
    """把规则文件解析成 ``{host: ip}`` 并缓存。

    缓存键是**文件原文**而不是 mtime：原文没变就直接复用解析结果，
    原文变了（无论是我们写的还是用户手改的）就重解析。这样既不需要
    ``stat``，也不会因为 mtime 精度问题读到旧值。

    规则文件读不到（被删了）时返回空映射而不是抛异常 —— 解析路径不该因为
    一个规则文件缺失就整体失败。
    """

    def __init__(self, store: FileRulesStore | InMemoryRulesStore) -> None:
        self._store = store
        self._cache: dict[str, tuple[str, dict[str, str]]] = {}

    def invalidate(self, name: str | None = None) -> None:
        if name is None:
            self._cache.clear()
        else:
            self._cache.pop((name or "").strip().lower(), None)

    def entries(self, name: str) -> Mapping[str, str]:
        key = (name or "").strip().lower()
        if not key:
            return {}
        try:
            text = self._store.read(key)
        except EnvBoardError:
            self._cache.pop(key, None)
            return {}
        cached = self._cache.get(key)
        if cached is not None and cached[0] == text:
            return cached[1]
        parsed = parse_hosts_text(text).entries
        self._cache[key] = (text, parsed)
        return parsed
