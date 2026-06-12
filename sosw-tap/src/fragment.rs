use std::collections::HashMap;

pub const MAC_HEADER_LEN: usize = 4;
pub const MAX_FRAG_PAYLOAD: usize = 438; // 442 - 4 header
pub const MAX_NODE_ID: u8 = 255;

const FRAG_FLAG_FIRST: u8 = 0x80;
const FRAG_FLAG_LAST: u8 = 0x40;
const FRAG_INDEX_MASK: u8 = 0x3F;

pub struct FragmentHeader {
    pub dst_id: u8,
    pub src_id: u8,
    pub first_frag: bool,
    pub last_frag: bool,
    pub frag_index: u8,
    pub frame_id: u8,
}

impl FragmentHeader {
    pub fn encode(&self) -> [u8; MAC_HEADER_LEN] {
        let mut flags = self.frag_index & FRAG_INDEX_MASK;
        if self.first_frag {
            flags |= FRAG_FLAG_FIRST;
        }
        if self.last_frag {
            flags |= FRAG_FLAG_LAST;
        }
        [self.dst_id, self.src_id, flags, self.frame_id]
    }

    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < MAC_HEADER_LEN {
            return None;
        }
        let flags = data[2];
        Some(Self {
            dst_id: data[0],
            src_id: data[1],
            first_frag: (flags & FRAG_FLAG_FIRST) != 0,
            last_frag: (flags & FRAG_FLAG_LAST) != 0,
            frag_index: flags & FRAG_INDEX_MASK,
            frame_id: data[3],
        })
    }
}

pub fn fragment_eth_frame(
    eth_frame: &[u8],
    dst_id: u8,
    src_id: u8,
    frame_id: u8,
) -> Vec<Vec<u8>> {
    let n_frags = (eth_frame.len() + MAX_FRAG_PAYLOAD - 1) / MAX_FRAG_PAYLOAD;
    let mut frags = Vec::with_capacity(n_frags);

    for i in 0..n_frags {
        let start = i * MAX_FRAG_PAYLOAD;
        let end = (start + MAX_FRAG_PAYLOAD).min(eth_frame.len());
        let frag_data = &eth_frame[start..end];

        let header = FragmentHeader {
            dst_id,
            src_id,
            first_frag: i == 0,
            last_frag: i == n_frags - 1,
            frag_index: i as u8,
            frame_id,
        };

        let mut mac_frame = Vec::with_capacity(MAC_HEADER_LEN + frag_data.len());
        mac_frame.extend_from_slice(&header.encode());
        mac_frame.extend_from_slice(frag_data);
        frags.push(mac_frame);
    }

    frags
}

#[derive(Default)]
pub struct ReassemblyBuffer {
    groups: HashMap<(u8, u8), FragmentGroup>,
}

#[allow(dead_code)]
struct FragmentGroup {
    fragments: Vec<Option<Vec<u8>>>,
    first_index: Option<u8>,
    last_index: Option<u8>,
    count: usize,
}

impl FragmentGroup {
    fn new() -> Self {
        Self {
            fragments: Vec::new(),
            first_index: None,
            last_index: None,
            count: 0,
        }
    }

    fn ensure_capacity(&mut self, idx: u8) {
        let needed = (idx as usize) + 1;
        if self.fragments.len() < needed {
            self.fragments.resize(needed, None);
        }
    }

    fn insert(&mut self, header: &FragmentHeader, data: &[u8]) {
        let idx = header.frag_index as usize;
        self.ensure_capacity(header.frag_index);
        if self.fragments[idx].is_none() {
            self.fragments[idx] = Some(data.to_vec());
            self.count += 1;
        }
        if header.first_frag {
            self.first_index = Some(header.frag_index);
        }
        if header.last_frag {
            self.last_index = Some(header.frag_index);
        }
    }

    fn is_complete(&self) -> bool {
        match (self.first_index, self.last_index) {
            (Some(first), Some(last)) => {
                if first > last {
                    return false;
                }
                let expected = (last - first + 1) as usize;
                self.count >= expected
                    && self.fragments.len() >= (last as usize + 1)
                    && (first as usize..=last as usize)
                        .all(|i| self.fragments[i].is_some())
            }
            _ => false,
        }
    }

