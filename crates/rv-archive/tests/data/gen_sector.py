"""Make mode1_sector.bin: one synthetic CD-ROM mode 1 sector (sync, header, 2048 pseudo-random
bytes, EDC, P and Q parity), the parity computed the way ECMA-130 annex A and Neill Corlett's
`ecm` do, independently of the code under test. Run from this directory: `python3 -I gen_sector.py`.
"""
import random, struct

f = [((i << 1) ^ (0x11D if i & 0x80 else 0)) & 0xFF for i in range(256)]
b = [0] * 256
for i in range(256):
    b[i ^ f[i]] = i


def block(src, major_count, minor_count, major_mult, minor_inc):
    size = major_count * minor_count
    dest = [0] * (2 * major_count)
    for major in range(major_count):
        index = (major >> 1) * major_mult + (major & 1)
        a = bb = 0
        for minor in range(minor_count):
            t = src[index]
            index += minor_inc
            if index >= size:
                index -= size
            a ^= t
            bb ^= t
            a = f[a]
        a = b[f[a] ^ bb]
        dest[major] = a
        dest[major + major_count] = a ^ bb
    return bytes(dest)


def edc(data):
    tbl = []
    for i in range(256):
        e = i
        for _ in range(8):
            e = (e >> 1) ^ (0xD8018001 if e & 1 else 0)
        tbl.append(e)
    c = 0
    for x in data:
        c = (c >> 8) ^ tbl[(c ^ x) & 0xFF]
    return struct.pack("<I", c)


rng = random.Random(7)
s = bytearray(2352)
s[1:11] = b"\xff" * 10
s[12:16] = bytes([0x00, 0x02, 0x00, 0x01])  # minute 0, second 2, frame 0, mode 1
s[16:2064] = bytes(rng.randrange(256) for _ in range(2048))
s[2064:2068] = edc(bytes(s[0:2064]))
s[2076:2248] = block(bytes(s[12:12 + 2064]), 86, 24, 2, 86)
s[2248:2352] = block(bytes(s[12:12 + 2236]), 52, 43, 86, 88)
open("mode1_sector.bin", "wb").write(bytes(s))
