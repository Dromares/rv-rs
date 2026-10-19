//! The MSB-first bit reader and the canonical Huffman decoder CHD uses for the v5 map and the
//! `huff` codec (`CHD/CHDSharpLib/Utils/{BitStream,HuffmanDecoder}.cs`, MAME `huffman.cpp`).

/// `BitStream` (`BitStream.cs`): reads past the end give zero bits and set the overflow flag.
pub(super) struct BitStream<'a> {
    data: &'a [u8],
    /// Next byte to load; may run past `data.len()`.
    offset: usize,
    buffer: u32,
    bits: i32,
}

impl<'a> BitStream<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitStream {
            data,
            offset: 0,
            buffer: 0,
            bits: 0,
        }
    }

    /// `peek` (`BitStream.cs:44-61`).
    pub fn peek(&mut self, numbits: u32) -> u32 {
        if numbits == 0 {
            return 0;
        }
        if numbits as i32 > self.bits {
            while self.bits <= 24 {
                if let Some(&b) = self.data.get(self.offset) {
                    self.buffer |= (b as u32) << (24 - self.bits);
                }
                self.offset += 1;
                self.bits += 8;
            }
        }
        self.buffer >> (32 - numbits)
    }

    /// `remove` (`BitStream.cs:67-71`).
    pub fn remove(&mut self, numbits: u32) {
        self.buffer = self.buffer.checked_shl(numbits).unwrap_or(0);
        self.bits -= numbits as i32;
    }

    /// `read` (`BitStream.cs:77-82`).
    pub fn read(&mut self, numbits: u32) -> u32 {
        let r = self.peek(numbits);
        self.remove(numbits);
        r
    }

    /// True when more bits were consumed than the data holds (`overflow`, `BitStream.cs:17-20`).
    pub fn overflow(&self) -> bool {
        self.offset as i64 - (self.bits / 8) as i64 > self.data.len() as i64
    }
}

#[derive(Clone, Copy, Default)]
struct Node {
    bits: u32,
    numbits: u8,
}

/// `HuffmanDecoder` (`HuffmanDecoder.cs:25-307`).
pub(super) struct HuffmanDecoder {
    numcodes: usize,
    maxbits: u32,
    lookup: Vec<u16>,
    nodes: Vec<Node>,
}

/// Why a tree could not be imported (`huffman_error`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum HuffError {
    InvalidData,
    InconsistentCodeLengths,
    InputTooSmall,
}

impl HuffmanDecoder {
    pub fn new(numcodes: usize, maxbits: u32) -> HuffmanDecoder {
        assert!(maxbits <= 24, "the lookup table would be too large");
        HuffmanDecoder {
            numcodes,
            maxbits,
            lookup: vec![0; 1 << maxbits],
            nodes: vec![Node::default(); numcodes],
        }
    }

    /// `DecodeOne` (`HuffmanDecoder.cs:78-89`).
    pub fn decode_one(&self, bs: &mut BitStream<'_>) -> u32 {
        let bits = bs.peek(self.maxbits);
        let entry = self.lookup[bits as usize] as u32;
        bs.remove(entry & 0x1f);
        entry >> 5
    }

    /// `ImportTreeRLE` (`HuffmanDecoder.cs:96-153`): the tree of the v5 map.
    pub fn import_tree_rle(&mut self, bs: &mut BitStream<'_>) -> Result<(), HuffError> {
        let numbits = if self.maxbits >= 16 {
            5
        } else if self.maxbits >= 8 {
            4
        } else {
            3
        };
        let mut cur = 0;
        while cur < self.numcodes {
            let mut nodebits = bs.read(numbits);
            if nodebits != 1 {
                self.nodes[cur].numbits = nodebits as u8;
                cur += 1;
            } else {
                // A one is an escape: a double one is a single one, anything else is a repeat.
                nodebits = bs.read(numbits);
                if nodebits == 1 {
                    self.nodes[cur].numbits = 1;
                    cur += 1;
                } else {
                    let mut rep = bs.read(numbits) as usize + 3;
                    if rep + cur > self.numcodes {
                        return Err(HuffError::InvalidData);
                    }
                    while rep > 0 {
                        self.nodes[cur].numbits = nodebits as u8;
                        cur += 1;
                        rep -= 1;
                    }
                }
            }
        }
        self.finish_import(bs)
    }

    /// `ImportTreeHuffman` (`HuffmanDecoder.cs:161-231`): the tree of the `huff` codec, itself
    /// Huffman coded with a 24-symbol, 6-bit tree.
    pub fn import_tree_huffman(&mut self, bs: &mut BitStream<'_>) -> Result<(), HuffError> {
        let mut small = HuffmanDecoder::new(24, 6);
        small.nodes[0].numbits = bs.read(3) as u8;
        let start = bs.read(3) as usize + 1;
        let mut count = 0;
        for index in 1..24 {
            if index < start || count == 7 {
                small.nodes[index].numbits = 0;
            } else {
                count = bs.read(3);
                small.nodes[index].numbits = if count == 7 { 0 } else { count as u8 };
            }
        }
        small.assign_canonical_codes()?;
        small.build_lookup_table();

        // The longest RLE count.
        let mut temp = self.numcodes.saturating_sub(9);
        let mut rlefullbits = 0;
        while temp != 0 {
            temp >>= 1;
            rlefullbits += 1;
        }

        let mut last = 0u8;
        let mut cur = 0;
        while cur < self.numcodes {
            let value = small.decode_one(bs);
            if value != 0 {
                last = (value - 1) as u8;
                self.nodes[cur].numbits = last;
                cur += 1;
            } else {
                let mut count = bs.read(3) as usize + 2;
                if count == 7 + 2 {
                    count += bs.read(rlefullbits) as usize;
                }
                while count != 0 && cur < self.numcodes {
                    self.nodes[cur].numbits = last;
                    cur += 1;
                    count -= 1;
                }
            }
        }
        self.finish_import(bs)
    }

