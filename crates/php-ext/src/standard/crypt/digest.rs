//! Incremental MD5/SHA-2 contexts for the crypt algorithms, on top of the
//! `php_hash` crate.

use php_hash::{HashAlgorithm, HashOptions, Hasher};

/// Looks up a built-in hash algorithm.
pub fn algorithm(name: &str) -> &'static HashAlgorithm {
    php_hash::find(name).expect("built-in hash algorithm")
}

/// One incremental digest computation.
pub struct Digest(Box<dyn Hasher>);

impl Digest {
    pub fn new(algo: &'static HashAlgorithm) -> Digest {
        Digest(
            algo.new_hasher(&HashOptions::none())
                .expect("built-in hash algorithm"),
        )
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finish(self) -> Vec<u8> {
        self.0.finalize()
    }
}
