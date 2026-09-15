from __future__ import annotations

import time


class SystemClock:
    """Clock 端口的默认实现。core 只依赖 ``Clock`` 协议，不直接触碰 ``time``。"""

    def now(self) -> float:
        return time.time()


class FrozenClock:
    """测试替身。"""

    def __init__(self, value: float = 1_000_000.0) -> None:
        self.value = value

    def now(self) -> float:
        return self.value

    def advance(self, seconds: float) -> None:
        self.value += seconds
