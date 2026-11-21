function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
function make_multiplier(factor) { return (x) => x * factor; }
function eval_op(op, a, b) {
    switch (op) {
        case 0: return a + b;
        case 1: return a >= b ? a - b : b - a;
        case 2: return a * ((b % 13) + 1);
        case 3: return b === 0 ? a : Math.trunc(a / ((b % 7) + 1));
        default: return a;
    }
}

function run_scripting_bench() {
    const fib_sum = fib(29) + fib(27);
    const mul3 = make_multiplier(3), mul5 = make_multiplier(5);
    let dispatch_acc = 0;
    for (let i = 0; i < 120000; i++) {
        const v = eval_op(i % 4, i, i + 3);
        dispatch_acc = (dispatch_acc + (i % 2 === 0 ? mul3(v % 100) : mul5(v % 100))) % 1000000007;
    }
    const env_map = { alpha: 11, beta: 22, gamma: 33, delta: 44 };
    const keys_list = ["alpha", "beta", "gamma", "delta"];
    let kv_acc = 0;
    for (let step = 0; step < 75000; step++) {
        const k = keys_list[step % 4];
        env_map[k] = (env_map[k] + 7) % 10000;
        kv_acc = (kv_acc + env_map[k]) % 1000000007;
    }
    return (fib_sum + dispatch_acc + kv_acc) % 1000000007;
}

const t0 = performance.now();
const checksum = run_scripting_bench();
const elapsed = performance.now() - t0;
console.log(`BENCH_RESULT 05_scripting ${elapsed} ${checksum}`);
