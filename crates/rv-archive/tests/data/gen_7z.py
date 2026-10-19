"""Generate the small synthetic 7z archives in this directory with p7zip.
Run from this directory: `python3 -I gen_7z.py`. tests/sevenzip.rs rebuilds the contents.
"""
import os, shutil, subprocess, tempfile


def sample_text():
    return "".join(f"line {i}: the quick brown fox jumps over the lazy dog\n" for i in range(200)).encode()


def tree(root):
    os.makedirs(os.path.join(root, "dir", "sub"))
    os.makedirs(os.path.join(root, "emptydir"))
    files = {
        "a.txt": sample_text(),
        "b.bin": bytes(range(256)) * 40,
        "dir/c.txt": b"c" * 5000,
        "dir/sub/d.txt": b"deep file\n",
        "empty.txt": b"",
    }
    for n, d in files.items():
        with open(os.path.join(root, n), "wb") as f:
            f.write(d)


def make(out, *args, times=False):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "src")
        os.makedirs(src)
        tree(src)
        arc = os.path.join(d, "o.7z")
        subprocess.run(["7z", "a", "-t7z", "-bd", *([] if times else ["-mtm-", "-mtc-", "-mta-"]), *args, arc, "."], cwd=src, check=True, capture_output=True)
        shutil.copy(arc, out)


make("seven_lzma2_solid.7z", "-m0=LZMA2", "-ms=on")
make("seven_lzma_nonsolid.7z", "-m0=LZMA", "-ms=off")
make("seven_copy.7z", "-m0=Copy", "-ms=on")
make("seven_ppmd.7z", "-m0=PPMd", "-ms=on")
make("seven_bzip2.7z", "-m0=BZip2", "-ms=on")
make("seven_deflate.7z", "-m0=Deflate", "-ms=on")
make("seven_bcj_lzma.7z", "-m0=BCJ", "-m1=LZMA", "-ms=on")
make("seven_delta_lzma2.7z", "-m0=Delta:4", "-m1=LZMA2", "-ms=on")
make("seven_plain_header.7z", "-m0=LZMA2", "-mhc=off", "-ms=on")
make("seven_with_times.7z", "-m0=LZMA2", "-mtm=on", "-mtc=on", "-mta=on", times=True)


# Branch-converter filters need data with branches in it: pseudo-random bytes, with x86 call
# opcodes planted. One 16 KiB file per filter, stored in an archive with that filter + LZMA.
def filter_data():
    import random
    rng = random.Random(11)
    data = bytearray(rng.randrange(256) for _ in range(16384))
    for i in range(0, len(data) - 8, 61):
        data[i] = 0xE8 if (i // 61) % 2 else 0xE9
        data[i + 4] = 0x00 if (i // 61) % 3 else 0xFF
    return bytes(data)


def make_filter(out, flt, coder):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "src")
        os.makedirs(src)
        with open(os.path.join(src, "branches.bin"), "wb") as f:
            f.write(filter_data())
        arc = os.path.join(d, "o.7z")
        subprocess.run(["7z", "a", "-t7z", "-bd", "-mtm-", "-mtc-", "-mta-", f"-m0={flt}", f"-m1={coder}", arc, "branches.bin"], cwd=src, check=True, capture_output=True)
        shutil.copy(arc, out)


for flt in ["BCJ", "ARM", "ARMT", "PPC", "SPARC", "IA64"]:
    make_filter(f"seven_filter_{flt.lower()}_lzma.7z", flt, "LZMA")
make_filter("seven_filter_bcj_lzma2.7z", "BCJ", "LZMA2")
make_filter("seven_filter_delta4_lzma2.7z", "Delta:4", "LZMA2")

with open("branches.bin", "wb") as f:
    f.write(filter_data())

# Folders this reader refuses: BCJ2 (four input streams) and AES, with the header open and shut.
make("seven_bcj2.7z", "-m0=BCJ2", "-m1=LZMA:d20", "-m2=LZMA:d19", "-m3=LZMA:d19", "-mb0:1", "-mb0s1:2", "-mb0s2:3")
make("seven_aes.7z", "-m0=LZMA2", "-p123", "-mhe=off")
make("seven_aes_header.7z", "-m0=LZMA2", "-p123", "-mhe=on")
