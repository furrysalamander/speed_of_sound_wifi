use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

pub struct Scrambler {
    seed: u64,
}

impl Scrambler {
    pub fn new(seed: u64) -> Self {
        Self { seed }
    }

    pub fn mask(&self, len: usize) -> Vec<u8> {
        let mut rng = ChaCha12Rng::seed_from_u64(self.seed);
        (0..len).map(|_| rng.random::<u8>()).collect()
    }

    pub fn scramble(&self, data: &[u8]) -> Vec<u8> {
        let mask = self.mask(data.len());
        data.iter().zip(mask.iter()).map(|(d, m)| d ^ m).collect()
    }

    pub fn descramble(&self, data: &[u8]) -> Vec<u8> {
        self.scramble(data)
    }
}
