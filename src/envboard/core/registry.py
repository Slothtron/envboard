"""环境注册表：增删改查 + 激活切换 + 持久化。"""

from __future__ import annotations

from collections.abc import Callable
from collections.abc import Mapping as MappingABC
from typing import Any

from .errors import ConflictError
from .errors import InvalidConfig
from .errors import NotFoundError
from .model import DEFAULT_ENV
from .model import Environment
from .model import MUTABLE_FIELDS
from .ports import Clock
from .ports import EnvStore
from .ports import Logger

STATE_VERSION = 1

Observer = Callable[[str], None]
"""观察者签名：收到 ``"environments"`` 或 ``"active"`` 事件字符串。"""


class _NullLogger:
    def debug(self, msg: str) -> None: ...
    def info(self, msg: str) -> None: ...
    def warning(self, msg: str) -> None: ...
    def error(self, msg: str) -> None: ...


class EnvRegistry:
    def __init__(
        self,
        store: EnvStore,
        clock: Clock,
        *,
        logger: Logger | None = None,
        default_env: str = DEFAULT_ENV,
    ) -> None:
        self._store = store
        self._clock = clock
        self._log: Logger = logger or _NullLogger()
        self._default_env = default_env
        self._envs: dict[str, Environment] = {}
        self._active: str = default_env
        self._observers: list[Observer] = []
        self._loaded = False

    # ------------------------------------------------------------ 生命周期

    def on_change(self, observer: Observer) -> None:
        self._observers.append(observer)

    def _notify(self, kind: str) -> None:
        for observer in list(self._observers):
            try:
                observer(kind)
            except Exception:  # noqa: BLE001 - 观察者异常不得影响主流程
                self._log.warning(f"envboard: change observer failed for {kind!r}")

    def load(self) -> None:
        """加载持久化状态；文件不存在时建立默认环境并落盘。"""
        raw = self._store.read()
        if not raw:
            self._envs = {self._default_env: Environment(name=self._default_env)}
            self._active = self._default_env
            self._loaded = True
            self.save()
            self._notify("environments")
            self._notify("active")
            return

        version = raw.get("version", STATE_VERSION)
        if version != STATE_VERSION:
            raise InvalidConfig(
                f"unsupported envboard state version {version!r}, expected {STATE_VERSION}",
                field="version",
            )
        envs: dict[str, Environment] = {}
        raw_envs = raw.get("environments") or []
        if not isinstance(raw_envs, list):
            raise InvalidConfig("environments must be a list", field="environments")
        for item in raw_envs:
            env = Environment.from_json(item, path="environments")
            if env.name in envs:
                raise InvalidConfig(
                    f"duplicate environment {env.name!r}", field="environments"
                )
            envs[env.name] = env
        if not envs:
            envs = {self._default_env: Environment(name=self._default_env)}
        active = str(raw.get("active") or "")
        if active not in envs:
            active = next(iter(envs))
        self._envs = envs
        self._active = active
        self._loaded = True
        self._notify("environments")
        self._notify("active")

    def save(self) -> None:
        self._store.write(
            {
                "version": STATE_VERSION,
                "active": self._active,
                "environments": [env.to_json() for env in self._envs.values()],
            }
        )

    # ------------------------------------------------------------ 查询

    @property
    def active(self) -> str:
        return self._active

    @property
    def active_env(self) -> Environment:
        return self._envs[self._active]

    def names(self) -> list[str]:
        return list(self._envs)

    def list(self) -> list[dict[str, Any]]:
        return [self._as_json(env) for env in self._envs.values()]

    def get(self, name: str) -> Environment:
        key = (name or "").strip().lower()
        try:
            return self._envs[key]
        except KeyError:
            raise NotFoundError(f"unknown environment {name!r}", field="name") from None

    def _as_json(self, env: Environment) -> dict[str, Any]:
        data = env.to_json()
        data["active"] = env.name == self._active
        return data

    # ------------------------------------------------------------ 写入

    def create(self, data: MappingABC[str, Any]) -> Environment:
        env = Environment.from_json(data, path="environment")
        if env.name in self._envs:
            raise ConflictError(f"environment {env.name!r} already exists", field="name")
        self._envs[env.name] = env
        self._persist("environments")
        return env

    def update(self, name: str, patch: MappingABC[str, Any]) -> Environment:
        current = self.get(name)
        if not isinstance(patch, MappingABC):
            raise InvalidConfig("patch must be an object", field="patch")
        unknown = set(patch) - MUTABLE_FIELDS
        if unknown:
            raise InvalidConfig(
                f"unknown field(s) {sorted(unknown)}", field=f"patch.{sorted(unknown)[0]}"
            )
        candidate = current.merged(patch)
        renamed = candidate.name != current.name
        if renamed and candidate.name in self._envs:
            raise ConflictError(
                f"environment {candidate.name!r} already exists", field="name"
            )
        if renamed:
            # 保持插入顺序，避免下拉框顺序跳动
            rebuilt: dict[str, Environment] = {}
            for key, value in self._envs.items():
                rebuilt[candidate.name if key == current.name else key] = (
                    candidate if key == current.name else value
                )
            self._envs = rebuilt
            if self._active == current.name:
                self._active = candidate.name
        else:
            self._envs[current.name] = candidate
        self._persist("environments")
        if renamed and self._active == candidate.name:
            self._notify("active")
        return candidate

    def delete(self, name: str) -> None:
        env = self.get(name)
        if env.name == self._active:
            raise ConflictError(
                f"cannot delete the active environment {env.name!r}; switch first",
                field="name",
            )
        if len(self._envs) == 1:
            raise ConflictError("cannot delete the last environment", field="name")
        del self._envs[env.name]
        self._persist("environments")

    def activate(self, name: str) -> Environment:
        env = self.get(name)
        if env.name == self._active:
            return env  # 幂等：切到当前环境不产生事件
        self._active = env.name
        self._persist("active")  # _persist 内部已 _notify("active")
        return env

    def _persist(self, kind: str) -> None:
        self.save()
        self._notify(kind)
