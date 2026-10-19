//! A decoder for the headerless FLAC frames CHD stores (`flac` and `cdfl` hunks): 16-bit stereo,
//! frame after frame with no `fLaC` marker or STREAMINFO. It reports how many bytes each frame
//! used, because in `cdfl` the subcode data starts right after the last frame.
//!
//! `CHD/CHDSharpLib/Flac/AudioDecoder.cs` (a trimmed CUETools Flake decoder) is the reference:
//! `DecodeFrame` `:623-639`, `decode_frame_header` `:299-356`, `decode_subframes` `:441-`,
//! `decode_residual` `:370-`, `restore_samples` `:562-621`. The format itself is RFC 9639.

use crate::error::ChdError;

fn bad(detail: &str) -> ChdError {
    ChdError::Decompression(format!("FLAC: {detail}"))
}

/// CRC-8 of the frame header (polynomial 0x07).
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16 of the whole frame (polynomial 0x8005, not reflected).
fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

struct Bits<'a> {
    data: &'a [u8],
    /// Position in bits.
    pos: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> Result<u32, ChdError> {
        debug_assert!(n <= 32);
        let end = self.pos + n as usize;
        if end > self.data.len() * 8 {
            return Err(bad("the frame is cut short"));
        }
        if n == 0 {
            return Ok(0);
        }
        // Up to 32 bits starting at bit offset 0..7 of a byte fit in five bytes.
        let byte = self.pos >> 3;
        let off = (self.pos & 7) as u32;
        let mut v = 0u64;
        for i in 0..5 {
            v = (v << 8) | self.data.get(byte + i).copied().unwrap_or(0) as u64;
        }
        self.pos = end;
        Ok(((v >> (40 - off - n)) & ((1u64 << n) - 1)) as u32)
    }

    fn read_signed(&mut self, n: u32) -> Result<i32, ChdError> {
        if n == 0 {
            return Ok(0);
        }
        let v = self.read(n)? as i64;
        Ok(((v << (64 - n)) >> (64 - n)) as i32)
    }

    /// Count zero bits up to the next one bit and consume it.
    fn unary(&mut self) -> Result<u32, ChdError> {
        let mut n = 0u32;
        loop {
            let byte = *self
                .data
                .get(self.pos >> 3)
                .ok_or_else(|| bad("the frame is cut short"))?;
            let shifted = byte << (self.pos & 7);
            if shifted != 0 {
                let z = shifted.leading_zeros();
                self.pos += z as usize + 1;
                return Ok(n + z);
            }
            let rest = 8 - (self.pos & 7);
            n += rest as u32;
            self.pos += rest;
            if n > 1 << 24 {
                return Err(bad("an unreasonably long unary code"));
            }
        }
    }

    fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }
}

/// One decoded frame: interleaved 16-bit samples and the bytes the frame used.
pub(super) struct Frame {
    pub samples: Vec<i16>,
    pub bytes_used: usize,
}

const BLOCK_SIZES_SMALL: [u32; 16] = [
    0, 192, 576, 1152, 2304, 4608, 0, 0, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768,
];

