import time

def clamp_byte(v):
    if v < 0:
        return 0
    elif v > 255:
        return 255
    return v

def run_visual_bench():
    w = 120
    h = 72
    total_pixels = w * h
    buf_r = []
    buf_g = []
    buf_b = []

    init = 0
    while init < total_pixels:
        buf_r.append(0)
        buf_g.append(0)
        buf_b.append(0)
        init += 1

    frames = 28
    frame = 0
    pixel_hash = 0

    while frame < frames:
        cx1 = 22 + frame * 3
        cy1 = 24 + ((frame % 4) * 2)
        cx2 = 58 - frame * 2
        cy2 = 20 + ((frame % 3) * 3)

        y = 0
        while y < h:
            row_off = y * w
            x = 0
            while x < w:
                bg_r = (x * 3 + frame * 5) % 256
                bg_g = (y * 5 + frame * 3) % 256
                bg_b = 40 + ((x + y) % 120)

                dx1 = x - cx1
                dy1 = y - cy1
                d1 = dx1 * dx1 + dy1 * dy1
                if d1 < 196:
                    shade = 255 - d1
                    bg_r = clamp_byte((bg_r + shade) // 2)
                    bg_g = clamp_byte((bg_g + shade // 3) // 2)

                dx2 = x - cx2
                dy2 = y - cy2
                d2 = dx2 * dx2 + dy2 * dy2
                if d2 < 144:
                    alpha = 200 - d2
                    bg_g = clamp_byte((bg_g + alpha) // 2)
                    bg_b = clamp_byte((bg_b + 240) // 2)

                if x == y or (x + y == 70):
                    bg_r = 255
                    bg_g = 255
                    bg_b = 255

                idx = row_off + x
                buf_r[idx] = bg_r
                buf_g[idx] = bg_g
                buf_b[idx] = bg_b
                x += 1
            y += 1

        k = 0
        while k < total_pixels:
            pixel_hash = (pixel_hash * 31 + buf_r[k] * 3 + buf_g[k] * 5 + buf_b[k] * 7) % 1000000007
            k += 23
        frame += 1

    return pixel_hash

t0 = time.perf_counter()
checksum = run_visual_bench()
elapsed = (time.perf_counter() - t0) * 1000.0
print(f"BENCH_RESULT 04_visual {elapsed} {checksum}")
