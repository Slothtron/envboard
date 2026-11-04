#!/usr/bin/env python3
# 绿色免安装发行包的**唯一打包入口**：`python scripts/package.py`（或 `mise run
# package-portable`）。
#
# 与 `scripts/verify.py` 同一纪律：只做编排，只用标准库，跨平台（Linux / macOS /
# Windows 原生），缺工具就响亮失败而不是静默跳过。
#
# 用法：
#   python scripts/package.py                # 常规 release 构建（依赖 MSVC 运行库）
#   python scripts/package.py --static-crt   # 静态链接 CRT，去掉 VCRUNTIME140.dll 依赖
#
# `--static-crt` 用**独立的目标目录**（`target/crt-static`）：RUSTFLAGS 一变，整棵
# 依赖树都要重编，混用同一个 `target/` 会让两种构建互相打穿缓存。实测代价是首次多
# 3~4 分钟构建、产物大 174 KB（34.6 → 34.8 MB），换来"目标机器不需要装 VC++ 运行库"。
#
# 产物长这样（以 Windows 为例）：
#
#   dist/envboard-0.3.0-windows-x64-portable/
#   ├── envboard.exe          单文件二进制（release，strip + thin LTO）
#   ├── envboard.portable     便携标记 —— 名字必须与
#   │                         crates/host/src/options.rs 的常量一致
#   └── README.txt            双击前先看的分发说明
#   dist/envboard-0.3.0-windows-x64-portable.zip
#
# 便携模式的判据在 `crates/host/src/options.rs`：exe 同目录存在
# `<应用名>.portable` 时，状态目录取同目录的 `<应用名>-data`。所以这里**只放一个
# 标记文件**，不乱建数据目录 —— 目录由应用首次运行时自己创建（判据不能依赖一个
# 该由应用创建的路径）。
"""__doc__ 由上方注释承担，模块本体只做编排。"""

import os
import platform
import shutil
import subprocess
import sys
import tomllib
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST = os.path.join(ROOT, "dist")

# 便携模式的形状（与 crates/host/src/options.rs 的两个常量一致）。
MARKER_SUFFIX = ".portable"
DATA_SUFFIX = "-data"

# 分发说明。内容刻意写"用户拿到的这个目录"该怎么用，而不是本仓怎么构建。
README_TXT = """\
envboard —— 绿色免安装版
=========================

用法
----
双击 envboard.exe 即可。首次运行会在**本目录**创建 envboard-data/，
里面是账本、日志、请求轨迹、CA 证书与单实例锁。

便携约定
--------
* 整个目录可以整体搬走（U 盘、另一台机器、改名都行），数据跟着走。
* 删除本目录即卸载：没有注册表项、没有系统目录、没有后台服务。
* 同一个副本只能开一个窗口；不同目录的副本互不影响。
* 想临时把数据放到别处：set ENVBOARD_STATE_DIR=D:\\somewhere（环境变量优先于
  本目录的便携标记 envboard.portable）。

系统要求
--------
Windows 10 1903 或更新版本。若提示缺少 VCRUNTIME140.dll，安装一次
Microsoft Visual C++ 2015-2022 可再发行组件即可。

未签名
------
本程序没有代码签名，首次运行可能出现 SmartScreen 提示（"Windows 已保护你的
电脑"）：点「更多信息」→「仍要运行」。
"""


def tool(name: str) -> str:
    """解析可执行文件绝对路径；缺工具**响亮失败**（跳过等于发出一个坏包）。"""
    path = shutil.which(name)
    if path is None:
        sys.exit(f"!!! package: 未找到 {name} —— 出路：mise install 或用系统包管理器安装后重跑")
    return path


def run(name: str, argv: list[str], env: dict[str, str] | None = None) -> None:
    print(f"=== {name} ===")
    proc = subprocess.run(argv, cwd=ROOT, env=env)
    if proc.returncode != 0:
        sys.exit(f"!!! {name} FAILED（退出码 {proc.returncode}）")


def workspace_version() -> str:
    """版本号的唯一来源：根 `Cargo.toml` 的 `[workspace.package] version`。"""
    with open(os.path.join(ROOT, "Cargo.toml"), "rb") as handle:
        manifest = tomllib.load(handle)
    return manifest["workspace"]["package"]["version"]