/// Decode one 16-bit stereo frame from the start of `buf` (`AudioDecoder.DecodeFrame`).
pub(super) fn decode_frame(buf: &[u8]) -> Result<Frame, ChdError> {
    let mut b = Bits { data: buf, pos: 0 };
    // Sync: 14 bits 0b11111111111110, a reserved zero bit, then the blocking strategy bit.
    if b.read(14)? != 0x3ffe || b.read(1)? != 0 {
        return Err(bad("no frame sync"));
    }
    b.read(1)?; // fixed or variable block size: not needed
    let bs_code = b.read(4)? as usize;
    let sr_code = b.read(4)?;
    let ch_code = b.read(4)?;
    let size_code = b.read(3)?;
    if b.read(1)? != 0 {
        return Err(bad("reserved bit set in the frame header"));
    }
    // The coded frame or sample number: a UTF-8 style integer, 1 to 7 bytes.
    let first = b.read(8)?;
    let extra = match first {
        0x00..=0x7f => 0,
        0xc0..=0xdf => 1,
        0xe0..=0xef => 2,
        0xf0..=0xf7 => 3,
        0xf8..=0xfb => 4,
        0xfc..=0xfd => 5,
        0xfe => 6,
        _ => return Err(bad("bad frame number")),
    };
    for _ in 0..extra {
        b.read(8)?;
    }
    let blocksize = match bs_code {
        0 => return Err(bad("reserved block size")),
        6 => b.read(8)? + 1,
        7 => b.read(16)? + 1,
        c => BLOCK_SIZES_SMALL[c],
    };
    // Only coded sample rates are accepted: there is no STREAMINFO to say what code 0 means.
    if !(1..=11).contains(&sr_code) {
        return Err(bad("unsupported sample rate code"));
    }
    if size_code != 4 {
        return Err(bad("not 16 bits per sample"));
    }
    let header_end = b.pos / 8;
    let want_crc = b.read(8)? as u8;
    if crc8(&buf[..header_end]) != want_crc {
        return Err(bad("frame header CRC mismatch"));
    }
    let blocksize = blocksize as usize;
    if blocksize == 0 || blocksize > 32768 {
        return Err(bad("bad block size"));
    }
    if !matches!(ch_code, 1 | 8 | 9 | 10) {
        return Err(bad("not two channels"));
    }

    // Subframes. Channel code 1 is two independent channels; 8-10 add one bit to the side channel.
    let extra_bits: [u32; 2] = match ch_code {
        8 => [0, 1],
        9 => [1, 0],
        10 => [0, 1],
        _ => [0, 0],
    };
    let mut ch: [Vec<i32>; 2] = [Vec::new(), Vec::new()];
    for (c, out) in ch.iter_mut().enumerate() {
        *out = decode_subframe(&mut b, blocksize, 16 + extra_bits[c])?;
    }
    b.align();
    let body_end = b.pos / 8;
    let want = b.read(16)? as u16;
    if crc16(&buf[..body_end]) != want {
        return Err(bad("frame CRC mismatch"));
    }

    let mut samples = Vec::with_capacity(blocksize * 2);
    for (&a, &b) in ch[0].iter().zip(&ch[1]) {
        let (l, r) = match ch_code {
            1 => (a, b),
            8 => (a, a.wrapping_sub(b)),
            9 => (a.wrapping_add(b), b),
            _ => {
                let mid = a.wrapping_shl(1) | (b & 1);
                (mid.wrapping_add(b) >> 1, mid.wrapping_sub(b) >> 1)
            }
        };
        samples.push(l as i16);
        samples.push(r as i16);
    }
    Ok(Frame {
        samples,
        bytes_used: b.pos / 8,
    })
}

fn decode_subframe(b: &mut Bits<'_>, blocksize: usize, bps: u32) -> Result<Vec<i32>, ChdError> {
    if b.read(1)? != 0 {
        return Err(bad("subframe padding bit set"));
    }
    let ty = b.read(6)?;
    let mut wasted = 0;
    if b.read(1)? == 1 {
        wasted = b.unary()? + 1;
        if wasted >= bps {
            return Err(bad("all bits wasted"));
        }
    }
    let bps = bps - wasted;
    let mut out = vec![0i32; blocksize];
    match ty {
        0 => {
            let v = b.read_signed(bps)?;
            out.iter_mut().for_each(|s| *s = v);
        }
        1 => {
            for s in out.iter_mut() {
                *s = b.read_signed(bps)?;
            }
        }
        8..=15 if ty - 8 <= 4 => {
            let order = (ty - 8) as usize;
            if order > blocksize {
                return Err(bad("predictor order above the block size"));
            }
            for s in out.iter_mut().take(order) {
                *s = b.read_signed(bps)?;
            }
            residual(b, &mut out, order)?;
            for i in order..blocksize {
                let p = |k: usize| out[i - k] as i64;
                let pred = match order {
                    0 => 0,
                    1 => p(1),
                    2 => 2 * p(1) - p(2),
                    3 => 3 * p(1) - 3 * p(2) + p(3),
                    _ => 4 * p(1) - 6 * p(2) + 4 * p(3) - p(4),
                };
                out[i] = (out[i] as i64 + pred) as i32;
            }
        }
        32..=63 => {
            let order = (ty - 31) as usize;
            if order > blocksize {
                return Err(bad("predictor order above the block size"));
            }
            for s in out.iter_mut().take(order) {
                *s = b.read_signed(bps)?;
            }
            let precision = b.read(4)? + 1;
            if precision == 16 {
                return Err(bad("reserved coefficient precision"));
            }
            let shift = b.read_signed(5)?;
            if shift < 0 {
                return Err(bad("negative LPC shift"));
            }
            let mut coefs = Vec::with_capacity(order);
            for _ in 0..order {
                coefs.push(b.read_signed(precision)? as i64);
            }
            residual(b, &mut out, order)?;
            for i in order..blocksize {
                let mut sum = 0i64;
                for (j, &c) in coefs.iter().enumerate() {
                    sum += c * out[i - 1 - j] as i64;
                }
                out[i] = (out[i] as i64 + (sum >> shift)) as i32;
            }
        }
        _ => return Err(bad("reserved subframe type")),
    }
    if wasted > 0 {
        out.iter_mut().for_each(|s| *s = s.wrapping_shl(wasted));
    }
    Ok(out)
}

