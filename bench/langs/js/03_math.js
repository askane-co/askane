function sieve_count(limit) {
    const flags = new Array(limit + 1).fill(true);
    flags[0] = false; flags[1] = false;
    for (let p = 2; p * p <= limit; p++) {
        if (flags[p]) {
            for (let m = p * p; m <= limit; m += p) flags[m] = false;
        }
    }
    let count = 0;
    for (let k = 2; k <= limit; k++) if (flags[k]) count++;
    return count;
}

function float_series(steps) {
    let acc = 0.0;
    for (let i = 1; i <= steps; i++) {
        const x = i * 0.001;
        acc += Math.sqrt(x * x + 1.0) + Math.sin(x) * Math.cos(x);
    }
    return Math.trunc(acc * 100.0);
}

function mod_pow(base, exp, modulus) {
    let res = 1n, b = BigInt(base % modulus), e = exp, m = BigInt(modulus);
    while (e > 0) {
        if (e % 2 === 1) res = (res * b) % m;
        b = (b * b) % m;
        e = Math.trunc(e / 2);
    }
    return Number(res);
}

function run_math_bench() {
    let primes = 0;
    for (let r = 0; r < 24; r++) primes += sieve_count(18000);
    const trig = float_series(60000);
    let mod_acc = 0;
    for (let j = 1; j <= 20000; j++) {
        mod_acc = (mod_acc + mod_pow(j + 7, (j % 31) + 5, 1000000007)) % 1000000007;
    }
    return primes + (trig % 100000) + (mod_acc % 100000);
}

const t0 = performance.now();
const checksum = run_math_bench();
const elapsed = performance.now() - t0;
console.log(`BENCH_RESULT 03_math ${elapsed} ${checksum}`);