def bin_name() -> str:
    """二进制名（= 便携标记与数据目录名的前缀）取自 `[[bin]] name`，不硬编码。"""
    with open(os.path.join(ROOT, "crates", "desktop", "Cargo.toml"), "rb") as handle:
        manifest = tomllib.load(handle)
    bins = manifest.get("bin") or []
    if not bins or "name" not in bins[0]:
        sys.exit("!!! package: crates/desktop/Cargo.toml 里没有 [[bin]] name")
    return bins[0]["name"]


def platform_tag() -> str:
    system = {"Windows": "windows", "Darwin": "macos", "Linux": "linux"}.get(
        platform.system()
    )
    if system is None:
        sys.exit(f"!!! package: 不认识的平台 {platform.system()}")
    machine = platform.machine().lower()
    arch = {
        "x86_64": "x64",
        "amd64": "x64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }.get(machine)
    if arch is None:
        sys.exit(f"!!! package: 不认识的架构 {platform.machine()}")
    return f"{system}-{arch}"


def build(static_crt: bool) -> str:
    """跑 release 构建，返回**目标目录**（`--locked --offline` 与验证门禁同口径）。

    静态 CRT 用独立目标目录：RUSTFLAGS 一变整棵依赖树重编，与常规构建共用
    `target/` 只会让两边互相打穿缓存。
    """
    argv = [tool("cargo"), "build", "--release", "--locked", "--offline"]
    if not static_crt:
        run("build", argv)
        return os.path.join(ROOT, "target")

    target = os.path.join(ROOT, "target", "crt-static")
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = target
    flags = env.get("RUSTFLAGS", "").strip()
    env["RUSTFLAGS"] = f"{flags} -C target-feature=+crt-static".strip()
    run("build (static crt)", argv, env)
    return target


def stage(bin_path: str, app: str, name: str) -> str:
    """组装发行目录：一个二进制 + 一个标记 + 一份说明。"""
    out = os.path.join(DIST, name)
    # 幂等：上次的残留（可能带着旧的 envboard-data）不能混进新包。
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(out)

    shutil.copy2(bin_path, os.path.join(out, os.path.basename(bin_path)))
    # 标记内容写清用途：它是数据文件，被打包/转发时也能看懂，且**非空**
    # （某些解压工具会丢掉零字节文件）。
    with open(os.path.join(out, app + MARKER_SUFFIX), "w", encoding="utf-8") as handle:
        handle.write(
            "envboard 便携模式标记：存在本文件时，数据落在同目录的 "
            f"{app}{DATA_SUFFIX}/。删除本文件即回到平台默认状态目录。\n"
        )
    with open(os.path.join(out, "README.txt"), "w", encoding="utf-8") as handle:
        handle.write(README_TXT)
    return out


def zip_dir(path: str) -> str:
    archive = path + ".zip"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as bundle:
        for dirpath, _dirnames, filenames in os.walk(path):
            for filename in sorted(filenames):
                source = os.path.join(dirpath, filename)
                arcname = os.path.relpath(source, os.path.dirname(path))
                bundle.write(source, arcname)
    return archive


def main() -> int:
    args = sys.argv[1:]
    unknown = [arg for arg in args if arg != "--static-crt"]
    if unknown:
        print(f"unknown argument: {' '.join(unknown)}", file=sys.stderr)
        print("用法：python scripts/package.py [--static-crt]", file=sys.stderr)
        return 2
    static_crt = "--static-crt" in args

    app = bin_name()
    version = workspace_version()
    tag = platform_tag()
    name = f"{app}-{version}-{tag}-portable" + ("-static" if static_crt else "")

    target = build(static_crt)

    bin_file = app + (".exe" if platform.system() == "Windows" else "")
    bin_path = os.path.join(target, "release", bin_file)
    if not os.path.isfile(bin_path):
        sys.exit(f"!!! package: 构建声称成功，但产物不在 {bin_path}")

    out = stage(bin_path, app, name)
    archive = zip_dir(out)

    print()
    print(f"目录：{os.path.relpath(out, ROOT)}")
    print(f"压缩包：{os.path.relpath(archive, ROOT)} "
          f"（{os.path.getsize(archive) / (1024 * 1024):.1f} MB）")
    print("说明：解压即用；数据落在解压目录的 envboard-data/，删目录即卸载。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
