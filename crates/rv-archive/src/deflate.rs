//! The raw-deflate encoder TorrentZip needs, ported from ROMVault's own zlib port.
//!
//! Stock zlib does not reproduce the deflate streams in the board's zips (measured: 85 of 92
//! sampled members differ from zlib 1.3 at level 9, memLevel 8), so this is a translation of
//! `Compress/Support/Compression/Deflate/{Deflate,Tree,Zlib}.cs` (a port of zlib 1.1.3): level
//! 9 only (`DeflateSlow`), raw deflate, 32 KiB window, memLevel 8, default strategy
//! (`ZipFileData.cs:567-570`). The `TRUNCATE_BLOCK` heuristic stays off (`Deflate.cs:728-750`).
//! Names follow the C# so the two can be read side by side.

use std::mem;

const MAX_BITS: usize = 15;
const BL_CODES: usize = 19;
const D_CODES: usize = 30;
const LITERALS: usize = 256;
const LENGTH_CODES: usize = 29;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BL_BITS: i32 = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const MIN_MATCH: i32 = 3;
const MAX_MATCH: i32 = 258;
const MIN_LOOKAHEAD: i32 = MAX_MATCH + MIN_MATCH + 1;
const W_BITS: u32 = 15;
const W_SIZE: i32 = 1 << W_BITS;
const W_MASK: i32 = W_SIZE - 1;
/// `memLevel` 8 (`Deflate.cs:93`): `hash_bits = memLevel + 7`, `lit_bufsize = 1 << (memLevel + 6)`
/// (`Deflate.cs:1562,1572`).
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: i32 = (1 << HASH_BITS) - 1;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(MIN_MATCH as u32);
const LIT_BUFSIZE: usize = 1 << 14;

/// Level 9 row of the config table (`Deflate.cs:146`): good, max lazy, nice, max chain.
const GOOD_LENGTH: i32 = 32;
const MAX_LAZY: i32 = 258;
const NICE_LENGTH: i32 = 258;
const MAX_CHAIN: i32 = 4096;
/// A length-3 match farther than this is dropped (`Deflate.cs:1334-1335`).
const TOO_FAR: i32 = 4096;