/// `decode_residual`: Rice-coded partitions, written into `out[order..]`.
fn residual(b: &mut Bits<'_>, out: &mut [i32], order: usize) -> Result<(), ChdError> {
    let method = b.read(2)?;
    if method > 1 {
        return Err(bad("reserved residual coding method"));
    }
    let (param_bits, escape) = if method == 0 { (4, 15) } else { (5, 31) };
    let part_order = b.read(4)?;
    let parts = 1usize << part_order;
    let blocksize = out.len();
    if !blocksize.is_multiple_of(parts) || blocksize / parts < order {
        return Err(bad("bad residual partition order"));
    }
    let per = blocksize / parts;
    let mut i = order;
    for p in 0..parts {
        let n = if p == 0 { per - order } else { per };
        let param = b.read(param_bits)?;
        if param == escape {
            let bits = b.read(5)?;
            for _ in 0..n {
                out[i] = b.read_signed(bits)?;
                i += 1;
            }
        } else {
            for _ in 0..n {
                let q = b.unary()?;
                let r = if param == 0 { 0 } else { b.read(param)? };
                let v = ((q as u64) << param) | r as u64;
                if v > u32::MAX as u64 {
                    return Err(bad("a residual does not fit 32 bits"));
                }
                let v = v as u32;
                out[i] = ((v >> 1) as i32) ^ -((v & 1) as i32);
                i += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MSB-first bit writer.
    struct W {
        bytes: Vec<u8>,
        cur: u64,
        n: u32,
    }
    impl W {
        fn new() -> Self {
            W {
                bytes: Vec::new(),
                cur: 0,
                n: 0,
            }
        }
        fn put(&mut self, v: u64, bits: u32) {
            for i in (0..bits).rev() {
                self.cur = (self.cur << 1) | ((v >> i) & 1);
                self.n += 1;
                if self.n == 8 {
                    self.bytes.push(self.cur as u8);
                    self.cur = 0;
                    self.n = 0;
                }
            }
        }
        fn align(&mut self) {
            while self.n != 0 {
                self.put(0, 1);
            }
        }
    }

    /// A stereo frame of `n` samples (block size code 6, 8-bit size field), channel code `ch`,
    /// each channel a verbatim subframe of the given values.
    fn verbatim_frame(ch_code: u32, left: &[i32], right: &[i32], bits: [u32; 2]) -> Vec<u8> {
        let n = left.len();
        let mut w = W::new();
        w.put(0x3ffe, 14);
        w.put(0, 1);
        w.put(0, 1);
        w.put(6, 4); // 8-bit block size follows
        w.put(9, 4); // 44.1 kHz
        w.put(ch_code as u64, 4);
        w.put(4, 3); // 16 bit
        w.put(0, 1);
        w.put(0, 8); // frame number 0
        w.put((n - 1) as u64, 8);
        let crc = crc8(&w.bytes);
        w.put(crc as u64, 8);
        for (vals, b) in [left, right].iter().zip(bits) {
            w.put(0, 1);
            w.put(1, 6); // verbatim
            w.put(0, 1);
            for &v in vals.iter() {
                w.put(v as u32 as u64 & ((1u64 << b) - 1), b);
            }
        }
        w.align();
        let crc = crc16(&w.bytes);
        w.put(crc as u64, 16);
        w.bytes
    }

    /// Frames written by libFLAC (`tests/data/gen_flac.py`) for 8192 stereo samples: fixed and
    /// LPC predictors, several block sizes, mid/side and independent channels, wasted bits.
    #[test]
    fn frames_written_by_libflac_decode_to_the_pcm() {
        let pcm: &[u8] = include_bytes!("../../tests/data/flac_pcm.raw");
        let want: Vec<i16> = pcm
            .chunks(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let variants: [(&str, &[u8]); 7] = [
            ("m0", include_bytes!("../../tests/data/flac_m0.frames")),
            ("m3", include_bytes!("../../tests/data/flac_m3.frames")),
            ("m5", include_bytes!("../../tests/data/flac_m5.frames")),
            ("m8", include_bytes!("../../tests/data/flac_m8.frames")),
            (
                "m8_nomid",
                include_bytes!("../../tests/data/flac_m8_nomid.frames"),
            ),
            (
                "m8_loose",
                include_bytes!("../../tests/data/flac_m8_loose.frames"),
            ),
            (
                "m6_exh",
                include_bytes!("../../tests/data/flac_m6_exh.frames"),
            ),
        ];
        for (name, frames) in variants {
            let (mut pos, mut got) = (0, Vec::new());
            while got.len() < want.len() {
                let f =
                    decode_frame(&frames[pos..]).unwrap_or_else(|e| panic!("{name} at {pos}: {e}"));
                got.extend(f.samples);
                pos += f.bytes_used;
            }
            assert_eq!(pos, frames.len(), "{name}: every byte belongs to a frame");
            assert_eq!(got, want, "{name}");
        }
    }

    #[test]
    fn independent_channels() {
        let l = [1, -2, 300, -32768];
        let r = [5, 6, -7, 32767];
        let f = verbatim_frame(1, &l, &r, [16, 16]);
        let d = decode_frame(&f).unwrap();
        assert_eq!(d.bytes_used, f.len());
        assert_eq!(d.samples, [1, 5, -2, 6, 300, -7, -32768, 32767]);
    }

    #[test]
    fn stereo_decorrelation_modes() {
        let (left, right) = ([100i32, -50, 7], [90i32, -60, 9]);
        let side: Vec<i32> = left.iter().zip(&right).map(|(l, r)| l - r).collect();
        let mid: Vec<i32> = left.iter().zip(&right).map(|(l, r)| (l + r) >> 1).collect();
        // left/side
        let f = verbatim_frame(8, &left, &side, [16, 17]);
        let d = decode_frame(&f).unwrap();
        assert_eq!(d.samples, [100, 90, -50, -60, 7, 9]);
        // side/right
        let f = verbatim_frame(9, &side, &right, [17, 16]);
        assert_eq!(decode_frame(&f).unwrap().samples, [100, 90, -50, -60, 7, 9]);
        // mid/side
        let f = verbatim_frame(10, &mid, &side, [16, 17]);
        assert_eq!(decode_frame(&f).unwrap().samples, [100, 90, -50, -60, 7, 9]);
    }

    #[test]
    fn a_damaged_frame_is_an_error() {
        let f = verbatim_frame(1, &[1, 2, 3], &[4, 5, 6], [16, 16]);
        for i in 0..f.len() {
            let mut g = f.clone();
            g[i] ^= 0x10;
            assert!(decode_frame(&g).is_err(), "byte {i}");
        }
        for len in 0..f.len() {
            assert!(decode_frame(&f[..len]).is_err(), "cut {len}");
        }
    }

    #[test]
    fn fixed_predictor_with_rice_residual() {
        // Order-2 fixed predictor over a ramp: residual zero everywhere, one partition.
        let n = 8usize;
        let mut w = W::new();
        w.put(0x3ffe, 14);
        w.put(0, 2);
        w.put(6, 4);
        w.put(9, 4);
        w.put(1, 4);
        w.put(4, 3);
        w.put(0, 1);
        w.put(0, 8);
        w.put((n - 1) as u64, 8);
        let crc = crc8(&w.bytes);
        w.put(crc as u64, 8);
        for base in [10i32, -20] {
            w.put(0, 1);
            w.put(8 + 2, 6); // fixed, order 2
            w.put(0, 1);
            w.put(base as u32 as u64 & 0xffff, 16);
            w.put((base + 3) as u32 as u64 & 0xffff, 16);
            w.put(0, 2); // rice method 0
            w.put(0, 4); // one partition
            w.put(0, 4); // rice parameter 0
            for _ in 0..n - 2 {
                w.put(1, 1); // quotient 0, value 0
            }
        }
        w.align();
        let crc = crc16(&w.bytes);
        w.put(crc as u64, 16);
        let d = decode_frame(&w.bytes).unwrap();
        let l: Vec<i16> = d.samples.iter().step_by(2).copied().collect();
        let r: Vec<i16> = d.samples.iter().skip(1).step_by(2).copied().collect();
        assert_eq!(l, [10, 13, 16, 19, 22, 25, 28, 31]);
        assert_eq!(r, [-20, -17, -14, -11, -8, -5, -2, 1]);
    }
}
