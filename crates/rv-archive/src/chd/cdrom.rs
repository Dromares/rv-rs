//! The CD sector error-correction codes CHD leaves out of `cdzl`, `cdzs` and `cdlz` hunks and
//! `ecc_generate` writes back (`CHD/CHDSharpLib/Utils/cdRom.cs:294-303`, ECMA-130 annex A).
//!
//! The C# keeps four precomputed tables; here they are computed. `ecclow` is multiplication by
//! x in GF(2^8) with the polynomial 0x11d, `ecchigh` is its inverse trick
//! (`ecchigh[i ^ ecclow[i]] = i`), the P rows are `i + 86 c` and the Q rows
//! `(86 (i / 2) + (i % 2) + 88 c) mod 2236` (`cdRom.cs` `poffsets` and `qoffsets`, checked
//! against every entry of the C# tables when this was written).

const SYNC_NUM_BYTES: usize = 12;
const MODE_OFFSET: usize = 0x0f;
const ECC_P_OFFSET: usize = 0x81c;
const ECC_P_NUM_BYTES: usize = 86;
const ECC_P_COMP: usize = 24;
const ECC_Q_OFFSET: usize = ECC_P_OFFSET + 2 * ECC_P_NUM_BYTES;
const ECC_Q_NUM_BYTES: usize = 52;
const ECC_Q_COMP: usize = 43;

struct Tables {
    low: [u8; 256],
    high: [u8; 256],
}

fn tables() -> &'static Tables {
    static TABLES: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    TABLES.get_or_init(build_tables)
}

fn build_tables() -> Tables {
    let mut low = [0u8; 256];
    let mut high = [0u8; 256];
    for i in 0..256usize {
        let j = ((i << 1) ^ if i & 0x80 != 0 { 0x11d } else { 0 }) & 0xff;
        low[i] = j as u8;
        high[i ^ j] = i as u8;
    }
    Tables { low, high }
}

/// `ecc_source_byte`: in mode 2 the first four bytes after the sync count as zero.
fn source_byte(sector: &[u8], offset: usize) -> u8 {
    if sector[MODE_OFFSET] == 2 && offset < 4 {
        0
    } else {
        sector[SYNC_NUM_BYTES + offset]
    }
}

/// `ecc_compute_bytes`: one P or Q parity pair over `row`.
fn compute(t: &Tables, sector: &[u8], row: impl Iterator<Item = usize>) -> (u8, u8) {
    let (mut val1, mut val2) = (0u8, 0u8);
    for offset in row {
        let b = source_byte(sector, offset);
        val1 ^= b;
        val2 ^= b;
        val1 = t.low[val1 as usize];
    }
    val1 = t.high[(t.low[val1 as usize] ^ val2) as usize];
    val2 ^= val1;
    (val1, val2)
}

/// `ecc_generate`: write the P and Q parity of the 2352-byte `sector` over any codes in it.
pub(super) fn ecc_generate(sector: &mut [u8]) {
    debug_assert!(sector.len() >= 2352);
    let t = tables();
    for i in 0..ECC_P_NUM_BYTES {
        let (a, b) = compute(t, sector, (0..ECC_P_COMP).map(|c| i + ECC_P_NUM_BYTES * c));
        sector[ECC_P_OFFSET + i] = a;
        sector[ECC_P_OFFSET + ECC_P_NUM_BYTES + i] = b;
    }
    for i in 0..ECC_Q_NUM_BYTES {
        let start = 86 * (i / 2) + (i % 2);
        let (a, b) = compute(t, sector, (0..ECC_Q_COMP).map(|c| (start + 88 * c) % 2236));
        sector[ECC_Q_OFFSET + i] = a;
        sector[ECC_Q_OFFSET + ECC_Q_NUM_BYTES + i] = b;
    }
}
