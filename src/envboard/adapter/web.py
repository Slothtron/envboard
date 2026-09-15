"""Dashboard 接线：把 envboard 的 REST 面挂到 mitmweb 的 tornado 应用上。

为什么不新起一个 Web 服务：

* mitmweb 已有成熟宿主；``Application.add_handlers()`` 是 tornado 公开 API，
  后挂的路由会被插到最终 catch-all 之前、mitmweb 自身路由之后 —— **已实测生效**。
* 继承 ``mitmproxy.tools.web.app.RequestHandler`` 即自动获得 mitmweb 的
  口令/令牌鉴权（``AuthRequestHandler.__init_subclass__`` 自动包装）、
  ``Sec-Fetch-Site`` 同源校验与 XSRF cookie 校验。
* 因此 Dashboard 不会引入第二套鉴权，也不会把口子开到 127.0.0.1 之外。
"""

from __future__ import annotations

import functools
import json
import logging
import os
from typing import Any

from ..core.errors import EnvBoardError
from ..core.errors import InvalidConfig
from ..core.errors import UpstreamError

logger = logging.getLogger(__name__)

_MOUNTED: set[int] = set()
_INDEX_CACHE: dict[str, tuple[float, str]] = {}


def normalize_prefix(prefix: str) -> str:
    value = (prefix or "").strip() or "/envboard"
    if not value.startswith("/"):
        value = "/" + value
    return value.rstrip("/") or "/envboard"


def index_path() -> str:
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.join(os.path.dirname(here), "web", "index.html")


def read_index() -> str:
    path = index_path()
    try:
        mtime = os.path.getmtime(path)
    except OSError as exc:
        # 打包错误，不是客户端错误：这里刻意抛基类（internal_error / 500）
        raise EnvBoardError(f"dashboard asset missing: {path} ({exc})") from exc
    cached = _INDEX_CACHE.get(path)
    if cached is None or cached[0] != mtime:
        with open(path, encoding="utf-8") as fh:
            _INDEX_CACHE[path] = (mtime, fh.read())
    return _INDEX_CACHE[path][1]


def mount(addon, prefix: str = "/envboard") -> bool:
    """把 Dashboard 路由挂到 mitmweb。返回是否挂载成功。"""
    from mitmproxy import ctx

    try:
        from mitmproxy.tools.web import app as webapp
    except Exception as exc:  # noqa: BLE001 - 非 mitmweb 宿主
        logger.info(f"envboard: mitmweb is not available ({exc!r}); dashboard disabled")
        return False

    application = getattr(ctx.master, "app", None)
    if application is None:
        logger.info(
            "envboard: no mitmweb application on this master; dashboard disabled. "
            "Use `envboard.*` commands instead."
        )
        return False
    if id(application) in _MOUNTED:
        logger.debug("envboard: dashboard already mounted")
        return True

    root = normalize_prefix(prefix)
    routes = _build_routes(webapp, addon, root)
    application.add_handlers(r".*", routes)
    _MOUNTED.add(id(application))
    logger.info(f"envboard: dashboard mounted at {root}/")
    return True


