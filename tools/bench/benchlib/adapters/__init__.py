"""被测产品适配器注册表。模块按需导入，缺模块时只在用到它时报错。"""
from __future__ import annotations

import importlib

ADAPTER_IDS = ("local", "remotescreen", "rustdesk", "moonlight")


def load(adapter_id: str):
    """返回 benchlib.adapters.<id>.create() 的结果。"""
    if adapter_id not in ADAPTER_IDS:
        raise KeyError(f"unknown adapter: {adapter_id}")
    module = importlib.import_module(f"benchlib.adapters.{adapter_id}")
    return module.create()
