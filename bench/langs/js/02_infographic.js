function make_bar(val, max_val, width) {
    const filled = Math.trunc((val * width) / max_val);
    let out = "[";
    for (let i = 0; i < width; i++) out += i < filled ? "#" : ".";
    return out + "]";
}

function run_infographic_bench() {
    const cats = ["compute", "io", "network", "render", "script", "cache"];
    const raw_rows = [];
    for (let i = 0; i < 600; i++) {
        raw_rows.push({ cat: cats[i % 6], score: 10 + ((i * 37 + 13) % 90), ok: (i % 5) !== 0, rank: i % 11 });
    }
    const payload = JSON.stringify({ title: "Askane Metrics", count: raw_rows.length, rows: raw_rows });
    const totals = { compute: 0, io: 0, network: 0, render: 0, script: 0, cache: 0 };
    const counts = { compute: 0, io: 0, network: 0, render: 0, script: 0, cache: 0 };
    let bars_len = 0;
    for (let round = 0; round < 320; round++) {
        const rows = JSON.parse(payload).rows;
        for (let idx = 0; idx < rows.length; idx++) {
            const r = rows[idx];
            if (r.ok) {
                totals[r.cat] = (totals[r.cat] + r.score) % 1000000;
                counts[r.cat]++;
            }
        }
        for (let c = 0; c < 6; c++) {
            const cname = cats[c];
            const avg = (totals[cname] % 100) + 1;
            const bar = `${cname}: ${make_bar(avg, 100, 20)} ${avg}`;
            bars_len += bar.length;
        }
    }
    const sum_all = totals.compute + totals.io + totals.network + totals.render + totals.script + totals.cache;
    return sum_all * 100 + bars_len + payload.length;
}

const t0 = performance.now();
const checksum = run_infographic_bench();
const elapsed = performance.now() - t0;
console.log(`BENCH_RESULT 02_infographic ${elapsed} ${checksum}`);
