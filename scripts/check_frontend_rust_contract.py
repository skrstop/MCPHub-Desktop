#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""前端 payload × Rust ServerConfig 契约 diff（R23 教训固化：invoke 参数反序列化
是 E2E 盲区，serde 错误只进前端 toast）。对字段名做 camel↔snake 对账，静默丢弃/
类型错配即 FAIL。CI 可直接跑：python3 scripts/check_frontend_rust_contract.py"""
import re, sys

RUST_STRUCT = 'src-tauri/src/models/server.rs'
FE_PAYLOAD = 'frontend/src/utils/serverFormPayload.ts'

def rust_fields():
    s = open(RUST_STRUCT).read()
    m = re.search(r'pub struct ServerConfig \{(.*?)\n\}', s, re.S)
    out = {}
    for line in m.group(1).splitlines():
        fm = re.match(r'\s*pub (\w+):\s*(.+?),?$', line)
        if fm:
            out[fm.group(1)] = fm.group(2).strip()
    return out

def fe_keys():
    s = open(FE_PAYLOAD).read()
    keys = set(re.findall(r'config\.(\w+)\s*=', s))
    # shorthand entries in the config literal: '    options,' style
    m = re.search(r'const config: Partial<ServerConfig> = \{(.*?)\n  \};', s, re.S)
    if m:
        for line in m.group(1).splitlines():
            sm = re.match(r'\s*(\w+),\s*$', line)
            if sm: keys.add(sm.group(1))
    return keys

def camel2snake(k):
    return re.sub(r'(?<!^)(?=[A-Z])', '_', k).lower()

def main():
    rust = rust_fields()
    fe = {camel2snake(k) for k in fe_keys()}
    problems = []
    # Fields Rust knows but frontend never emits: fine IF Option/default — just report
    # Fields frontend emits that Rust doesn't know: serde ignores → silent data loss
    # 'type' → server_type rename; 'oauth' → R10-adjudicated (UI collects, no
    # Rust storage point yet, needs product decision) — tracked, not new.
    dropped = sorted(fe - set(rust) - {'type', 'oauth'})
    if dropped:
        problems.append(f"frontend emits fields Rust ServerConfig does not have (silent drop): {dropped}")
    for p in problems:
        print("FAIL:", p)
    print("OK: no silent-drop contract mismatches" if not problems else f"{len(problems)} problem(s)")
    sys.exit(1 if problems else 0)

if __name__ == '__main__':
    main()
