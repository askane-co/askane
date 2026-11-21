import json
import time

def make_bar(val, max_val, width):
    filled = (val * width) // max_val
    out = "["
    i = 0
    while i < width:
        out += "#" if i < filled else "."
        i += 1
    return out + "]"

def run_infographic_bench():
    cats = ["compute", "io", "network", "render", "script", "cache"]
    raw_rows = []
    i = 0
    while i < 600:
        c = cats[i % 6]
        val = 10 + ((i * 37 + 13) % 90)
        ok = (i % 5) != 0
        raw_rows.append({"cat": c, "score": val, "ok": ok, "rank": i % 11})
        i += 1

    payload = json.dumps({"title": "Askane Metrics", "count": len(raw_rows), "rows": raw_rows}, separators=(",", ":"))
    totals = {"compute": 0, "io": 0, "network": 0, "render": 0, "script": 0, "cache": 0}
    counts = {"compute": 0, "io": 0, "network": 0, "render": 0, "script": 0, "cache": 0}
    bars_len = 0

    rnd = 0
    while rnd < 320:
        decoded = json.loads(payload)
        rows = decoded["rows"]
        idx = 0
        n = len(rows)
        while idx < n:
            r = rows[idx]
            if r["ok"]:
                k = r["cat"]
                totals[k] = (totals[k] + r["score"]) % 1000000
                counts[k] += 1
            idx += 1
        c_idx = 0
        while c_idx < 6:
            cname = cats[c_idx]
            avg = (totals[cname] % 100) + 1
            bar = cname + ": " + make_bar(avg, 100, 20) + " " + str(avg)
            bars_len += len(bar)
            c_idx += 1
        rnd += 1

    sum_all = totals["compute"] + totals["io"] + totals["network"] + totals["render"] + totals["script"] + totals["cache"]
    return sum_all * 100 + bars_len + len(payload)

t0 = time.perf_counter()
checksum = run_infographic_bench()
elapsed = (time.perf_counter() - t0) * 1000.0
print(f"BENCH_RESULT 02_infographic {elapsed} {checksum}")
