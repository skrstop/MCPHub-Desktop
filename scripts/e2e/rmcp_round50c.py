#!/usr/bin/env python3
# 50轮复核二轮套件 C：task 深度 / 严格x task / 伪造头防护 / 通知与订阅过滤 / 边界
# 用法: python3 rmcp_round50c.py [--keep-strict]
import json, http.client, sys, time, sqlite3, os

HOST, PORT = "localhost", 23333
PASS, FAIL = [], []
def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("PASS" if ok else "FAIL"), "|", name, ("| " + str(detail)[:130] if detail and not ok else ""))

def req(method, path, payload=None, headers=None, timeout=90):
    c = http.client.HTTPConnection(HOST, PORT, timeout=timeout)
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if headers: h.update(headers)
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8") if payload is not None else None
    c.request(method, path, body=body, headers=h)
    r = c.getresponse(); data = r.read().decode("utf-8", "replace")
    st = r.status; ct = r.getheader("content-type") or ""
    sid = r.getheader("mcp-session-id")
    c.close()
    obj = None
    try: obj = json.loads(data)
    except Exception:
        for l in data.split("\n"):
            if l.startswith("data: "):
                try: obj = json.loads(l[6:])
                except Exception: pass
    return st, obj, ct, sid, data

META = {"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
    "io.modelcontextprotocol/clientInfo":{"name":"round50c","version":"1"},
    "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}}}}
DB = os.path.expanduser("~/Library/Application Support/app.mcphub.desktop/mcphub.db")

def set_strict(on):
    con = sqlite3.connect(DB)
    cfg = json.loads(con.execute("SELECT config_json FROM system_config WHERE rowid=1").fetchone()[0] or "{}")
    cfg.setdefault("mcp", {})["strictValidation"] = on
    con.execute("UPDATE system_config SET config_json=?, updated_at=datetime('now') WHERE rowid=1",
                (json.dumps(cfg, ensure_ascii=False),))
    con.commit(); con.close()

def call_ip_task(name="本机公网ip查询-getPublicIp", ttl=60000, extra_headers=None, with_task=True):
    hdr = dict(extra_headers or {})
    hdr.setdefault("MCP-Protocol-Version", "2026-07-28")
    params = {"name":name,"arguments":{}, **META}
    if with_task: params["task"] = {"ttl":ttl}
    st, obj, ct, sid, raw = req("POST", "/mcp", {"jsonrpc":"2.0","id":1,"method":"tools/call",
        "params":params}, hdr)
    return st, obj

# ============ T1: 严格模式 x task（标记翻译在严格下同样生效）============
print("== T1 严格 x task ==")
set_strict(True)
try:
    hdr_strict = {"MCP-Protocol-Version":"2026-07-28",
        "Mcp-Method":"tools/call", "Mcp-Name":"=?base64?" + __import__("base64").b64encode("本机公网ip查询-getPublicIp".encode()).decode() + "?="}
    st, obj = call_ip_task(extra_headers=hdr_strict)
    tid = (obj or {}).get("result", {}).get("taskId")
    check("T1.1 严格模式下 task 创建仍生效", st==200 and obj and obj.get("result",{}).get("resultType")=="task" and tid,
          f"st={st} {str(obj)[:100]}")
    # 轮询终态（严格模式：SEP-2243 头须齐，Mcp-Name=taskId）
    got_result = False
    for _ in range(30):
        time.sleep(0.5)
        st2, obj2, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tasks/result",
            "params":{"taskId":tid, **META}},
            {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tasks/result","Mcp-Name":tid})
        res = (obj2 or {}).get("result", {})
        if st2==200 and isinstance(res, dict) and (res.get("status") in ("completed","failed","cancelled")
            or (res.get("resultType") == "complete" and res.get("content"))):
            got_result = res.get("resultType") == "complete" or res.get("status") == "completed"
            content = json.dumps(res)
            check("T1.2 任务终态 completed", got_result, f"res={str(res)[:120]}")
            check("T1.3 任务结果含真实IP内容", "Success" in content or "121." in content, content[:80])
            break
    else:
        check("T1.2 任务终态 completed", False, "poll timeout")
    # 严格模式正常规范请求不受影响（防标记翻译破坏 406 语义）
    st3, obj3, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"initialize",
        "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}},
        {"Accept":"application/json"})  # 单报 Accept，严格应 406
    check("T1.4 严格下坏 Accept 仍 406（翻译不越权）", st3==406, f"st={st3}")
finally:
    set_strict(False)

# ============ T2: 伪造头防护 ============
print("== T2 伪造头防护 ==")
st, obj = call_ip_task(extra_headers={"x-mcphub-task-requested": '{"ttl":60000}'}, with_task=False)
check("T2.1 body 无 task 时伪造头被剥离→同步执行",
      st==200 and obj and "result" in obj and obj["result"].get("resultType") != "task"
      and obj["result"].get("isError") is not True, f"st={st} {str(obj)[:120]}")

# ============ T3: 任务取消 + 未知 id + tasks/list ============
print("== T3 任务生命周期 ==")
st, obj = call_ip_task()
tid = (obj or {}).get("result", {}).get("taskId")
check("T3.1 创建返回 resultType:task + taskId",
      (obj or {}).get("result",{}).get("resultType")=="task" and bool(tid), str(obj)[:100])
# tasks/get
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tasks/get",
    "params":{"taskId":tid, **META}})
res = (obj or {}).get("result", {})
check("T3.2 tasks/get working", isinstance(res, dict) and res.get("status") in ("working","completed"), str(res)[:100])
# tasks/cancel
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":3,"method":"tasks/cancel",
    "params":{"taskId":tid, **META}})
