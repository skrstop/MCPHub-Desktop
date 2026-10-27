import json

def flatten(d, prefix=""):
    out = {}
    for k, v in d.items():
        key = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            out.update(flatten(v, key))
        else:
            out[key] = v
    return out

en = flatten(json.load(open("locales/en.json")))
zh = flatten(json.load(open("locales/zh.json")))
keys = json.load(open("scripts/untranslated_fr_keys.json"))
for k in keys:
    print(f"KEY: {k}")
    print(f"  EN: {en[k]!r}")
    print(f"  ZH: {zh[k]!r}")
