//! The clock and the randomness the kernel reads, behind one trait so a test can replace both.
//!
//! [`SeededEnv`] is the Rust half of test/kernel-parity-hooks.ts: the same SHA-256 counter stream and the same 1 ms clock,
//! consumed in the same order, so a Rust run and a Node run of one sequence write the same rows.

use capstan_ledger::now_millis;
use sha2::{Digest, Sha256};
use std::cell::Cell;

pub trait Env {
    /// Milliseconds since the epoch, like `Date.now()`; every call is one reading of the clock.
    fn now(&self) -> i64;
    /// A version 4 UUID, like `crypto.randomUUID()`.
    fn uuid(&self) -> String;
    /// `n` random bytes, like `crypto.randomBytes(n)`.
    fn random_bytes(&self, n: usize) -> Vec<u8>;
}

/// The UUID text of 16 random bytes: version 4, variant 10.
pub fn uuid_from_bytes(mut b: [u8; 16]) -> String {
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// The wall clock and the operating system's random source.
pub struct SystemEnv;

impl Env for SystemEnv {
    fn now(&self) -> i64 {
        now_millis()
    }

    fn uuid(&self) -> String {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).expect("the operating system has a random source");
        uuid_from_bytes(bytes)
    }

    fn random_bytes(&self, n: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; n];
        getrandom::fill(&mut bytes).expect("the operating system has a random source");
        bytes
    }
}

/// 2026-01-01T00:00:00.000Z, where every seeded clock starts.
pub const SEEDED_EPOCH_MS: i64 = 1_767_225_600_000;

/// Deterministic randomness and time. Block `i` of the stream is `SHA-256(seed UTF-8 || i as 8 bytes, big endian)`;
/// `random_bytes(n)` takes whole blocks from the counter until it has `n` bytes and drops the rest of the last block.
/// The clock returns `SEEDED_EPOCH_MS` on the first reading and one millisecond more on each later one.
pub struct SeededEnv {
    seed: Vec<u8>,
    counter: Cell<u64>,
    clock: Cell<i64>,
}

impl SeededEnv {
    pub fn new(seed: &str) -> Self {
        Self {
            seed: seed.as_bytes().to_vec(),
            counter: Cell::new(0),
            clock: Cell::new(SEEDED_EPOCH_MS),
        }
    }
}

impl Env for SeededEnv {
    fn now(&self) -> i64 {
        let now = self.clock.get();
        self.clock.set(now + 1);
        now
    }

    fn uuid(&self) -> String {
        let bytes = self.random_bytes(16);
        let mut array = [0u8; 16];
        array.copy_from_slice(&bytes);
        uuid_from_bytes(array)
    }

    fn random_bytes(&self, n: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(n + 32);
        while out.len() < n {
            let counter = self.counter.get();
            self.counter.set(counter + 1);
            let mut hasher = Sha256::new();
            hasher.update(&self.seed);
            hasher.update(counter.to_be_bytes());
            out.extend_from_slice(&hasher.finalize());
        }
        out.truncate(n);
        out
    }
}
