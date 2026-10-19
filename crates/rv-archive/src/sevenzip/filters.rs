//! The 7z branch-conversion filters and Delta, decode direction only
//! (`Compress/Support/Filters/{BCJFilter,BCJFilterARM,BCJFilterARMT,BCJFilterPPC,BCJFilterSPARC,
//! BCJFilterIA64,DeltaFilter}.cs`, which follow xz-java).
//!
//! They are done here instead of through liblzma because liblzma holds back the last bytes of a
//! BCJ stream until the LZMA1 decoder reports the end of the stream, and 7z's LZMA1 data has no
//! end marker (the size is known instead), so `BCJ + LZMA` folders would lose their tail.
//! BCJ2 (four input streams) and ARM64 (ROMVault leaves it out too) are not ported.

use std::io::{self, Read};

/// A filter that rewrites a prefix of the buffer in place. Bytes it does not reach (it needs a
/// few bytes of look-ahead) are offered again with more data after them.
pub(super) trait Transform {
    /// Convert `buf` from the front; return how many bytes are final.
    fn transform(&mut self, buf: &mut [u8]) -> usize;
}

/// Wraps a reader in a [`Transform`]. At the end of the data the unreached tail passes through
/// unchanged, as it does in the encoder.
pub(super) struct FilterReader<R, T> {
    inner: R,
    filter: T,
    buf: Vec<u8>,
    /// Bytes `buf[out..done]` are final and not yet returned.
    out: usize,
    done: usize,
    filled: usize,
    eof: bool,
}

impl<R: Read, T: Transform> FilterReader<R, T> {
    pub fn new(inner: R, filter: T) -> Self {
        FilterReader {
            inner,
            filter,
            buf: vec![0; 1 << 16],
            out: 0,
            done: 0,
            filled: 0,
            eof: false,
        }
    }
}

impl<R: Read, T: Transform> Read for FilterReader<R, T> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        while self.out == self.done {
            if self.eof {
                return Ok(0);
            }
            // Keep the unconverted tail at the front, then top the buffer up.
            self.buf.copy_within(self.done..self.filled, 0);
            self.filled -= self.done;
            self.out = 0;
            self.done = 0;
            while self.filled < self.buf.len() && !self.eof {
                let n = self.inner.read(&mut self.buf[self.filled..])?;
                if n == 0 {
                    self.eof = true;
                }
                self.filled += n;
            }
            self.done = self.filter.transform(&mut self.buf[..self.filled]);
            if self.eof {
                self.done = self.filled;
            }
        }
        let n = out.len().min(self.done - self.out);
        out[..n].copy_from_slice(&self.buf[self.out..self.out + n]);
        self.out += n;
        Ok(n)
    }
}

/// x86 `BCJ` (`BCJFilter.cs`).
pub(super) struct X86 {
    pos: i32,
    prev_mask: u32,
}

impl X86 {
    /// `start` is the 4-byte property some encoders write; plain BCJ starts at 0 (+5).
    pub fn new(start: u32) -> Self {
        X86 {
            pos: start as i32 + 5,
            prev_mask: 0,
        }
    }
}

const MASK_TO_ALLOWED_STATUS: [bool; 8] = [true, true, true, false, true, false, false, false];
const MASK_TO_BIT_NUMBER: [u32; 8] = [0, 1, 2, 2, 3, 3, 3, 3];

fn test86(b: u8) -> bool {
    b == 0x00 || b == 0xff
}

impl Transform for X86 {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 5 {
            return 0;
        }
        let end = buf.len() - 5;
        let mut prev_pos: i32 = -1;
        let mut i = 0usize;
        while i <= end {
            if buf[i] & 0xfe != 0xe8 {
                i += 1;
                continue;
            }
            let mut pp = i as i32 - prev_pos;
            if pp & !3 != 0 {
                self.prev_mask = 0;
            } else {
                self.prev_mask = (self.prev_mask << (pp - 1)) & 7;
                if self.prev_mask != 0
                    && (!MASK_TO_ALLOWED_STATUS[self.prev_mask as usize]
                        || test86(
                            buf[i + 4 - MASK_TO_BIT_NUMBER[self.prev_mask as usize] as usize],
                        ))
                {
                    prev_pos = i as i32;
                    self.prev_mask = (self.prev_mask << 1) | 1;
                    i += 1;
                    continue;
                }
            }
            prev_pos = i as i32;
            pp = 0;
            let _ = pp;
            if test86(buf[i + 4]) {
                let mut src = i32::from_le_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]);
                let mut dest;
                loop {
                    dest = src.wrapping_sub(self.pos.wrapping_add(i as i32));
                    if self.prev_mask == 0 {
                        break;
                    }
                    let index = MASK_TO_BIT_NUMBER[self.prev_mask as usize] * 8;
                    if !test86((dest >> (24 - index)) as u8) {
                        break;
                    }
                    src = dest ^ ((1i32 << (32 - index)) - 1);
                }
                buf[i + 1] = dest as u8;
                buf[i + 2] = (dest >> 8) as u8;
                buf[i + 3] = (dest >> 16) as u8;
                buf[i + 4] = !((((dest >> 24) & 1) - 1) as u8);
                i += 4;
            } else {
                self.prev_mask = (self.prev_mask << 1) | 1;
            }
            i += 1;
        }
        let pp = i as i32 - prev_pos;
        self.prev_mask = if pp & !3 != 0 {
            0
        } else {
            self.prev_mask << (pp - 1)
        };
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// ARM (`BCJFilterARM.cs`).
pub(super) struct Arm {
    pos: i32,
}

