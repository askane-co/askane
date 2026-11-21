import math
import time

def sieve_count(limit):
    flags = []
    i = 0
    while i <= limit:
        flags.append(True)
        i += 1
    flags[0] = False
    flags[1] = False

    p = 2
    while p * p <= limit:
        if flags[p]:
            m = p * p
            while m <= limit:
                flags[m] = False
                m += p
        p += 1

    count = 0
    k = 2
    while k <= limit:
        if flags[k]:
            count += 1
        k += 1
    return count

def float_series(steps):
    acc = 0.0
    i = 1
    while i <= steps:
        x = float(i) * 0.001
        acc += math.sqrt(x * x + 1.0) + math.sin(x) * math.cos(x)
        i += 1
    return int(acc * 100.0)

def mod_pow(base, exp, modulus):
    res = 1
    b = base % modulus
    e = exp
    while e > 0:
        if e % 2 == 1:
            res = (res * b) % modulus
        b = (b * b) % modulus
        e = e // 2
    return res

def run_math_bench():
    primes = 0
    r = 0
    while r < 24:
        primes += sieve_count(18000)
        r += 1
    trig = float_series(60000)
    mod_acc = 0
    j = 1
    while j <= 20000:
        mod_acc = (mod_acc + mod_pow(j + 7, (j % 31) + 5, 1000000007)) % 1000000007
        j += 1
    return primes + (trig % 100000) + (mod_acc % 100000)

t0 = time.perf_counter()
checksum = run_math_bench()
elapsed = (time.perf_counter() - t0) * 1000.0
print(f"BENCH_RESULT 03_math {elapsed} {checksum}")