def _build_routes(webapp, addon, root: str):  # noqa: ANN001, ANN202
    import tornado.web

    def guard(fn):  # noqa: ANN001, ANN202
        @functools.wraps(fn)
        async def wrapper(self, *args, **kwargs):  # noqa: ANN001, ANN202
            try:
                return await fn(self, *args, **kwargs)
            except EnvBoardError as exc:
                self.finish_json(exc.to_json(), status=exc.http_status)
            except tornado.web.HTTPError:
                raise
            except Exception as exc:  # noqa: BLE001
                logger.exception("envboard: dashboard handler failed")
                self.finish_json(
                    {"error": {"code": "internal_error", "message": str(exc)}}, status=500
                )

        return wrapper

    class Base(webapp.RequestHandler):
        application: Any

        def finish_json(self, payload: Any, status: int = 200) -> None:
            self.set_status(status)
            self.set_header("Content-Type", "application/json; charset=UTF-8")
            self.finish(json.dumps(payload, ensure_ascii=False))

        def body_json(self) -> dict[str, Any]:
            raw = self.request.body
            if not raw:
                return {}
            try:
                data = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError) as exc:
                raise InvalidConfig(f"invalid JSON body: {exc}", field="body") from exc
            if not isinstance(data, dict):
                raise InvalidConfig("request body must be a JSON object", field="body")
            return data

        @property
        def board(self):  # noqa: ANN201
            return addon

        def require_service(self):  # noqa: ANN201
            service = addon.service
            if service is None or addon.registry is None:
                raise UpstreamError("envboard is not initialised yet")
            return service

    class IndexHandler(Base):
        def auth_fail(self, invalid_password: bool) -> None:
            # 复用 mitmweb 自带的登录页，避免出现"裸 403"
            self.render("login.html", invalid_password=invalid_password)

        @guard
        async def get(self) -> None:
            # 强制下发 _mitmproxy_xsrf cookie：mitmweb 是靠 /updates 这个 WebSocket
            # 触发下发的，而本页不开 WS。不主动读一次 xsrf_token，写请求会 403。
            assert self.xsrf_token
            self.set_header("Content-Type", "text/html; charset=UTF-8")
            self.finish(read_index())

        post = get  # 登录表单 POST 到当前路径，行为与 mitmweb 的 IndexHandler 一致

    class AssetHandler(Base):
        """Dashboard 的静态资源（同源外部文件）。

        mitmweb 对 Web 界面下发的 CSP 是::

            default-src 'self'; connect-src 'self' ws:; img-src 'self' data:;
            style-src 'self' 'unsafe-inline'

        没有给 ``script-src`` 单独开口，于是回落到 ``default-src 'self'`` ——
        浏览器会**直接拒绝内联 <script>**（只报一行 console 错误，页面看着"正常"但
        一行 JS 都没跑）。所以 Dashboard 的 JavaScript **必须**是外部同源文件。
        内联 <style> 不受影响，因为 CSP 显式给了 ``style-src 'unsafe-inline'``。
        """

        #: 允许提供的资源白名单（名字 -> Content-Type）。白名单同时挡掉了路径穿越。
        ASSETS = {"app.js": "application/javascript; charset=UTF-8"}

        @guard
        async def get(self, name: str) -> None:
            ctype = self.ASSETS.get(name)
            if ctype is None:
                raise tornado.web.HTTPError(404)
            path = os.path.join(os.path.dirname(index_path()), name)
            try:
                with open(path, encoding="utf-8") as fh:
                    payload = fh.read()
            except OSError as exc:
                # 打包错误，不是客户端错误
                raise EnvBoardError(f"dashboard asset missing: {path} ({exc})") from exc
            self.set_header("Content-Type", ctype)
            self.set_header("Cache-Control", "no-cache")
            self.finish(payload)

    class EnvironmentsHandler(Base):
        @guard
        async def get(self) -> None:
            registry = self.board.registry
            self.finish_json(
                {
                    "active": registry.active,
                    "environments": registry.list(),
                }
            )

        @guard
        async def post(self) -> None:
            registry = self.board.registry
            env = registry.create(self.body_json())
            self.finish_json({"environment": _as_json(registry, env)}, status=201)

    class EnvironmentHandler(Base):
        @guard
        async def get(self, name: str) -> None:
            registry = self.board.registry
            self.finish_json({"environment": _as_json(registry, registry.get(name))})

        @guard
        async def put(self, name: str) -> None:
            registry = self.board.registry
            env = registry.update(name, self.body_json())
            self.finish_json({"environment": _as_json(registry, env)})

        @guard
        async def delete(self, name: str) -> None:
            registry = self.board.registry
            registry.delete(name)
            self.board.index.forget_env(name)
            self.finish_json({"deleted": name})

    class RulesHandler(Base):
        """规则文件：清单 / 导入 / 查看 / 删除。

        规则文件按环境绑定（``Environment.rules_file``），切环境即切规则文件；
        绑定走通用的 ``PUT /api/environments/<env>``（``rules_file`` 是可变字段）。
        """

        @guard
        async def get(self) -> None:
            self.finish_json(
                {
                    "rules": self.board.rules_list(),
                    "dir": self.board.rules_store.dir_path(),
                }
            )

        @guard
        async def post(self) -> None:
            # 导入挂在**集合**上（POST /api/rules）而不是 /api/rules/import：
            # "import" 本身是合法的规则名，做成子路径会和 {name} 路由撞车 ——
            # tornado 先匹配到 RuleHandler，而它没有 post，于是得到 405。
            payload = self.body_json()
            name = str(payload.get("name") or "").strip().lower()
            text = payload.get("text")
            path = payload.get("path")
            if text is not None and not isinstance(text, str):
                raise InvalidConfig("`text` must be a string", field="text")
            if not name and not path:
                raise InvalidConfig("`name` is required", field="name")
            if text is not None:
                if not name:
                    raise InvalidConfig("`name` is required when importing text", field="name")
                result = self.board.import_rules_text(
                    name, text, source=str(payload.get("source") or "dashboard")
                )
            elif path:
                if not isinstance(path, str):
                    raise InvalidConfig("`path` must be a string", field="path")
                result = self.board.import_rules_file(path, name)
            else:
                raise InvalidConfig("either `text` or `path` is required", field="text")
            self.finish_json({"import": result}, status=201)

    class RuleHandler(Base):
        @guard
        async def get(self, name: str) -> None:
            self.finish_json({"rules": self.board.rules_show(name)})

        @guard
        async def delete(self, name: str) -> None:
            self.board.rules_remove(name)
            self.finish_json({"deleted": name})

    class ActiveHandler(Base):
        @guard
        async def get(self) -> None:
            registry = self.board.registry
            self.finish_json({"active": registry.active})

        @guard
        async def put(self) -> None:
            registry = self.board.registry
            payload = self.body_json()
            env = registry.activate(str(payload.get("name") or ""))
            self.finish_json({"active": env.name})

    class MappingsHandler(Base):
        @guard
        async def get(self) -> None:
            self.require_service()
            raw_env = self.get_argument("env", default="")
            # env="*" 表示不按环境过滤 —— 多环境对比时看全量
            env = None if raw_env == "*" else (raw_env or self.board.registry.active)
            raw_limit = self.get_argument("limit", default="500")
            try:
                limit = int(raw_limit)
            except ValueError:
                raise InvalidConfig(
                    f"`limit` must be an integer, got {raw_limit!r}", field="limit"
                ) from None
            rows = self.board.index.snapshot(
                env=env,
                query=self.get_argument("q", default=""),
                source=self.get_argument("source", default="") or None,
                limit=limit,
            )
            self.finish_json(
                {"env": env or "*", "count": len(rows), "mappings": rows}
            )

    class ResolveHandler(Base):
        @guard
        async def post(self) -> None:
            service = self.require_service()
            payload = self.body_json()
            hosts = _as_str_list(payload.get("hosts") or payload.get("host"))
            if not hosts:
                raise InvalidConfig("`hosts` must be a non-empty list", field="hosts")
            env_arg = str(payload.get("env") or "") or None
            if payload.get("all_envs"):
                # 一次拿到该域名在**所有环境**里的解析结果 —— 多环境对比的主场景
                groups = {
                    name: await service.resolve_hosts(hosts, env_name=name)
                    for name in self.board.registry.names()
                }
                self.finish_json({"all_envs": True, "environments": groups})
                return
            results = await service.resolve_hosts(hosts, env_name=env_arg)
            self.finish_json({"results": results})

    class StatusHandler(Base):
        @guard
        async def get(self) -> None:
            service = self.require_service()
            env_arg = self.get_argument("env", default="")
            env = self.board.registry.get(env_arg or self.board.registry.active)
            static = service.static_map(env.name)
            from_env = sum(1 for _, origin in static.values() if origin == "environment")
            self.finish_json(
                {
                    "active": self.board.registry.active,
                    "config_path": self.board._config_path,
                    "servers": service.report_servers(env_arg or None),
                    "index": self.board.index.stats(),
                    "rules": {
                        "dir": self.board.rules_store.dir_path(),
                        "files": len(self.board.rules_store.names()),
                        "active_env": env.name,
                        "bound": env.rules_file,
                        "bound_entries": len(static) - from_env,
                        "static_total": len(static),
                    },
                    "dashboard": {"mounted": self.board._mounted, "prefix": root},
                }
            )

    class RefreshHandler(Base):
        @guard
        async def post(self) -> None:
            service = self.require_service()
            from mitmproxy import ctx as mctx

            payload = self.body_json()
            hosts = _as_str_list(payload.get("hosts"))
            if not hosts:
                hosts = [str(h) for h in mctx.options.envboard_watch_hosts]
            results = await service.resolve_hosts(
                hosts, env_name=str(payload.get("env") or "") or None
            )
            self.finish_json({"refreshed": len(results), "results": results})

    def _as_json(registry, env):  # noqa: ANN001, ANN202
        data = env.to_json()
        data["active"] = env.name == registry.active
        return data

    def _as_str_list(value: Any) -> list[str]:
        if value is None:
            return []
        if isinstance(value, str):
            return [part.strip() for part in value.split(",") if part.strip()]
        if isinstance(value, (list, tuple)):
            return [str(v).strip() for v in value if str(v).strip()]
        return []

    name_re = r"(?P<name>[a-z0-9_-]+)"
    asset_re = r"(?P<name>[A-Za-z0-9_.-]+)"
    return [
        (rf"{root}/?", IndexHandler),
        (rf"{root}/{asset_re}", AssetHandler),
        (rf"{root}/api/environments/?", EnvironmentsHandler),
        (rf"{root}/api/environments/{name_re}/?", EnvironmentHandler),
        (rf"{root}/api/rules/?", RulesHandler),
        (rf"{root}/api/rules/{name_re}/?", RuleHandler),
        (rf"{root}/api/active/?", ActiveHandler),
        (rf"{root}/api/mappings/?", MappingsHandler),
        (rf"{root}/api/resolve/?", ResolveHandler),
        (rf"{root}/api/status/?", StatusHandler),
        (rf"{root}/api/refresh/?", RefreshHandler),
    ]
