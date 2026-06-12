use reed_solomon::{Encoder, Decoder, Buffer};

pub struct ReedSolomonFec {
    nsym: usize,
    encoder: Encoder,
    decoder: Decoder,
}

impl ReedSolomonFec {
    pub fn new(nsym: usize) -> Self {
        Self {
            nsym,
            encoder: Encoder::new(nsym),
            decoder: Decoder::new(nsym),
        }
    }

    pub fn nsym(&self) -> usize {
        self.nsym
    }

    pub fn max_data_bytes(&self) -> usize {
        255 - self.nsym
    }

    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let msg_len = self.max_data_bytes();
        let n_blocks = (data.len() + msg_len - 1) / msg_len;
        let mut result = Vec::with_capacity(n_blocks * 255);

        for block_idx in 0..n_blocks {
            let start = block_idx * msg_len;
            let end = std::cmp::min(start + msg_len, data.len());
            let block = &data[start..end];

            let mut padded = vec![0u8; msg_len];
            for (j, &b) in block.iter().enumerate() {
                padded[j] = b;
            }

            let encoded: Buffer = self.encoder.encode(&padded);
            result.extend_from_slice(encoded.data());
            result.extend_from_slice(encoded.ecc());
        }

        result
    }

    pub fn decode(&self, encoded: &[u8]) -> (Vec<u8>, bool) {
        let block_len = 255usize;
        let n_blocks = (encoded.len() + block_len - 1) / block_len;
        let mut decoded = Vec::with_capacity(n_blocks * block_len);
        let mut overall_success = true;

        for block_idx in 0..n_blocks {
            let start = block_idx * block_len;
            let end = std::cmp::min(start + block_len, encoded.len());
            let mut block_data = Vec::from(&encoded[start..end]);
            if block_data.len() < block_len {
                block_data.resize(block_len, 0);
            }

            let msg_len = self.max_data_bytes();
            match self.decoder.correct(&mut block_data, None) {
                Ok(corrected) => {
                    decoded.extend_from_slice(corrected.data());
                }
                Err(_) => {
                    overall_success = false;
                    decoded.extend_from_slice(&block_data[..msg_len]);
                }
            }
        }

        (decoded, overall_success)
    }
}
