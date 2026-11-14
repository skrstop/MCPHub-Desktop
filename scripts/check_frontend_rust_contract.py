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
    # 'type' → server_type rename. ('oauth' whitelist removed round 16: Rust
    # ServerConfig has an oauth field since R10, so a dropped key IS a defect.)
    dropped = sorted(fe - set(rust) - {'type'})
    if dropped:
        problems.append(f"frontend emits fields Rust ServerConfig does not have (silent drop): {dropped}")
    problems += check_decision_commands()
    for p in problems:
        print("FAIL:", p)
    print("OK: no silent-drop contract mismatches" if not problems else f"{len(problems)} problem(s)")
    sys.exit(1 if problems else 0)

DECISION_COMMANDS = {
    # command: set of invoke arg keys the frontend must send (snake_case as Rust expects)
    'decision_set_enabled': {'enabled'},
    'decision_select_model': {'model'},
    'decision_download_model': {'model', 'source'},
    'decision_history_list': {'limit', 'offset'},
    'decision_history_get': {'id'},
    'decision_download_status': {'model'},
    'decision_download_cancel': {'model'},
    'decision_decide': {'request'},  # wrapped struct arg
}

def rust_command_params(path='src-tauri/src/commands/decision.rs'):
    s = open(path).read()
    out = {}
    for m in re.finditer(r'#\[tauri::command\]\s*\npub (?:async )?fn (\w+)\((.*?)\)', s, re.S):
        name, params = m.group(1), m.group(2)
        keys = set()
        for pm in re.finditer(r'(\w+)\s*:\s*([^,]+)', params):
            pname, ptype = pm.group(1), pm.group(2)
            if 'State<' in ptype or 'AppHandle' in ptype or pname == 'crate':
                continue  # injected params / generic-path false capture, not invoke args
            keys.add(pname)
        out[name] = keys
    return out

def fe_decision_args(path='frontend/src/utils/tauriClient.ts'):
    # Limitation: an `args: { ...spread }` body evades the key regex below.
    # Registered commands (DECISION_COMMANDS) are still caught by the reverse
    # required-args check; a spread on a zero-arg command would pass silently.
    s = open(path).read()
    out = {}
    for m in re.finditer(r"command:\s*'(decision_\w+)'[\s\S]{0,400}?args:\s*(\{[^}]*\})", s):
        name, body = m.group(1), m.group(2)
        keys = set(re.findall(r"(\w+)\s*:", body))
        out.setdefault(name, set()).update(keys)
    return out

def check_decision_commands():
    """decision 命令面 invoke 参数对账（camel→snake 后必须与 Rust 签名一致）。"""
    rust = rust_command_params()
    fe = fe_decision_args()
    problems = []
    for cmd, keys in fe.items():
        if cmd not in rust:
            problems.append(f"tauriClient invokes unknown decision command: {cmd}")
            continue
        want = {camel2snake(k) for k in keys}
        missing = want - rust[cmd]
        if missing:
            problems.append(f"{cmd}: frontend sends args Rust does not accept: {sorted(missing)} (rust has {sorted(rust[cmd])})")
    # reverse direction: required args (per DECISION_COMMANDS) must be sent
    for cmd, required in DECISION_COMMANDS.items():
        if cmd not in rust:
            continue
        sent = {camel2snake(k) for k in fe.get(cmd, set())}
        absent = required - sent
        if absent:
            problems.append(f"{cmd}: Rust requires args frontend never sends: {sorted(absent)}")
    for cmd in DECISION_COMMANDS:
        if cmd in rust and cmd not in fe:
            problems.append(f"{cmd}: registered in Rust but never invoked from tauriClient (dead surface or missing mapping)")
    return problems

if __name__ == '__main__':
    main()
