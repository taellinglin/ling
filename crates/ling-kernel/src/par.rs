//! Data-parallel helpers over the x86-64 worker cores (`arch::x86_64::smp`).
//! On the Raspberry Pi (whose other cores stay parked) everything runs in
//! order on the boot core, so callers never branch on the platform.
//!
//! Job closures run on other cores: they must not allocate, print, or call
//! into the Ling runtime, the scheduler or a driver. Pure pixel work only.

/// Cores in use for parallel work (1 = none besides the boot core).
pub fn cores() -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::smp::cores()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        1
    }
}

/// `f(0) .. f(n-1)`, spread over the working cores; returns when all are done.
#[inline]
pub fn for_each<F: Fn(usize) + Sync>(n: usize, f: &F) {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::smp::parallel_for(n, f)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        for i in 0..n {
            f(i);
        }
    }
}

/// Split `rows` rows into chunks for [`for_each`]: a few per core so a busy
/// band doesn't leave the others idle. Returns (chunks, rows per chunk).
pub fn row_chunks(rows: usize, min_rows: usize) -> (usize, usize) {
    let c = cores();
    if c <= 1 || rows < min_rows * 2 {
        return (1, rows.max(1));
    }
    let want = (c * 4).min(rows / min_rows.max(1)).max(1);
    let per = rows.div_ceil(want);
    (rows.div_ceil(per), per)
}

/// A raw pointer that may be shared with the worker cores. Each job must only
/// touch the part of the memory that is its own (disjoint rows).
#[derive(Clone, Copy)]
pub struct Shared<T>(pub *mut T);
unsafe impl<T> Sync for Shared<T> {}
unsafe impl<T> Send for Shared<T> {}

impl<T> Shared<T> {
    /// The pointer (a method, so closures capture the whole `Sync` wrapper,
    /// not the raw field).
    #[inline(always)]
    pub fn get(&self) -> *mut T {
        self.0
    }
}

/// The `cores` command (console / SSH): show, or limit, the cores used.
pub fn command(arg: &str) -> alloc::string::String {
    use alloc::format;
    #[cfg(target_arch = "x86_64")]
    {
        let a = arg.trim();
        if let Ok(n) = a.parse::<usize>() {
            let got = crate::arch::smp::set_cores(n);
            return format!("cores: parallel work now uses {got} of {} cores\n", crate::arch::smp::cores_online());
        }
        let (jobs, us, by) = crate::arch::smp::stats();
        let mut s = format!(
            "cores: {} in use of {} online; {} parallel jobs so far, {:.1} ms inside them\n  tasks per core:",
            crate::arch::smp::cores(),
            crate::arch::smp::cores_online(),
            jobs,
            us as f64 / 1000.0
        );
        for (i, t) in by.iter().enumerate().take(crate::arch::smp::cores_online()) {
            s.push_str(&format!(" #{i}={t}"));
        }
        s.push_str("\n  `cores N` limits parallel work to N cores (1 = boot core only)\n");
        s
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = arg;
        format!("cores: 1 (the other cores are parked on this board)\n")
    }
}
