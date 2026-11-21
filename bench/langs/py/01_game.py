import time

def abs_i(x):
    return -x if x < 0 else x

def run_game_bench():
    entity_count = 64
    frames = 280
    entities = []
    seed = 1337

    i = 0
    while i < entity_count:
        seed = (seed * 1103515245 + 12345) % 2147483647
        px = seed % 800
        seed = (seed * 1103515245 + 12345) % 2147483647
        py = seed % 600
        vx = (i % 7) - 3
        if vx == 0:
            vx = 2
        vy = ((i * 3) % 7) - 3
        if vy == 0:
            vy = -2
        entities.append({"id": i, "x": px, "y": py, "vx": vx, "vy": vy, "hp": 100, "cd": 0})
        i += 1

    collisions = 0
    total_dmg = 0
    f = 0
    while f < frames:
        a = 0
        while a < entity_count:
            e = entities[a]
            nx = e["x"] + e["vx"]
            ny = e["y"] + e["vy"]
            if nx < 0 or nx > 800:
                e["vx"] = -e["vx"]
                nx = e["x"] + e["vx"]
            if ny < 0 or ny > 600:
                e["vy"] = -e["vy"]
                ny = e["y"] + e["vy"]
            e["x"] = nx
            e["y"] = ny
            if e["cd"] > 0:
                e["cd"] -= 1
            a += 1

        j = 0
        while j < entity_count:
            e1 = entities[j]
            k = j + 1
            while k < entity_count:
                e2 = entities[k]
                dx = e1["x"] - e2["x"]
                dy = e1["y"] - e2["y"]
                if abs_i(dx) < 28 and abs_i(dy) < 28:
                    if dx * dx + dy * dy < 784:
                        collisions += 1
                        if e1["cd"] == 0:
                            dmg = 3 + ((f + j + k) % 5)
                            e2["hp"] -= dmg
                            if e2["hp"] <= 0:
                                e2["hp"] = 100
                            e1["cd"] = 4
                            total_dmg += dmg
                k += 1
            j += 1
        f += 1

    pos_sum = 0
    for e in entities:
        pos_sum += e["x"] + e["y"] + e["hp"]
    return collisions * 100000 + total_dmg * 100 + (pos_sum % 100000)

t0 = time.perf_counter()
checksum = run_game_bench()
elapsed = (time.perf_counter() - t0) * 1000.0
print(f"BENCH_RESULT 01_game {elapsed} {checksum}")