    fn reassemble(self) -> Option<Vec<u8>> {
        let first = self.first_index?;
        let last = self.last_index?;
        let mut out = Vec::new();
        for i in first..=last {
            out.extend_from_slice(self.fragments[i as usize].as_ref()?);
        }
        Some(out)
    }
}

impl ReassemblyBuffer {
    pub fn add_fragment(&mut self, mac_frame: &[u8]) -> Option<Vec<u8>> {
        let header = FragmentHeader::decode(mac_frame)?;
        let frag_data = &mac_frame[MAC_HEADER_LEN..];
        let key = (header.src_id, header.frame_id);

        let group = self
            .groups
            .entry(key)
            .or_insert_with(FragmentGroup::new);

        group.insert(&header, frag_data);

        if group.is_complete() {
            let group = self.groups.remove(&key)?;
            return group.reassemble();
        }

        None
    }

    pub fn cleanup(&mut self) {
        self.groups.retain(|_, g| {
            match (g.first_index, g.last_index) {
                (Some(_), Some(last)) => {
                    let max_age = (last as usize + 1) * 4;
                    g.count < max_age // keep recent-ish groups
                }
                _ => g.count < 256, // unbounded keep-all
            }
        });
    }

    pub fn pending_count(&self) -> usize {
        self.groups.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fragment_header_roundtrip() {
        let hdr = FragmentHeader {
            dst_id: 0xAA,
            src_id: 0xBB,
            first_frag: true,
            last_frag: false,
            frag_index: 3,
            frame_id: 0xCC,
        };
        let encoded = hdr.encode();
        let decoded = FragmentHeader::decode(&encoded).unwrap();
        assert_eq!(decoded.dst_id, 0xAA);
        assert_eq!(decoded.src_id, 0xBB);
        assert!(decoded.first_frag);
        assert!(!decoded.last_frag);
        assert_eq!(decoded.frag_index, 3);
        assert_eq!(decoded.frame_id, 0xCC);
    }

    #[test]
    fn test_decode_too_short() {
        assert!(FragmentHeader::decode(&[0; 3]).is_none());
    }

    #[test]
    fn test_fragment_and_reassemble_small() {
        let data = b"hello sonic wifi!";
        let frags = fragment_eth_frame(data, 1, 2, 42);
        assert_eq!(frags.len(), 1);

        let mut reassembly = ReassemblyBuffer::default();
        let result = reassembly.add_fragment(&frags[0]);
        assert_eq!(result, Some(data.to_vec()));
    }

    #[test]
    fn test_fragment_and_reassemble_large() {
        let data: Vec<u8> = (0..200u8).cycle().take(1500).collect();
        let frags = fragment_eth_frame(&data, 1, 2, 42);
        assert!(frags.len() > 1);

        let mut reassembly = ReassemblyBuffer::default();
        let mut result = None;
        for frag in &frags {
            result = reassembly.add_fragment(frag);
        }
        assert_eq!(result, Some(data));
    }

    #[test]
    fn test_reassembly_incomplete() {
        let data = vec![0xAB; 1000];
        let frags = fragment_eth_frame(&data, 1, 2, 42);
        assert!(frags.len() >= 2);

        let mut reassembly = ReassemblyBuffer::default();
        assert!(reassembly.add_fragment(&frags[0]).is_none());
        assert!(reassembly.pending_count() > 0);
    }

    #[test]
    fn test_out_of_order_fragments() {
        let data: Vec<u8> = (0..200u8).cycle().take(1500).collect();
        let mut frags = fragment_eth_frame(&data, 1, 2, 42);
        assert!(frags.len() >= 2);

        let last = frags.len() - 1;
        frags.swap(0, last);

        let mut reassembly = ReassemblyBuffer::default();
        let mut result = None;
        for frag in &frags {
            result = reassembly.add_fragment(frag);
        }
        assert_eq!(result, Some(data));
    }

    #[test]
    fn test_max_node_id_in_header() {
        let hdr = FragmentHeader {
            dst_id: 0xFF,
            src_id: 0x7F,
            first_frag: false,
            last_frag: true,
            frag_index: 0x3F,
            frame_id: 0xFF,
        };
        let encoded = hdr.encode();
        let decoded = FragmentHeader::decode(&encoded).unwrap();
        assert_eq!(decoded.frag_index, 0x3F);
        assert!(!decoded.first_frag);
        assert!(decoded.last_frag);
    }
}