    fn finish_import(&mut self, bs: &BitStream<'_>) -> Result<(), HuffError> {
        self.assign_canonical_codes()?;
        self.build_lookup_table();
        if bs.overflow() {
            Err(HuffError::InputTooSmall)
        } else {
            Ok(())
        }
    }

    /// `AssignCanonicalCodes` (`HuffmanDecoder.cs:240-276`).
    fn assign_canonical_codes(&mut self) -> Result<(), HuffError> {
        let mut histo = [0u32; 33];
        for n in &self.nodes {
            if n.numbits as u32 > self.maxbits {
                return Err(HuffError::InconsistentCodeLengths);
            }
            if n.numbits <= 32 {
                histo[n.numbits as usize] += 1;
            }
        }
        let mut curstart = 0u32;
        for codelen in (1..=32).rev() {
            let nextstart = (curstart + histo[codelen]) >> 1;
            if codelen != 1 && nextstart * 2 != curstart + histo[codelen] {
                return Err(HuffError::InconsistentCodeLengths);
            }
            histo[codelen] = curstart;
            curstart = nextstart;
        }
        for n in &mut self.nodes {
            if n.numbits > 0 {
                n.bits = histo[n.numbits as usize];
                histo[n.numbits as usize] += 1;
            }
        }
        Ok(())
    }

    /// `BuildLookupTable` (`HuffmanDecoder.cs:283-306`).
    fn build_lookup_table(&mut self) {
        for (code, n) in self.nodes.iter().enumerate() {
            if n.numbits == 0 {
                continue;
            }
            let value = ((code as u32) << 5 | n.numbits as u32 & 0x1f) as u16;
            let shift = self.maxbits - n.numbits as u32;
            let dest = (n.bits << shift) as usize;
            let end = (((n.bits + 1) << shift) - 1) as usize;
            let last = end.min(self.lookup.len() - 1);
            for slot in &mut self.lookup[dest..=last] {
                *slot = value;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit writer for building test streams.
    pub(crate) struct BitWriter {
        pub bytes: Vec<u8>,
        acc: u64,
        n: u32,
    }

    impl BitWriter {
        pub fn new() -> Self {
            BitWriter {
                bytes: Vec::new(),
                acc: 0,
                n: 0,
            }
        }
        pub fn put(&mut self, value: u32, bits: u32) {
            self.acc = (self.acc << bits) | (value as u64 & ((1u64 << bits) - 1));
            self.n += bits;
            while self.n >= 8 {
                self.bytes.push((self.acc >> (self.n - 8)) as u8);
                self.n -= 8;
            }
        }
        pub fn finish(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.bytes.push((self.acc << (8 - self.n)) as u8);
            }
            self.bytes
        }
    }

    #[test]
    fn bit_stream_reads_msb_first_and_flags_overflow() {
        let data = [0b1010_0000, 0xff];
        let mut b = BitStream::new(&data);
        assert_eq!(b.read(3), 0b101);
        assert_eq!(b.read(5), 0);
        assert_eq!(b.read(8), 0xff);
        assert!(!b.overflow());
        assert_eq!(b.read(8), 0);
        assert!(b.overflow());
    }

    #[test]
    fn rle_tree_decodes_a_flat_16_symbol_code() {
        // Sixteen codes of 4 bits each, written as sixteen 4-bit lengths (maxbits 8 -> 4-bit
        // fields), then symbols 0..16 as 4-bit codes. A complete tree: canonical code = symbol.
        let mut w = BitWriter::new();
        for _ in 0..16 {
            w.put(4, 4);
        }
        for s in [3u32, 0, 15, 7, 8] {
            w.put(s, 4);
        }
        let data = w.finish();
        let mut bs = BitStream::new(&data);
        let mut d = HuffmanDecoder::new(16, 8);
        d.import_tree_rle(&mut bs).unwrap();
        let got: Vec<u32> = (0..5).map(|_| d.decode_one(&mut bs)).collect();
        assert_eq!(got, [3, 0, 15, 7, 8]);
    }

    #[test]
    fn rle_tree_uses_repeat_escapes() {
        // Eight lengths of 3 (a complete tree) written as `1, 3, 5` = repeat three-plus-5 times.
        let mut w = BitWriter::new();
        w.put(1, 4);
        w.put(3, 4);
        w.put(5, 4);
        let data = w.finish();
        let mut bs = BitStream::new(&data);
        let mut d = HuffmanDecoder::new(8, 8);
        d.import_tree_rle(&mut bs).unwrap();
        assert!(d.nodes.iter().all(|n| n.numbits == 3));
    }

    #[test]
    fn an_over_subscribed_tree_is_rejected() {
        // Five codes of two bits cannot exist (a two-bit code has four values).
        let mut w = BitWriter::new();
        for _ in 0..5 {
            w.put(2, 4);
        }
        let data = w.finish();
        let mut bs = BitStream::new(&data);
        let mut d = HuffmanDecoder::new(5, 8);
        assert_eq!(
            d.import_tree_rle(&mut bs),
            Err(HuffError::InconsistentCodeLengths)
        );
    }

    #[test]
    fn a_tree_that_runs_past_the_data_is_an_error() {
        let mut bs = BitStream::new(&[0x44]);
        let mut d = HuffmanDecoder::new(16, 8);
        assert!(d.import_tree_rle(&mut bs).is_err());
    }
}