const EXTRA_LENGTH_BITS: [i32; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DISTANCE_BITS: [i32; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BL_BITS: [i32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// The derived tables of `Tree.cs:99-167` (zlib's `tr_static_init`).
struct Tables {
    dist_code: [u8; 512],
    length_code: [u8; 256],
    length_base: [i32; LENGTH_CODES],
    distance_base: [i32; D_CODES],
    /// Static literal/length tree as `(code, len)` pairs (`Zlib.cs:325-362`).
    static_ltree: [i16; (L_CODES + 2) * 2],
    /// Static distance tree (`Zlib.cs:364-368`).
    static_dtree: [i16; D_CODES * 2],
}

impl Tables {
    fn new() -> Tables {
        let mut length_code = [0u8; 256];
        let mut length_base = [0i32; LENGTH_CODES];
        let mut length = 0usize;
        for code in 0..LENGTH_CODES - 1 {
            length_base[code] = length as i32;
            for _ in 0..(1 << EXTRA_LENGTH_BITS[code]) {
                length_code[length] = code as u8;
                length += 1;
            }
        }
        length_code[length - 1] = (LENGTH_CODES - 1) as u8;

        let mut dist_code = [0u8; 512];
        let mut distance_base = [0i32; D_CODES];
        let mut dist = 0usize;
        for code in 0..16 {
            distance_base[code] = dist as i32;
            for _ in 0..(1 << EXTRA_DISTANCE_BITS[code]) {
                dist_code[dist] = code as u8;
                dist += 1;
            }
        }
        dist >>= 7;
        for code in 16..D_CODES {
            distance_base[code] = (dist << 7) as i32;
            for _ in 0..(1 << (EXTRA_DISTANCE_BITS[code] - 7)) {
                dist_code[256 + dist] = code as u8;
                dist += 1;
            }
        }

        let mut static_ltree = [0i16; (L_CODES + 2) * 2];
        let mut bl_count = [0i16; MAX_BITS + 1];
        for n in 0..L_CODES + 2 {
            let len = if n <= 143 {
                8
            } else if n <= 255 {
                9
            } else if n <= 279 {
                7
            } else {
                8
            };
            static_ltree[n * 2 + 1] = len;
            bl_count[len as usize] += 1;
        }
        gen_codes(&mut static_ltree, L_CODES + 1, &bl_count);
        let mut static_dtree = [0i16; D_CODES * 2];
        for n in 0..D_CODES {
            static_dtree[n * 2 + 1] = 5;
            static_dtree[n * 2] = bi_reverse(n as i32, 5) as i16;
        }
        Tables {
            dist_code,
            length_code,
            length_base,
            distance_base,
            static_ltree,
            static_dtree,
        }
    }

    /// `Tree.DistanceCode` (`Tree.cs:176-181`).
    fn distance_code(&self, dist: i32) -> usize {
        if dist < 256 {
            self.dist_code[dist as usize] as usize
        } else {
            self.dist_code[256 + (dist >> 7) as usize] as usize
        }
    }
}

/// `Tree.gen_codes` (`Tree.cs:375-403`).
fn gen_codes(tree: &mut [i16], max_code: usize, bl_count: &[i16; MAX_BITS + 1]) {
    let mut next_code = [0i16; MAX_BITS + 1];
    let mut code: i16 = 0;
    for bits in 1..=MAX_BITS {
        code = (code.wrapping_add(bl_count[bits - 1])) << 1;
        next_code[bits] = code;
    }
    for n in 0..=max_code {
        let len = tree[n * 2 + 1] as usize;
        if len == 0 {
            continue;
        }
        tree[n * 2] = bi_reverse(next_code[len] as i32, len as i32) as i16;
        next_code[len] = next_code[len].wrapping_add(1);
    }
}

/// `Tree.bi_reverse` (`Tree.cs:408-419`).
fn bi_reverse(mut code: i32, mut len: i32) -> i32 {
    let mut res = 0;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len <= 0 {
            break;
        }
    }
    res >> 1
}

/// Which of the three trees `build_tree` works on (`StaticTree.cs`, `Zlib.cs:388-393`).
#[derive(Clone, Copy)]
enum TreeKind {
    Literals,
    Distances,
    BitLengths,
}

/// LSB-first bit sink; `send_bits` in `Deflate.cs:647-673` buffers 16 bits at a time, which
/// only changes when bytes are flushed, not their values.
struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn send_bits(&mut self, value: i32, length: i32) {
        debug_assert!(length <= 16);
        self.acc |= ((value as u64) & ((1u64 << length) - 1)) << self.n;
        self.n += length as u32;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    fn send_code(&mut self, c: usize, tree: &[i16]) {
        self.send_bits(tree[c * 2] as u16 as i32, tree[c * 2 + 1] as u16 as i32);
    }

    /// `bi_windup` (`Deflate.cs:871-885`).
    fn windup(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.acc = 0;
        self.n = 0;
    }
}

/// A raw deflate stream at TorrentZip settings. Feed it with [`Deflater::write`] and end it
/// with [`Deflater::finish`].
pub struct Deflater {
    tables: Tables,
    window: Vec<u8>,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: i32,
    block_start: i32,
    match_length: i32,
    prev_match: i32,
    match_available: bool,
    strstart: i32,
    match_start: i32,
    lookahead: i32,
    prev_length: i32,
    dyn_ltree: Vec<i16>,
    dyn_dtree: Vec<i16>,
    bl_tree: Vec<i16>,
    l_max_code: i32,
    d_max_code: i32,
    bl_count: [i16; MAX_BITS + 1],
    heap: [i32; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [i8; HEAP_SIZE],
    d_buf: Vec<u16>,
    l_buf: Vec<u8>,
    last_lit: usize,
    opt_len: i32,
    static_len: i32,
    bw: BitWriter,
}

impl Default for Deflater {
    fn default() -> Self {
        Self::new()
    }
}

impl Deflater {
    /// `DeflateManager.Reset` for level 9 (`Deflate.cs:1595-1613`).
    pub fn new() -> Deflater {
        let mut d = Deflater {
            tables: Tables::new(),
            window: vec![0; 2 * W_SIZE as usize],
            prev: vec![0; W_SIZE as usize],
            head: vec![0; HASH_SIZE],
            ins_h: 0,
            block_start: 0,
            match_length: MIN_MATCH - 1,
            prev_match: 0,
            match_available: false,
            strstart: 0,
            match_start: 0,
            lookahead: 0,
            prev_length: MIN_MATCH - 1,
            dyn_ltree: vec![0; HEAP_SIZE * 2],
            dyn_dtree: vec![0; (2 * D_CODES + 1) * 2],
            bl_tree: vec![0; (2 * BL_CODES + 1) * 2],
            l_max_code: 0,
            d_max_code: 0,
            bl_count: [0; MAX_BITS + 1],
            heap: [0; HEAP_SIZE],
            heap_len: 0,
            heap_max: 0,
            depth: [0; HEAP_SIZE],
            d_buf: vec![0; LIT_BUFSIZE],
            l_buf: vec![0; LIT_BUFSIZE],
            last_lit: 0,
            opt_len: 0,
            static_len: 0,
            bw: BitWriter {
                out: Vec::new(),
                acc: 0,
                n: 0,
            },
        };
        d.init_blocks();
        d
    }

    /// Compress `input` (more may follow). Finished bytes are appended to `out`.
    pub fn write(&mut self, mut input: &[u8], out: &mut Vec<u8>) {
        if !input.is_empty() {
            self.deflate_slow(&mut input, false);
        }
        out.append(&mut self.bw.out);
    }

    /// End the stream: the last block gets the final bit set and the output is byte aligned.
    pub fn finish(&mut self, out: &mut Vec<u8>) {
        let mut input: &[u8] = &[];
        self.deflate_slow(&mut input, true);
        out.append(&mut self.bw.out);
    }

    /// `_InitializeBlocks` (`Deflate.cs:386-399`).
    fn init_blocks(&mut self) {
        for i in 0..L_CODES {
            self.dyn_ltree[i * 2] = 0;
        }
        for i in 0..D_CODES {
            self.dyn_dtree[i * 2] = 0;
        }
        for i in 0..BL_CODES {
            self.bl_tree[i * 2] = 0;
        }
        self.dyn_ltree[END_BLOCK * 2] = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.last_lit = 0;
    }

    /// `pqdownheap` (`Deflate.cs:405-426`).
    fn pqdownheap(&mut self, tree: &[i16], mut k: usize) {
        let v = self.heap[k];
        let mut j = k << 1;
        while j <= self.heap_len {
            if j < self.heap_len && is_smaller(tree, self.heap[j + 1], self.heap[j], &self.depth) {
                j += 1;
            }
            if is_smaller(tree, v, self.heap[j], &self.depth) {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = v;
    }

    /// `Tree.build_tree` (`Tree.cs:282-367`).
    fn build_tree(&mut self, kind: TreeKind) {
        let mut tree = match kind {
            TreeKind::Literals => mem::take(&mut self.dyn_ltree),
            TreeKind::Distances => mem::take(&mut self.dyn_dtree),
            TreeKind::BitLengths => mem::take(&mut self.bl_tree),
        };
        let elems = match kind {
            TreeKind::Literals => L_CODES,
            TreeKind::Distances => D_CODES,
            TreeKind::BitLengths => BL_CODES,
        };
        let mut max_code: i32 = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if tree[n * 2] != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n as i32;
                max_code = n as i32;
                self.depth[n] = 0;
            } else {
                tree[n * 2 + 1] = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code
            } else {
                0
            } as usize;
            self.heap_len += 1;
            self.heap[self.heap_len] = node as i32;
            tree[node * 2] = 1;
            self.depth[node] = 0;
            self.opt_len -= 1;
            match kind {
                TreeKind::Literals => {
                    self.static_len -= self.tables.static_ltree[node * 2 + 1] as i32
                }
                TreeKind::Distances => {
                    self.static_len -= self.tables.static_dtree[node * 2 + 1] as i32
                }
                TreeKind::BitLengths => {}
            }
        }
        match kind {
            TreeKind::Literals => self.l_max_code = max_code,
            TreeKind::Distances => self.d_max_code = max_code,
            TreeKind::BitLengths => {}
        }
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(&tree, n);
            n -= 1;
        }
        let mut node = elems;
        loop {
            let n = self.heap[1] as usize;
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(&tree, 1);
            let m = self.heap[1] as usize;
            self.heap_max -= 1;
            self.heap[self.heap_max] = n as i32;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m as i32;
            tree[node * 2] = tree[n * 2].wrapping_add(tree[m * 2]);
            self.depth[node] = (self.depth[n] as u8).max(self.depth[m] as u8) as i8 + 1;
            tree[n * 2 + 1] = node as i16;
            tree[m * 2 + 1] = node as i16;
            self.heap[1] = node as i32;
            node += 1;
            self.pqdownheap(&tree, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(&mut tree, kind, max_code);
        gen_codes(&mut tree, max_code as usize, &self.bl_count);
        match kind {
            TreeKind::Literals => self.dyn_ltree = tree,
            TreeKind::Distances => self.dyn_dtree = tree,
            TreeKind::BitLengths => self.bl_tree = tree,
        }
    }

    /// `Tree.gen_bitlen` (`Tree.cs:195-274`).
    fn gen_bitlen(&mut self, tree: &mut [i16], kind: TreeKind, max_code: i32) {
        let (extra, extra_base, max_length): (&[i32], i32, i32) = match kind {
            TreeKind::Literals => (&EXTRA_LENGTH_BITS, LITERALS as i32 + 1, MAX_BITS as i32),
            TreeKind::Distances => (&EXTRA_DISTANCE_BITS, 0, MAX_BITS as i32),
            TreeKind::BitLengths => (&EXTRA_BL_BITS, 0, MAX_BL_BITS),
        };
        let mut overflow = 0;
        for b in self.bl_count.iter_mut() {
            *b = 0;
        }
        tree[self.heap[self.heap_max] as usize * 2 + 1] = 0;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h] as usize;
            let mut bits = tree[tree[n * 2 + 1] as usize * 2 + 1] as i32 + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n * 2 + 1] = bits as i16;
            h += 1;
            if n as i32 > max_code {
                continue;
            }
            self.bl_count[bits as usize] += 1;
            let mut xbits = 0;
            if n as i32 >= extra_base {
                xbits = extra[n - extra_base as usize];
            }
            let f = tree[n * 2] as i32;
            self.opt_len += f * (bits + xbits);
            match kind {
                TreeKind::Literals => {
                    self.static_len += f * (self.tables.static_ltree[n * 2 + 1] as i32 + xbits)
                }
                TreeKind::Distances => {
                    self.static_len += f * (self.tables.static_dtree[n * 2 + 1] as i32 + xbits)
                }
                TreeKind::BitLengths => {}
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits as usize] == 0 {
                bits -= 1;
            }
            self.bl_count[bits as usize] -= 1;
            self.bl_count[bits as usize + 1] += 2;
            self.bl_count[max_length as usize] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut bits = max_length;
        while bits != 0 {
            let mut n = self.bl_count[bits as usize];
            while n != 0 {
                h -= 1;
                let m = self.heap[h] as usize;
                if m as i32 > max_code {
                    continue;
                }
                if tree[m * 2 + 1] as i32 != bits {
                    self.opt_len +=
                        ((bits as i64 - tree[m * 2 + 1] as i64) * tree[m * 2] as i64) as i32;
                    tree[m * 2 + 1] = bits as i16;
                }
                n -= 1;
            }
            bits -= 1;
        }
    }

    /// `scan_tree` (`Deflate.cs:438-493`).
    fn scan_tree(tree: &mut [i16], bl_tree: &mut [i16], max_code: i32) {
        let mut prevlen: i32 = -1;
        let mut nextlen = tree[1] as i32;
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        tree[(max_code as usize + 1) * 2 + 1] = 0x7fff;
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = tree[(n + 1) * 2 + 1] as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                bl_tree[curlen as usize * 2] += count as i16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    bl_tree[curlen as usize * 2] += 1;
                }
                bl_tree[REP_3_6 * 2] += 1;
            } else if count <= 10 {
                bl_tree[REPZ_3_10 * 2] += 1;
            } else {
                bl_tree[REPZ_11_138 * 2] += 1;
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    /// `build_bl_tree` (`Deflate.cs:497-522`).
    fn build_bl_tree(&mut self) -> usize {
        Self::scan_tree(&mut self.dyn_ltree, &mut self.bl_tree, self.l_max_code);
        Self::scan_tree(&mut self.dyn_dtree, &mut self.bl_tree, self.d_max_code);
        self.build_tree(TreeKind::BitLengths);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex] * 2 + 1] != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len += 3 * (max_blindex as i32 + 1) + 5 + 5 + 4;
        max_blindex
    }

    /// `send_all_trees` (`Deflate.cs:528-541`).
    fn send_all_trees(&mut self, lcodes: i32, dcodes: i32, blcodes: i32) {
        self.bw.send_bits(lcodes - 257, 5);
        self.bw.send_bits(dcodes - 1, 5);
        self.bw.send_bits(blcodes - 4, 4);
        for &order in BL_ORDER.iter().take(blcodes as usize) {
            self.bw.send_bits(self.bl_tree[order * 2 + 1] as i32, 3);
        }
        Self::send_tree(&mut self.bw, &self.dyn_ltree, &self.bl_tree, lcodes - 1);
        Self::send_tree(&mut self.bw, &self.dyn_dtree, &self.bl_tree, dcodes - 1);
    }

    /// `send_tree` (`Deflate.cs:545-608`).
    fn send_tree(bw: &mut BitWriter, tree: &[i16], bl_tree: &[i16], max_code: i32) {
        let mut prevlen: i32 = -1;
        let mut nextlen = tree[1] as i32;
        let mut count = 0;
        let mut max_count = 7;
        let mut min_count = 4;
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = tree[(n + 1) * 2 + 1] as i32;
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                loop {
                    bw.send_code(curlen as usize, bl_tree);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    bw.send_code(curlen as usize, bl_tree);
                    count -= 1;
                }
                bw.send_code(REP_3_6, bl_tree);
                bw.send_bits(count - 3, 2);
            } else if count <= 10 {
                bw.send_code(REPZ_3_10, bl_tree);
                bw.send_bits(count - 3, 3);
            } else {
                bw.send_code(REPZ_11_138, bl_tree);
                bw.send_bits(count - 11, 7);
            }
            count = 0;
            prevlen = curlen;
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    /// `_tr_tally` (`Deflate.cs:707-757`): true when the block is full.
    fn tr_tally(&mut self, dist: i32, lc: i32) -> bool {
        self.d_buf[self.last_lit] = dist as u16;
        self.l_buf[self.last_lit] = lc as u8;
        self.last_lit += 1;
        if dist == 0 {
            self.dyn_ltree[lc as usize * 2] += 1;
        } else {
            let dist = dist - 1;
            let lcode = self.tables.length_code[lc as usize] as usize;
            self.dyn_ltree[(lcode + LITERALS + 1) * 2] += 1;
            let dcode = self.tables.distance_code(dist);
            self.dyn_dtree[dcode * 2] += 1;
        }
        self.last_lit == LIT_BUFSIZE - 1 || self.last_lit == LIT_BUFSIZE
    }

    /// `send_compressed_block` (`Deflate.cs:762-821`).
    fn send_compressed_block(&mut self, fixed: bool) {
        for lx in 0..self.last_lit {
            let mut distance = self.d_buf[lx] as i32;
            let mut lc = self.l_buf[lx] as i32;
            let (ltree, dtree): (&[i16], &[i16]) = if fixed {
                (&self.tables.static_ltree, &self.tables.static_dtree)
            } else {
                (&self.dyn_ltree, &self.dyn_dtree)
            };
            if distance == 0 {
                self.bw.send_code(lc as usize, ltree);
            } else {
                let code = self.tables.length_code[lc as usize] as usize;
                self.bw.send_code(code + LITERALS + 1, ltree);
                let extra = EXTRA_LENGTH_BITS[code];
                if extra != 0 {
                    lc -= self.tables.length_base[code];
                    self.bw.send_bits(lc, extra);
                }
                distance -= 1;
                let code = self.tables.distance_code(distance);
                self.bw.send_code(code, dtree);
                let extra = EXTRA_DISTANCE_BITS[code];
                if extra != 0 {
                    distance -= self.tables.distance_base[code];
                    self.bw.send_bits(distance, extra);
                }
            }
        }
        let ltree: &[i16] = if fixed {
            &self.tables.static_ltree
        } else {
            &self.dyn_ltree
        };
        self.bw.send_code(END_BLOCK, ltree);
    }

    /// `_tr_stored_block` + `copy_block` (`Deflate.cs:889-906,983-987`).
    fn tr_stored_block(&mut self, buf: i32, stored_len: i32, eof: bool) {
        self.bw.send_bits(eof as i32, 3);
        self.bw.windup();
        let len = stored_len as u32;
        self.bw.out.push(len as u8);
        self.bw.out.push((len >> 8) as u8);
        self.bw.out.push(!len as u8);
        self.bw.out.push((!len >> 8) as u8);
        self.bw
            .out
            .extend_from_slice(&self.window[buf as usize..buf as usize + stored_len as usize]);
    }

    /// `_tr_flush_block` (`Deflate.cs:991-1058`).
    fn tr_flush_block(&mut self, buf: i32, stored_len: i32, eof: bool) {
        self.build_tree(TreeKind::Literals);
        self.build_tree(TreeKind::Distances);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = (self.opt_len + 3 + 7) >> 3;
        let static_lenb = (self.static_len + 3 + 7) >> 3;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        if stored_len + 4 <= opt_lenb && buf != -1 {
            self.tr_stored_block(buf, stored_len, eof);
        } else if static_lenb == opt_lenb {
            self.bw.send_bits((1 << 1) + eof as i32, 3);
            self.send_compressed_block(true);
        } else {
            self.bw.send_bits((2 << 1) + eof as i32, 3);
            self.send_all_trees(
                self.l_max_code + 1,
                self.d_max_code + 1,
                max_blindex as i32 + 1,
            );
            self.send_compressed_block(false);
        }
        self.init_blocks();
        if eof {
            self.bw.windup();
        }
    }

    /// `flush_block_only` (`Deflate.cs:908-913`).
    fn flush_block_only(&mut self, eof: bool) {
        let buf = if self.block_start >= 0 {
            self.block_start
        } else {
            -1
        };
        self.tr_flush_block(buf, self.strstart - self.block_start, eof);
        self.block_start = self.strstart;
    }

    /// `_fillWindow` (`Deflate.cs:1068-1154`); `read_buf` takes from `input`.
    fn fill_window(&mut self, input: &mut &[u8]) {
        loop {
            let mut more = 2 * W_SIZE - self.lookahead - self.strstart;
            if more == 0 && self.strstart == 0 && self.lookahead == 0 {
                more = W_SIZE;
            } else if more == -1 {
                more -= 1;
            } else if self.strstart >= W_SIZE + W_SIZE - MIN_LOOKAHEAD {
                self.window.copy_within(W_SIZE as usize.., 0);
                self.match_start -= W_SIZE;
                self.strstart -= W_SIZE;
                self.block_start -= W_SIZE;
                for h in self.head.iter_mut() {
                    let m = *h as i32;
                    *h = if m >= W_SIZE { (m - W_SIZE) as u16 } else { 0 };
                }
                for p in self.prev.iter_mut() {
                    let m = *p as i32;
                    *p = if m >= W_SIZE { (m - W_SIZE) as u16 } else { 0 };
                }
                more += W_SIZE;
            }
            if input.is_empty() {
                return;
            }
            let n = (more as usize).min(input.len());
            let at = (self.strstart + self.lookahead) as usize;
            self.window[at..at + n].copy_from_slice(&input[..n]);
            *input = &input[n..];
            self.lookahead += n as i32;
            if self.lookahead >= MIN_MATCH {
                let s = self.strstart as usize;
                self.ins_h = self.window[s] as i32;
                self.ins_h = ((self.ins_h << HASH_SHIFT) ^ self.window[s + 1] as i32) & HASH_MASK;
            }
            if !(self.lookahead < MIN_LOOKAHEAD && !input.is_empty()) {
                break;
            }
        }
    }

    /// `DeflateSlow` (`Deflate.cs:1280-1429`) without the output-full early returns: output
    /// is an unbounded buffer here, so they never trigger.
    fn deflate_slow(&mut self, input: &mut &[u8], finish: bool) {
        let mut hash_head: i32 = 0;
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window(input);
                if self.lookahead < MIN_LOOKAHEAD && !finish {
                    return;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            if self.lookahead >= MIN_MATCH {
                let s = self.strstart as usize;
                self.ins_h = ((self.ins_h << HASH_SHIFT) ^ self.window[s + 2] as i32) & HASH_MASK;
                hash_head = self.head[self.ins_h as usize] as i32;
                self.prev[s & W_MASK as usize] = self.head[self.ins_h as usize];
                self.head[self.ins_h as usize] = self.strstart as u16;
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != 0
                && self.prev_length < MAX_LAZY
                && ((self.strstart - hash_head) & 0xffff) <= W_SIZE - MIN_LOOKAHEAD
            {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && self.match_length == MIN_MATCH
                    && self.strstart - self.match_start > TOO_FAR
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tr_tally(
                    self.strstart - 1 - self.prev_match,
                    self.prev_length - MIN_MATCH,
                );
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        let s = self.strstart as usize;
                        self.ins_h =
                            ((self.ins_h << HASH_SHIFT) ^ self.window[s + 2] as i32) & HASH_MASK;
                        hash_head = self.head[self.ins_h as usize] as i32;
                        self.prev[s & W_MASK as usize] = self.head[self.ins_h as usize];
                        self.head[self.ins_h as usize] = self.strstart as u16;
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block_only(false);
                }
            } else if self.match_available {
                let bflush = self.tr_tally(0, self.window[self.strstart as usize - 1] as i32);
                if bflush {
                    self.flush_block_only(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            self.tr_tally(0, self.window[self.strstart as usize - 1] as i32);
            self.match_available = false;
        }
        self.flush_block_only(finish);
    }

    /// `longest_match` (`Deflate.cs:1432-1517`).
    fn longest_match(&mut self, mut cur_match: i32) -> i32 {
        let mut chain_length = MAX_CHAIN;
        let mut scan = self.strstart;
        let mut best_len = self.prev_length;
        let limit = if self.strstart > W_SIZE - MIN_LOOKAHEAD {
            self.strstart - (W_SIZE - MIN_LOOKAHEAD)
        } else {
            0
        };
        let mut nice_length = NICE_LENGTH;
        let strend = self.strstart + MAX_MATCH;
        let w = &self.window;
        let mut scan_end1 = w[(scan + best_len - 1) as usize];
        let mut scan_end = w[(scan + best_len) as usize];
        if self.prev_length >= GOOD_LENGTH {
            chain_length >>= 2;
        }
        if nice_length > self.lookahead {
            nice_length = self.lookahead;
        }
        loop {
            let mut matchp = cur_match;
            let ok = w[(matchp + best_len) as usize] == scan_end
                && w[(matchp + best_len - 1) as usize] == scan_end1
                && w[matchp as usize] == w[scan as usize]
                && {
                    matchp += 1;
                    w[matchp as usize] == w[scan as usize + 1]
                };
            if ok {
                scan += 2;
                matchp += 1;
                loop {
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    scan += 1;
                    matchp += 1;
                    if w[scan as usize] != w[matchp as usize] {
                        break;
                    }
                    if scan >= strend {
                        break;
                    }
                }
                let len = MAX_MATCH - (strend - scan);
                scan = strend - MAX_MATCH;
                if len > best_len {
                    self.match_start = cur_match;
                    best_len = len;
                    if len >= nice_length {
                        break;
                    }
                    scan_end1 = w[(scan + best_len - 1) as usize];
                    scan_end = w[(scan + best_len) as usize];
                }
            }
            cur_match = self.prev[(cur_match & W_MASK) as usize] as i32;
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        if best_len <= self.lookahead {
            best_len
        } else {
            self.lookahead
        }
    }
}

/// `_IsSmaller` (`Deflate.cs:428-433`).
fn is_smaller(tree: &[i16], n: i32, m: i32, depth: &[i8; HEAP_SIZE]) -> bool {
    let tn2 = tree[n as usize * 2];
    let tm2 = tree[m as usize * 2];
    tn2 < tm2 || (tn2 == tm2 && depth[n as usize] <= depth[m as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn compress(data: &[u8], chunk: usize) -> Vec<u8> {
        let mut d = Deflater::new();
        let mut out = Vec::new();
        for c in data.chunks(chunk.max(1)) {
            d.write(c, &mut out);
        }
        d.finish(&mut out);
        out
    }

    fn inflate(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        flate2::read::DeflateDecoder::new(raw)
            .read_to_end(&mut out)
            .unwrap();
        out
    }

    /// xorshift, so every run sees the same bytes.
    fn pseudo_random(n: usize, mut x: u64) -> Vec<u8> {
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 24) as u8
            })
            .collect()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn nothing_written_is_the_empty_static_block() {
        // The two bytes ROMVault appends for an empty file (`ZipFileData.cs:630-634`).
        assert_eq!(compress(&[], 1), [0x03, 0x00]);
    }

    /// Short inputs have no block-splitting or match-choice freedom, so they agree with stock zlib
    /// level 9 (`zlib.compressobj(9, DEFLATED, -15, 8)`, checked when these were written). Long
    /// inputs do not: see `tests/fixtures.rs` for the byte-exactness evidence.
    #[test]
    fn short_inputs_match_the_known_stream() {
        assert_eq!(
            hex(&compress(b"hello hello hello hello", 100)),
            "cb48cdc9c957c8402701"
        );
        assert_eq!(hex(&compress(&[b'a'; 1000], 100)), "4b4c1c05a360140c770000");
        assert_eq!(
            hex(&compress(&b"abcabcabcabcxyz".repeat(20), 7)),
            "4b4c4a4e84a18acaaac4512e6e2e00"
        );
    }

    #[test]
    fn round_trips_through_an_independent_inflater() {
        let text: Vec<u8> = (0..30_000)
            .map(|i| format!("line {i}: the quick brown fox\n"))
            .collect::<String>()
            .into_bytes();
        let mut mixed = pseudo_random(70_000, 1);
        mixed.extend(text.iter().copied());
        mixed.extend(vec![0u8; 100_000]);
        mixed.extend(pseudo_random(5_000, 2));
        for data in [
            Vec::new(),
            vec![7],
            b"ab".to_vec(),
            text,
            pseudo_random(300_000, 3),
            vec![0u8; 1_000_000],
            mixed,
        ] {
            assert_eq!(
                inflate(&compress(&data, 1 << 20)),
                data,
                "len {}",
                data.len()
            );
        }
    }

    #[test]
    fn output_does_not_depend_on_how_the_input_is_chunked() {
        let mut data = pseudo_random(40_000, 9);
        data.extend((0..120_000u32).map(|i| (i / 7 % 251) as u8));
        let whole = compress(&data, data.len());
        for chunk in [1usize, 3, 100, 4096, 65_536, 1 << 20] {
            assert_eq!(compress(&data, chunk), whole, "chunk {chunk}");
        }
    }

    #[test]
    fn incompressible_data_becomes_stored_blocks() {
        let data = pseudo_random(20_000, 5);
        let out = compress(&data, 1 << 20);
        assert_eq!(inflate(&out), data);
        assert!(out.len() > data.len(), "stored blocks add headers");
        assert!(out.len() < data.len() + 64);
    }

    #[test]
    fn blocks_end_at_16383_symbols() {
        // Two million bytes over a 16-letter alphabet give many blocks; the stream must decode
        // when it is split by the symbol count.
        let data: Vec<u8> = (0..2_000_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 28) as u8)
            .collect();
        assert_eq!(inflate(&compress(&data, 65_536)), data);
    }
}
