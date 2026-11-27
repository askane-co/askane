use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub const UNLIMITED_BUDGET: u64 = u64::MAX;

/// Thread-safe resource accounting shared by the VM and future background media/audio workers.
#[derive(Debug)]
pub struct BudgetCore {
    pub limit_bytes: AtomicU64,
    pub used_bytes: AtomicU64,
    pub peak_bytes: AtomicU64,
    pub io_limit: AtomicU64,
    pub io_used: AtomicU64,
    pub work_steps: AtomicU64,
    pub fatal: AtomicBool,
    pub epoch: AtomicU64,
}

impl Default for BudgetCore {
    fn default() -> Self {
        Self::new(Some(64 * 1024 * 1024), Some(128 * 1024 * 1024))
    }
}

impl BudgetCore {
    pub fn new(limit_bytes: Option<u64>, io_limit: Option<u64>) -> Self {
        Self {
            limit_bytes: AtomicU64::new(limit_bytes.unwrap_or(UNLIMITED_BUDGET)),
            used_bytes: AtomicU64::new(0),
            peak_bytes: AtomicU64::new(0),
            io_limit: AtomicU64::new(io_limit.unwrap_or(UNLIMITED_BUDGET)),
            io_used: AtomicU64::new(0),
            work_steps: AtomicU64::new(0),
            fatal: AtomicBool::new(false),
            epoch: AtomicU64::new(1),
        }
    }

    pub fn configure(&self, limit_bytes: Option<u64>, io_limit: Option<u64>) {
        self.limit_bytes
            .store(limit_bytes.unwrap_or(UNLIMITED_BUDGET), Ordering::Release);
        self.io_limit
            .store(io_limit.unwrap_or(UNLIMITED_BUDGET), Ordering::Release);
    }

    pub fn reset(&self, limit_bytes: Option<u64>, io_limit: Option<u64>) {
        self.configure(limit_bytes, io_limit);
        self.used_bytes.store(0, Ordering::Release);
        self.peak_bytes.store(0, Ordering::Release);
        self.io_used.store(0, Ordering::Release);
        self.work_steps.store(0, Ordering::Release);
        self.fatal.store(false, Ordering::Release);
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    #[inline]
    pub fn used_bytes(&self) -> u64 {
        self.used_bytes.load(Ordering::Acquire)
    }

    #[inline]
    pub fn peak_bytes(&self) -> u64 {
        self.peak_bytes.load(Ordering::Acquire)
    }

    #[inline]
    pub fn io_used(&self) -> u64 {
        self.io_used.load(Ordering::Acquire)
    }

    #[inline]
    pub fn work_steps(&self) -> u64 {
        self.work_steps.load(Ordering::Acquire)
    }

    #[inline]
    pub fn limit_bytes_opt(&self) -> Option<u64> {
        match self.limit_bytes.load(Ordering::Acquire) {
            UNLIMITED_BUDGET => None,
            v => Some(v),
        }
    }

    #[inline]
    pub fn io_limit_opt(&self) -> Option<u64> {
        match self.io_limit.load(Ordering::Acquire) {
            UNLIMITED_BUDGET => None,
            v => Some(v),
        }
    }

    pub fn charge_alloc(&self, bytes: u64) -> Result<(), String> {
        let limit = self.limit_bytes.load(Ordering::Acquire);
        let mut cur = self.used_bytes.load(Ordering::Acquire);
        loop {
            let next = cur.saturating_add(bytes);
            if limit != UNLIMITED_BUDGET && next > limit {
                self.fatal.store(true, Ordering::Release);
                return Err(format!(
                    "Sandbox memory budget exceeded (limit {} bytes)",
                    limit
                ));
            }
            match self.used_bytes.compare_exchange_weak(
                cur,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.peak_bytes.fetch_max(next, Ordering::AcqRel);
                    return Ok(());
                }
                Err(actual) => cur = actual,
            }
        }
    }

    pub fn refund_alloc(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let mut cur = self.used_bytes.load(Ordering::Acquire);
        loop {
            let next = cur.saturating_sub(bytes);
            match self.used_bytes.compare_exchange_weak(
                cur,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
    }

    pub fn charge_io(&self, bytes: u64) -> Result<(), String> {
        let limit = self.io_limit.load(Ordering::Acquire);
        let mut cur = self.io_used.load(Ordering::Acquire);
        loop {
            let next = cur.saturating_add(bytes);
            if limit != UNLIMITED_BUDGET && next > limit {
                self.fatal.store(true, Ordering::Release);
                return Err(format!("Sandbox I/O budget exceeded (limit {} bytes)", limit));
            }
            match self.io_used.compare_exchange_weak(
                cur,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => cur = actual,
            }
        }
    }

    #[inline]
    pub fn add_work(&self, steps: u64) {
        if steps > 0 {
            self.work_steps.fetch_add(steps, Ordering::Relaxed);
        }
    }

    pub fn temp_charge(&self, bytes: u64) -> Result<TempCharge<'_>, String> {
        self.charge_alloc(bytes)?;
        Ok(TempCharge {
            core: self,
            charged: bytes,
        })
    }

    pub fn with_temp_charge<F, R>(&self, bytes: u64, f: F) -> Result<R, String>
    where
        F: FnOnce() -> Result<R, String>,
    {
        let _guard = self.temp_charge(bytes)?;
        f()
    }
}

/// RAII guard for transient parsing/decoding allocations that refunds automatically on scope exit.
pub struct TempCharge<'a> {
    core: &'a BudgetCore,
    charged: u64,
}

impl Drop for TempCharge<'_> {
    fn drop(&mut self) {
        self.core.refund_alloc(self.charged);
    }
}
