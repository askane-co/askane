use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "random",
        &[
            ("seed", rand_seed),
            ("int", rand_int),
            ("float", rand_float),
            ("bool", rand_bool),
            ("choice", rand_choice),
            ("shuffle", rand_shuffle),
            ("crypto_int", rand_crypto_int),
            ("crypto_bytes", rand_crypto_bytes),
        ],
        &[],
    )
}

thread_local! {
    static RNG_STATE: Cell<u64> = Cell::new(default_seed());
}

fn default_seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    nanos ^ 0xA076_1D64_78BD_642F
}

fn next_u64() -> u64 {
    RNG_STATE.with(|cell| {
        let mut x = cell.get();
        if x == 0 {
            x = 0x9E37_79B9_7F4A_7C15;
        }
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        cell.set(x);
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    })
}

#[cfg(windows)]
fn os_random_bytes(buf: &mut [u8]) -> Result<(), String> {
    use std::ffi::c_void;
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            hAlgorithm: *mut c_void,
            pbBuffer: *mut u8,
            cbBuffer: u32,
            dwFlags: u32,
        ) -> i32;
    }
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status != 0 {
        return Err(format!("BCryptGenRandom failed with status {:#x}", status));
    }
    Ok(())
}

#[cfg(not(windows))]
fn os_random_bytes(buf: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").map_err(|e| e.to_string())?;
    f.read_exact(buf).map_err(|e| e.to_string())
}

fn rand_seed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("random.seed() expects 1 integer argument".to_string());
    }
    match &args[0] {
        Value::Int(s) => {
            RNG_STATE.with(|c| c.set((*s as u64).max(1)));
            Ok(Value::Unit)
        }
        _ => Err("random.seed() expects an integer".to_string()),
    }
}

fn rand_range_i64(min: i64, max: i64, val: u64) -> Result<i64, String> {
    if min >= max {
        return Err("random range requires min < max".to_string());
    }
    let span = (max as i128 - min as i128) as u128;
    let offset = (val as u128) % span;
    Ok((min as i128 + offset as i128) as i64)
}

fn crypto_range_i64(min: i64, max: i64) -> Result<i64, String> {
    if min >= max {
        return Err("random range requires min < max".to_string());
    }
    let span = (max as i128 - min as i128) as u128;
    // Rejection sampling over the full 2^64 value space. Using MAX % span
    // directly would reject the wrong number of values for even spans.
    let threshold = (0u64.wrapping_sub(span as u64)) % span as u64;
    loop {
        let mut buf = [0u8; 8];
        os_random_bytes(&mut buf)?;
        let value = u64::from_le_bytes(buf);
        if value >= threshold {
            return Ok((min as i128 + (value as u128 % span) as i128) as i64);
        }
    }
}

fn rand_int(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (min, max) = match args {
        [Value::Int(max)] => (0, *max),
        [Value::Int(min), Value::Int(max)] => (*min, *max),
        _ => return Err("random.int() expects (max) or (min, max) integers".to_string()),
    };
    Ok(Value::Int(rand_range_i64(min, max, next_u64())?))
}

fn rand_float(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("random.float() expects 0 arguments".to_string());
    }
    let v = (next_u64() >> 11) as f64 / ((1u64 << 53) as f64);
    Ok(Value::Float(v))
}

fn rand_bool(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("random.bool() expects 0 arguments".to_string());
    }
    Ok(Value::Bool((next_u64() & 1) == 1))
}

fn rand_choice(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("random.choice() expects 1 list argument".to_string());
    }
    match &args[0] {
        Value::List(l) => {
            let items = l.borrow();
            if items.is_empty() {
                return Err("random.choice() called on empty list".to_string());
            }
            let idx = (next_u64() as usize) % items.len();
            Ok(items[idx].clone())
        }
        other => Err(format!(
            "random.choice() expects a list, got '{}'",
            other.type_name()
        )),
    }
}

fn rand_shuffle(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("random.shuffle() expects 1 list argument".to_string());
    }
    match &args[0] {
        Value::List(l) => {
            let mut items = l.borrow().clone();
            for i in (1..items.len()).rev() {
                let j = (next_u64() as usize) % (i + 1);
                items.swap(i, j);
            }
            Ok(Value::List(std::rc::Rc::new(std::cell::RefCell::new(
                items,
            ))))
        }
        other => Err(format!(
            "random.shuffle() expects a list, got '{}'",
            other.type_name()
        )),
    }
}

fn rand_crypto_int(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (min, max) = match args {
        [Value::Int(max)] => (0, *max),
        [Value::Int(min), Value::Int(max)] => (*min, *max),
        _ => return Err("random.crypto_int() expects (max) or (min, max) integers".to_string()),
    };
    Ok(Value::Int(crypto_range_i64(min, max)?))
}

fn rand_crypto_bytes(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("random.crypto_bytes() expects 1 integer argument (count)".to_string());
    }
    let count = match &args[0] {
        Value::Int(n) if *n >= 0 && *n <= 65536 => *n as usize,
        _ => return Err("random.crypto_bytes() count must be between 0 and 65536".to_string()),
    };
    vm.charge_alloc(count)?;
    vm.charge_work((count as u64) / 32 + 1)?;
    let mut buf = vec![0u8; count];
    if count > 0 {
        os_random_bytes(&mut buf)?;
    }
    Ok(Value::Bytes(std::rc::Rc::new(buf)))
}