check("T3.3 tasks/cancel 可调用", st==200 and "error" not in (obj or {}), f"st={st} {str(obj)[:100]}")
# tasks/list 含该 id
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":4,"method":"tasks/list","params":META})
tasks_list = json.dumps(obj)
check("T3.4 tasks/list 含刚创建的任务", tid in tasks_list, f"len={len(tasks_list)}")
# 未知 id
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":5,"method":"tasks/get",
    "params":{"taskId":"no-such-task", **META}})
check("T3.5 未知 taskId 报错（非挂起）", (obj or {}).get("error") is not None, str(obj)[:100])

# ============ T4: scope 通道 task ============
print("== T4 scope 通道 task ==")
from urllib.parse import quote
base = "/mcp/" + quote("本机公网ip查询")
st, obj, ct, sid, raw = req("POST", base, {"jsonrpc":"2.0","id":1,"method":"tools/call",
    "params":{"name":"getPublicIp","arguments":{},"task":{"ttl":60000}, **META}})
tid4 = (obj or {}).get("result", {}).get("taskId")
check("T4.1 单服务器 scope task 创建", (obj or {}).get("result",{}).get("resultType")=="task" and bool(tid4), f"st={st} {str(obj)[:100]}")
if tid4:
    ok_done = False
    for _ in range(30):
        time.sleep(0.5)
        st2, obj2, _, _, _ = req("POST", base, {"jsonrpc":"2.0","id":2,"method":"tasks/result",
            "params":{"taskId":tid4, **META}})
        res = (obj2 or {}).get("result", {})
        if isinstance(res, dict) and (res.get("status") in ("completed","failed","cancelled")
            or res.get("resultType") == "complete" and res.get("content")):
            ok_done = res.get("resultType") == "complete" or res.get("status") == "completed"
            break
    check("T4.2 scope 通道任务终态 completed", ok_done, str(res)[:120])

# ============ T5: 通知回归 + 订阅过滤 ============
print("== T5 通知/订阅 ==")
st, obj, _, sid, raw = req("POST", "/mcp", {"jsonrpc":"2.0","method":"notifications/initialized",
    "params":{}}, {"Accept":"application/json, text/event-stream"})
check("T5.1 无 id 通知 POST 不挂不 5xx", st in (200,202,204,400,404,422), f"st={st}")
# 订阅 ack honored filter 内容
c = http.client.HTTPConnection(HOST, PORT, timeout=20)
c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":11,"method":"subscriptions/listen",
    "params":{"notifications":{"promptsListChanged":True,"resourcesListChanged":True,"toolsListChanged":False}, **META}}).encode(),
    headers={"Content-Type":"application/json","Accept":"text/event-stream"})
r = c.getresponse()
acc = ""
for _ in range(10):
    acc += r.readline().decode("utf-8","replace")
    if "acknowledged" in acc: break
c.close()
ack_ok = ("acknowledged" in acc and '"promptsListChanged":true' in acc
          and '"resourcesListChanged":true' in acc and '"toolsListChanged"' not in acc)
check("T5.2 订阅 ack honored 过滤精确（未订阅类型不出现）", ack_ok, acc[:140])
# 资源订阅 ack
c = http.client.HTTPConnection(HOST, PORT, timeout=20)
c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":12,"method":"subscriptions/listen",
    "params":{"notifications":{"resourceSubscriptions":["file:///a.txt"]}, **META}}).encode(),
    headers={"Content-Type":"application/json","Accept":"text/event-stream"})
r = c.getresponse()
acc2 = ""
for _ in range(10):
    acc2 += r.readline().decode("utf-8","replace")
    if "acknowledged" in acc2: break
c.close()
check("T5.3 resourceSubscriptions ack 回显 URI", "file:///a.txt" in acc2, acc2[:140])

# ============ T6: 边界 ============
print("== T6 边界 ==")
# 非 ASCII 数字 id 保留
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":"str-id-中文","method":"ping","params":META})
check("T6.1 string id（含中文）回显保留", (obj or {}).get("id") == "str-id-中文", str(obj)[:100])
# 缺 Content-Type（宽松放行）
c = http.client.HTTPConnection(HOST, PORT, timeout=30)
c.request("POST", "/mcp", body=json.dumps({"jsonrpc":"2.0","id":1,"method":"initialize",
    "params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"m","version":"1"}}}).encode(),
    headers={"Accept":"application/json, text/event-stream"})
r = c.getresponse(); st = r.status; r.read(); c.close()
check("T6.2 缺 Content-Type 宽松放行", st==200, f"st={st}")
# 错误 Mcp-Method：宽松模式以 body 派生值覆写（"信任 body"原则，错误残留头被纠正放行）；
# 严格模式不受影响（中间件直通，rmcp 仍 -32020，由 strict_matrix 守护）。
st, obj, _, _, _ = req("POST", "/mcp", {"jsonrpc":"2.0","id":2,"method":"tools/list","params":META},
    {"MCP-Protocol-Version":"2026-07-28","Mcp-Method":"resources/list"})
check("T6.3 错误 Mcp-Method 宽松覆写放行（tools/list 成功）", st==200 and (obj or {}).get("result") is not None, f"st={st} {str(obj)[:100]}")
# 并发 task 创建
import threading
ids = []
def mk():
    s, o = call_ip_task()
    t = (o or {}).get("result", {}).get("taskId")
    if t: ids.append(t)
threads = [threading.Thread(target=mk) for _ in range(5)]
[t.start() for t in threads]; [t.join() for t in threads]
check("T6.4 并发 5 任务创建全部独立 id", len(set(ids))==5, f"n={len(set(ids))}")

print(f"\n== {len(PASS)}/{len(PASS)+len(FAIL)} passed ==")
if FAIL:
    print("FAILED:"); [print(" -", f) for f in FAIL]
sys.exit(1 if FAIL else 0)