impl Arm {
    pub fn new() -> Self {
        Arm { pos: 8 }
    }
}

impl Transform for Arm {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 4 {
            return 0;
        }
        let end = buf.len() - 4;
        let mut i = 0;
        while i <= end {
            if buf[i + 3] == 0xeb {
                let src = ((buf[i + 2] as u32) << 16) | ((buf[i + 1] as u32) << 8) | buf[i] as u32;
                let src = src << 2;
                let dest = src.wrapping_sub((self.pos as u32).wrapping_add(i as u32)) >> 2;
                buf[i + 2] = (dest >> 16) as u8;
                buf[i + 1] = (dest >> 8) as u8;
                buf[i] = dest as u8;
            }
            i += 4;
        }
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// ARM Thumb (`BCJFilterARMT.cs`).
pub(super) struct ArmThumb {
    pos: i32,
}

impl ArmThumb {
    pub fn new() -> Self {
        ArmThumb { pos: 4 }
    }
}

impl Transform for ArmThumb {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 4 {
            return 0;
        }
        let end = buf.len() - 4;
        let mut i = 0;
        while i <= end {
            if buf[i + 1] & 0xf8 == 0xf0 && buf[i + 3] & 0xf8 == 0xf8 {
                let src = (((buf[i + 1] & 7) as u32) << 19)
                    | ((buf[i] as u32) << 11)
                    | (((buf[i + 3] & 7) as u32) << 8)
                    | buf[i + 2] as u32;
                let src = src << 1;
                let dest = src.wrapping_sub((self.pos as u32).wrapping_add(i as u32)) >> 1;
                buf[i + 1] = 0xf0 | ((dest >> 19) & 7) as u8;
                buf[i] = (dest >> 11) as u8;
                buf[i + 3] = 0xf8 | ((dest >> 8) & 7) as u8;
                buf[i + 2] = dest as u8;
                i += 2;
            }
            i += 2;
        }
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// PowerPC (`BCJFilterPPC.cs`).
pub(super) struct Ppc {
    pos: i32,
}

impl Ppc {
    pub fn new() -> Self {
        Ppc { pos: 0 }
    }
}

impl Transform for Ppc {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 4 {
            return 0;
        }
        let end = buf.len() - 4;
        let mut i = 0;
        while i <= end {
            if buf[i] & 0xfc == 0x48 && buf[i + 3] & 3 == 1 {
                let src = (((buf[i] & 3) as u32) << 24)
                    | ((buf[i + 1] as u32) << 16)
                    | ((buf[i + 2] as u32) << 8)
                    | (buf[i + 3] & 0xfc) as u32;
                let dest = src.wrapping_sub((self.pos as u32).wrapping_add(i as u32));
                buf[i] = 0x48 | ((dest >> 24) & 3) as u8;
                buf[i + 1] = (dest >> 16) as u8;
                buf[i + 2] = (dest >> 8) as u8;
                buf[i + 3] = (buf[i + 3] & 3) | dest as u8;
            }
            i += 4;
        }
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// SPARC (`BCJFilterSPARC.cs`).
pub(super) struct Sparc {
    pos: i32,
}

impl Sparc {
    pub fn new() -> Self {
        Sparc { pos: 0 }
    }
}

impl Transform for Sparc {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 4 {
            return 0;
        }
        let end = buf.len() - 4;
        let mut i = 0;
        while i <= end {
            if (buf[i] == 0x40 && buf[i + 1] & 0xc0 == 0x00)
                || (buf[i] == 0x7f && buf[i + 1] & 0xc0 == 0xc0)
            {
                let src = u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]) << 2;
                let dest = src.wrapping_sub((self.pos as u32).wrapping_add(i as u32)) >> 2;
                let dest = ((0u32.wrapping_sub((dest >> 22) & 1) << 22) & 0x3fff_ffff)
                    | (dest & 0x3f_ffff)
                    | 0x4000_0000;
                buf[i..i + 4].copy_from_slice(&dest.to_be_bytes());
            }
            i += 4;
        }
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// IA-64 (`BCJFilterIA64.cs`).
pub(super) struct Ia64 {
    pos: i32,
}

impl Ia64 {
    pub fn new() -> Self {
        Ia64 { pos: 0 }
    }
}

const IA64_BRANCH_TABLE: [u8; 32] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 6, 6, 0, 0, 7, 7, 4, 4, 0, 0, 4, 4, 0, 0,
];

impl Transform for Ia64 {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        if buf.len() < 16 {
            return 0;
        }
        let end = buf.len() - 16;
        let mut i = 0;
        while i <= end {
            let mask = IA64_BRANCH_TABLE[(buf[i] & 0x1f) as usize];
            let mut bit_pos = 5usize;
            for slot in 0..3 {
                if (mask >> slot) & 1 != 0 {
                    let byte_pos = bit_pos >> 3;
                    let bit_res = (bit_pos & 7) as u32;
                    let mut instr = 0u64;
                    for j in 0..6 {
                        instr |= (buf[i + byte_pos + j] as u64) << (8 * j);
                    }
                    let mut norm = instr >> bit_res;
                    if (norm >> 37) & 0x0f == 0x05 && (norm >> 9) & 0x07 == 0x00 {
                        let mut src = ((norm >> 13) & 0x0f_ffff) as u32;
                        src |= (((norm >> 36) & 1) as u32) << 20;
                        src <<= 4;
                        let dest = src.wrapping_sub((self.pos as u32).wrapping_add(i as u32)) >> 4;
                        norm &= !(0x8f_ffffu64 << 13);
                        norm |= ((dest & 0x0f_ffff) as u64) << 13;
                        norm |= ((dest & 0x10_0000) as u64) << (36 - 20);
                        instr &= (1u64 << bit_res) - 1;
                        instr |= norm << bit_res;
                        for j in 0..6 {
                            buf[i + byte_pos + j] = (instr >> (8 * j)) as u8;
                        }
                    }
                }
                bit_pos += 41;
            }
            i += 16;
        }
        self.pos = self.pos.wrapping_add(i as i32);
        i
    }
}

