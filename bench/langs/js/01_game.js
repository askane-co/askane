function abs_i(x) { return x < 0 ? -x : x; }

function run_game_bench() {
    const entity_count = 64, frames = 280, entities = [];
    let seed = 1337;
    for (let i = 0; i < entity_count; i++) {
        seed = (seed * 1103515245 + 12345) % 2147483647;
        const px = seed % 800;
        seed = (seed * 1103515245 + 12345) % 2147483647;
        const py = seed % 600;
        let vx = (i % 7) - 3; if (vx === 0) vx = 2;
        let vy = ((i * 3) % 7) - 3; if (vy === 0) vy = -2;
        entities.push({ id: i, x: px, y: py, vx, vy, hp: 100, cd: 0 });
    }
    let collisions = 0, total_dmg = 0;
    for (let f = 0; f < frames; f++) {
        for (let a = 0; a < entity_count; a++) {
            const e = entities[a];
            let nx = e.x + e.vx, ny = e.y + e.vy;
            if (nx < 0 || nx > 800) { e.vx = -e.vx; nx = e.x + e.vx; }
            if (ny < 0 || ny > 600) { e.vy = -e.vy; ny = e.y + e.vy; }
            e.x = nx; e.y = ny;
            if (e.cd > 0) e.cd--;
        }
        for (let j = 0; j < entity_count; j++) {
            const e1 = entities[j];
            for (let k = j + 1; k < entity_count; k++) {
                const e2 = entities[k];
                const dx = e1.x - e2.x, dy = e1.y - e2.y;
                if (abs_i(dx) < 28 && abs_i(dy) < 28 && dx * dx + dy * dy < 784) {
                    collisions++;
                    if (e1.cd === 0) {
                        const dmg = 3 + ((f + j + k) % 5);
                        e2.hp -= dmg;
                        if (e2.hp <= 0) e2.hp = 100;
                        e1.cd = 4;
                        total_dmg += dmg;
                    }
                }
            }
        }
    }
    let pos_sum = 0;
    for (const e of entities) pos_sum += e.x + e.y + e.hp;
    return collisions * 100000 + total_dmg * 100 + (pos_sum % 100000);
}

const t0 = performance.now();
const checksum = run_game_bench();
const elapsed = performance.now() - t0;
console.log(`BENCH_RESULT 01_game ${elapsed} ${checksum}`);
