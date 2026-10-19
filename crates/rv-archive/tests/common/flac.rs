//! Verbatim FLAC frames for the CHD tests (16-bit stereo, independent channels).

#![allow(dead_code)]

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

/// One frame holding `samples` (interleaved left, right), each channel a verbatim subframe.
pub fn verbatim_frame(frame_no: u8, samples: &[i16]) -> Vec<u8> {
    let n = samples.len() / 2;
    assert!((1..=65536).contains(&n) && frame_no < 128);
    let mut b = Vec::new();
    b.extend([0xff, 0xf8]); // sync, fixed block size
    b.push(0x70 | 9); // block size: 16 bits follow (code 7), 44.1 kHz
    b.push((1 << 4) | (4 << 1)); // two independent channels (code 1), 16 bits per sample
    b.push(frame_no);
    b.extend(((n - 1) as u16).to_be_bytes());
    let c = crc8(&b);
    b.push(c);
    for ch in 0..2 {
        b.push(0x02); // padding bit, subframe type verbatim (000001), no wasted bits
        for i in 0..n {
            b.extend(samples[2 * i + ch].to_be_bytes());
        }
    }
    let c = crc16(&b);
    b.extend(c.to_be_bytes());
    b
}