/// Delta (`DeltaFilter.cs`): every byte is added to the byte `distance` places back.
pub(super) struct Delta {
    history: Vec<u8>,
    index: usize,
}

impl Delta {
    /// The 7z property byte is the distance minus one.
    pub fn new(property: u8) -> Self {
        Delta {
            history: vec![0; property as usize + 1],
            index: 0,
        }
    }
}

impl Transform for Delta {
    fn transform(&mut self, buf: &mut [u8]) -> usize {
        for b in buf.iter_mut() {
            *b = b.wrapping_add(self.history[self.index]);
            self.history[self.index] = *b;
            self.index = (self.index + 1) % self.history.len();
        }
        buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T: Transform>(t: T, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        FilterReader::new(data, t).read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn delta_adds_the_byte_one_distance_back() {
        // Distance 2: out[i] = in[i] + out[i - 2].
        let out = run(Delta::new(1), &[1, 2, 1, 1, 1, 1]);
        assert_eq!(out, [1, 2, 2, 3, 3, 4]);
    }

    #[test]
    fn bytes_without_branches_pass_through_whatever_the_chunking() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 7) as u8).collect();
        assert_eq!(run(X86::new(0), &data), data);
        assert_eq!(run(Arm::new(), &data), data);
        assert_eq!(run(Sparc::new(), &data), data);
    }

    #[test]
    fn an_x86_call_gets_its_absolute_target_turned_back_into_a_relative_one() {
        // `call rel32` at offset 0 whose encoded (absolute) operand is 0x1005: relative = 0x1000.
        let mut data = vec![0xe8, 0x05, 0x10, 0x00, 0x00];
        data.extend([0x90; 8]);
        let out = run(X86::new(0), &data);
        assert_eq!(&out[..5], &[0xe8, 0x00, 0x10, 0x00, 0x00]);
        assert_eq!(&out[5..], &[0x90; 8]);
    }

    #[test]
    fn the_unreached_tail_is_passed_on_unchanged() {
        // A branch opcode too close to the end to be converted stays as it is.
        let data = [0x90, 0x90, 0xe8, 0x05, 0x10];
        assert_eq!(run(X86::new(0), &data), data);
    }

    #[test]
    fn arm_branch_link() {
        // `bl` at offset 8 (pos 8 + i 0): absolute word target 6 -> bytes 0x06 00 00 EB;
        // decoding subtracts the position (8) shifted: (6 << 2) - 8 = 16 -> 4.
        let out = run(Arm::new(), &[0x06, 0x00, 0x00, 0xeb]);
        assert_eq!(out, [0x04, 0x00, 0x00, 0xeb]);
    }
}
