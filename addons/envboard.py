"""envboard mitmproxy 加载入口。

用法::

    mitmweb  -s addons/envboard.py
    mitmdump -s addons/envboard.py --mode dns

mitmproxy 会把 ``addons/`` 目录临时加入 ``sys.path``。导入包在 ``src/envboard/``，
本仓库在**未安装**的情况下也能直接跑：这里把仓库的 ``src/`` 加进去（仅在包确实存在时）。
"""

from __future__ import annotations

import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(_HERE)
_SRC = os.path.join(_ROOT, "src")
if os.path.isdir(os.path.join(_SRC, "envboard")) and _SRC not in sys.path:
    sys.path.insert(0, _SRC)

from envboard.adapter.addon import EnvBoardAddon  # noqa: E402

#: mitmproxy 通过 ``traverse()`` 递归注册 ``addons`` 列表里的实例。
addons = [EnvBoardAddon()]

__all__ = ["EnvBoardAddon", "addons"]
