use std::fmt;

const GF256_PRIMITIVE: u16 = 0x11d;

pub struct Gf256 {
    exp: [u8; 512],
    log: [u8; 256],
}

impl fmt::Debug for Gf256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Gf256(primitive=0x11d)")
    }
}

impl Default for Gf256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Gf256 {
    pub fn new() -> Self {
        let mut exp = [0u8; 512];
        let mut log = [0u8; 256];
        let mut val: u16 = 1;
        for i in 0..255 {
            exp[i] = val as u8;
            log[val as usize] = i as u8;
            val = Self::gf_mul_step(val);
        }
        for i in 255..511 {
            exp[i] = exp[i - 255];
        }
        Self { exp, log }
    }

    fn gf_mul_step(val: u16) -> u16 {
        let mut v = val << 1;
        if v & 0x100 != 0 {
            v ^= GF256_PRIMITIVE;
        }
        v
    }

    pub fn add(&self, a: u8, b: u8) -> u8 {
        a ^ b
    }

    pub fn sub(&self, a: u8, b: u8) -> u8 {
        a ^ b
    }

    pub fn mul(&self, a: u8, b: u8) -> u8 {
        if a == 0 || b == 0 {
            return 0;
        }
        let sum = self.log[a as usize] as u16 + self.log[b as usize] as u16;
        self.exp[sum as usize]
    }

    pub fn inv(&self, a: u8) -> u8 {
        if a == 0 {
            return 0;
        }
        let l = self.log[a as usize] as u16;
        self.exp[255 - l as usize]
    }

    pub fn pow(&self, a: u8, n: i32) -> u8 {
        if a == 0 {
            return 0;
        }
        if n == 0 {
            return 1;
        }
        let log_a = self.log[a as usize] as i32;
        let mut sum = log_a * n;
        sum %= 255;
        if sum < 0 {
            sum += 255;
        }
        self.exp[sum as usize]
    }

    pub fn poly_eval(&self, coeffs: &[u8], x: u8) -> u8 {
        let mut y = coeffs[0];
        for &c in &coeffs[1..] {
            y = self.add(self.mul(y, x), c);
        }
        y
    }
}
