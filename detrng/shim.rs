//! Deterministic `getrandom()` for playtest children (Linux, LD_PRELOAD).
//!
//! Compiled by bevy_playtest's build script into a tiny shared library
//! the battery preloads into the game process. It replaces the C
//! library's `getrandom()` with splitmix64 streams seeded from
//! `BEVY_PLAYTEST_DETRNG_SEED`. Callers that reach the OS through libc's
//! `getrandom` symbol are covered: the `getrandom` crate >= 0.3 on glibc
//! (it looks the function up with `dlsym(RTLD_DEFAULT, "getrandom")`,
//! which finds this preloaded definition), hence `rand::rng()` (rand
//! >= 0.9) and ahash's random state. NOT covered: getrandom 0.2 (raw
//! syscall), `/dev/urandom` readers, glibc's `arc4random`.
//!
//! Determinism scope: the MAIN thread gets its own stream, so its values
//! depend only on the seed and the order of its own calls. Under the
//! takeover's single-threaded executor the game's systems run there, so
//! their randomness repeats per seed. Other threads share a second
//! stream whose order depends on thread timing: still seeded, not
//! guaranteed repeatable.
//!
//! `no_std` + `panic = "abort"` so it carries no Rust runtime of its
//! own into the game process.

#![no_std]

use core::sync::atomic::{AtomicU64, Ordering};

unsafe extern "C" {
    fn getenv(name: *const u8) -> *const u8;
    fn getpid() -> i32;
    fn gettid() -> i32;
    fn __errno_location() -> *mut i32;
}

const EINVAL: i32 = 22;
/// GRND_NONBLOCK | GRND_RANDOM | GRND_INSECURE: the flags the real call accepts.
const KNOWN_FLAGS: u32 = 0x1 | 0x2 | 0x4;

/// Seed + 1 once initialized (0 = not yet read from the environment).
static SEED_PLUS_ONE: AtomicU64 = AtomicU64::new(0);
/// Stream positions (8-byte words handed out): main thread, others.
static MAIN_COUNTER: AtomicU64 = AtomicU64::new(0);
static OTHER_COUNTER: AtomicU64 = AtomicU64::new(0);

fn seed() -> u64 {
    let cached = SEED_PLUS_ONE.load(Ordering::Relaxed);
    if cached != 0 {
        return cached - 1;
    }
    let mut value: u64 = 0;
    // SAFETY: getenv with a NUL-terminated name; the result is either
    // null or a NUL-terminated string owned by the environment.
    let ptr = unsafe { getenv(b"BEVY_PLAYTEST_DETRNG_SEED\0".as_ptr()) };
    if !ptr.is_null() {
        let mut i = 0;
        loop {
            // SAFETY: reading up to the terminating NUL.
            let c = unsafe { *ptr.add(i) };
            if !c.is_ascii_digit() {
                break;
            }
            value = value.wrapping_mul(10).wrapping_add(u64::from(c - b'0'));
            i += 1;
        }
    }
    SEED_PLUS_ONE.store(value.wrapping_add(1), Ordering::Relaxed);
    value
}

fn splitmix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// `ssize_t getrandom(void *buf, size_t buflen, unsigned int flags)`.
///
/// # Safety
/// `buf` must be valid for `len` bytes of writes (the C contract).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn getrandom(buf: *mut u8, len: usize, flags: u32) -> isize {
    if flags & !KNOWN_FLAGS != 0 {
        // SAFETY: errno is thread-local storage owned by libc.
        unsafe { *__errno_location() = EINVAL };
        return -1;
    }
    let len = len.min(isize::MAX as usize);
    // SAFETY: plain syscall wrappers without preconditions.
    let main = unsafe { getpid() == gettid() };
    let (counter, stream) = if main {
        (&MAIN_COUNTER, seed())
    } else {
        (&OTHER_COUNTER, seed() ^ 0xA5A5_A5A5_A5A5_A5A5)
    };
    let mut written = 0;
    while written < len {
        let n = counter.fetch_add(1, Ordering::Relaxed);
        let word = splitmix64(stream.wrapping_add(n.wrapping_mul(0x9E37_79B9_7F4A_7C15)));
        for byte in word.to_le_bytes() {
            if written == len {
                break;
            }
            // SAFETY: written < len, within the caller's buffer.
            unsafe { *buf.add(written) = byte };
            written += 1;
        }
    }
    len as isize
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
