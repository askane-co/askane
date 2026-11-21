import time

def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

def make_multiplier(factor):
    return lambda x: x * factor

def eval_op(op, a, b):
    match op:
        case 0:
            return a + b
        case 1:
            return a - b if a >= b else b - a
        case 2:
            return a * ((b % 13) + 1)
        case 3:
            return a if b == 0 else a // ((b % 7) + 1)
        case _:
            return a

def run_scripting_bench():
    fib_sum = fib(29) + fib(27)

    mul3 = make_multiplier(3)
    mul5 = make_multiplier(5)
    dispatch_acc = 0
    i = 0
    while i < 120000:
        v = eval_op(i % 4, i, i + 3)
        if i % 2 == 0:
            dispatch_acc = (dispatch_acc + mul3(v % 100)) % 1000000007
        else:
            dispatch_acc = (dispatch_acc + mul5(v % 100)) % 1000000007
        i += 1

    env_map = {"alpha": 11, "beta": 22, "gamma": 33, "delta": 44}
    keys_list = ["alpha", "beta", "gamma", "delta"]
    kv_acc = 0
    step = 0
    while step < 75000:
        k = keys_list[step % 4]
        cur = env_map[k]
        env_map[k] = (cur + 7) % 10000
        kv_acc = (kv_acc + env_map[k]) % 1000000007
        step += 1

    return (fib_sum + dispatch_acc + kv_acc) % 1000000007

t0 = time.perf_counter()
checksum = run_scripting_bench()
elapsed = (time.perf_counter() - t0) * 1000.0
print(f"BENCH_RESULT 05_scripting {elapsed} {checksum}")
