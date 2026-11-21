function clamp_byte(v) { return v < 0 ? 0 : (v > 255 ? 255 : v); }

function run_visual_bench() {
    const w = 120, h = 72, total_pixels = w * h;
    const buf_r = new Array(total_pixels).fill(0);
    const buf_g = new Array(total_pixels).fill(0);
    const buf_b = new Array(total_pixels).fill(0);
    const frames = 28;
    let pixel_hash = 0;
    for (let frame = 0; frame < frames; frame++) {
        const cx1 = 22 + frame * 3, cy1 = 24 + ((frame % 4) * 2);
        const cx2 = 58 - frame * 2, cy2 = 20 + ((frame % 3) * 3);
        for (let y = 0; y < h; y++) {
            const row_off = y * w;
            for (let x = 0; x < w; x++) {
                let bg_r = (x * 3 + frame * 5) % 256;
                let bg_g = (y * 5 + frame * 3) % 256;
                let bg_b = 40 + ((x + y) % 120);
                const d1 = (x - cx1) ** 2 + (y - cy1) ** 2;
                if (d1 < 196) {
                    const shade = 255 - d1;
                    bg_r = clamp_byte(Math.trunc((bg_r + shade) / 2));
                    bg_g = clamp_byte(Math.trunc((bg_g + Math.trunc(shade / 3)) / 2));
                }
                const d2 = (x - cx2) ** 2 + (y - cy2) ** 2;
                if (d2 < 144) {
                    const alpha = 200 - d2;
                    bg_g = clamp_byte(Math.trunc((bg_g + alpha) / 2));
                    bg_b = clamp_byte(Math.trunc((bg_b + 240) / 2));
                }
                if (x === y || (x + y === 70)) { bg_r = 255; bg_g = 255; bg_b = 255; }
                const idx = row_off + x;
                buf_r[idx] = bg_r; buf_g[idx] = bg_g; buf_b[idx] = bg_b;
            }
        }
        for (let k = 0; k < total_pixels; k += 23) {
            pixel_hash = (pixel_hash * 31 + buf_r[k] * 3 + buf_g[k] * 5 + buf_b[k] * 7) % 1000000007;
        }
    }
    return pixel_hash;
}

const t0 = performance.now();
const checksum = run_visual_bench();
const elapsed = performance.now() - t0;
console.log(`BENCH_RESULT 04_visual ${elapsed} ${checksum}`);
