"""Make flac_pcm.raw (16-bit little-endian stereo PCM) and flac_*.frames (the frames libFLAC
writes for it, with the `fLaC` marker and metadata blocks cut off, which is how CHD stores them).
Run from this directory: `python3 -I gen_flac.py` (needs the `flac` command line tool).
"""
import math, random, struct, subprocess, tempfile, os


def pcm():
    rng = random.Random(5)
    n = 8192
    out = bytearray()
    for i in range(n):
        t = i / 44100.0
        base = 9000 * math.sin(2 * math.pi * 440 * t) + 3000 * math.sin(2 * math.pi * 1250 * t)
        noise = rng.randrange(-300, 300)
        # Left: every sample a multiple of 8 (exercises wasted bits); right follows left closely
        # (mid/side wins); a burst of loud noise near the end makes the residuals large.
        left = int(base + noise) & ~7
        right = int(base * 0.9 + rng.randrange(-40, 40))
        if i > n - 700:
            left = (rng.randrange(-30000, 30000)) & ~7
            right = rng.randrange(-30000, 30000)
        out += struct.pack("<hh", max(-32768, min(32767, left)), max(-32768, min(32767, right)))
    return bytes(out)


def frames(flac_bytes):
    assert flac_bytes[:4] == b"fLaC"
    pos = 4
    while True:
        last = flac_bytes[pos] & 0x80
        size = int.from_bytes(flac_bytes[pos + 1:pos + 4], "big")
        pos += 4 + size
        if last:
            return flac_bytes[pos:]


raw = pcm()
open("flac_pcm.raw", "wb").write(raw)
variants = {
    "m0": ["-0", "-b", "1152"],
    "m3": ["-3", "-b", "576"],
    "m5": ["-5", "-b", "4096"],
    "m8": ["-8", "-b", "4096"],
    "m8_nomid": ["-8", "-b", "2048", "--no-mid-side"],
    "m8_loose": ["-8", "-b", "1024", "-M"],
    "m6_exh": ["-6", "-b", "4608", "-e", "-p"],
}
with tempfile.TemporaryDirectory() as d:
    for name, args in variants.items():
        out = os.path.join(d, name + ".flac")
        subprocess.run(
            ["flac", "--force-raw-format", "--endian=little", "--sign=signed", "--channels=2", "--bps=16",
             "--sample-rate=44100", "-s", "-f", "--no-padding", "--no-seektable", *args, "-o", out, "flac_pcm.raw"],
            check=True)
        open(f"flac_{name}.frames", "wb").write(frames(open(out, "rb").read()))
