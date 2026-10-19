"""Generate the small synthetic zips in this directory with independent encoders (Python's
zipfile, p7zip and the zstd CLI). Run from this directory: `python3 -I gen_zips.py`.
Every file holds `sample.txt` = sample_text(), which tests/zip_methods.rs rebuilds.
"""
import io, os, struct, subprocess, tempfile, zipfile, zlib


def sample_text():
    return "".join(f"line {i}: the quick brown fox jumps over the lazy dog\n" for i in range(200)).encode()


def put(name, data):
    with open(name, "wb") as f:
        f.write(data)


def py_zip(name, method, entries=None, **kw):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", method) as z:
        for n, d in (entries or [("sample.txt", sample_text())]):
            z.writestr(n, d, **kw)
    put(name, buf.getvalue())


def sevenzip_zip(name, method):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "sample.txt")
        put(src, sample_text())
        out = os.path.join(d, "o.zip")
        subprocess.run(["7z", "a", "-tzip", f"-mm={method}", "-mx=9", out, src], check=True, capture_output=True)
        put(name, open(out, "rb").read())


def manual_zip(name, method, payload, data, extra_header=b""):
    """One-entry zip with an arbitrary method and raw payload."""
    fname = b"sample.txt"
    crc = zlib.crc32(data)
    local = struct.pack("<IHHHHHIIIHH", 0x04034B50, 63, 0, method, 0, 0x21, crc, len(payload), len(data), len(fname), 0) + fname
    body = local + payload
    cd = struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, 63, 63, 0, method, 0, 0x21, crc, len(payload), len(data), len(fname), 0, 0, 0, 0, 0, 0) + fname
    eocd = struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, 1, 1, len(cd), len(body), 0)
    put(name, body + cd + eocd)


py_zip("method_stored.zip", zipfile.ZIP_STORED)
py_zip("method_deflate.zip", zipfile.ZIP_DEFLATED)
py_zip("method_bzip2.zip", zipfile.ZIP_BZIP2)
py_zip("method_lzma.zip", zipfile.ZIP_LZMA)
sevenzip_zip("method_deflate64.zip", "Deflate64")
sevenzip_zip("method_ppmd.zip", "PPMd")
with tempfile.TemporaryDirectory() as d:
    src = os.path.join(d, "s")
    put(src, sample_text())
    subprocess.run(["zstd", "-19", "-q", "-f", src, "-o", src + ".zst"], check=True)
    manual_zip("method_zstd.zip", 93, open(src + ".zst", "rb").read(), sample_text())

# zip64 local and central extra fields, forced on a small file.
buf = io.BytesIO()
with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
    with z.open("sample.txt", "w", force_zip64=True) as f:
        f.write(sample_text())
put("zip64_forced.zip", buf.getvalue())

# Data descriptors (general purpose bit 3): written to a stream that cannot seek.
class NoSeek(io.RawIOBase):
    def __init__(self):
        self.b = bytearray()
    def writable(self):
        return True
    def write(self, d):
        self.b += d
        return len(d)
ns = NoSeek()
with zipfile.ZipFile(ns, "w", zipfile.ZIP_DEFLATED) as z:
    z.writestr("sample.txt", sample_text())
put("data_descriptor.zip", bytes(ns.b))

# A name that needs the UTF-8 flag, and one that is code page 437.
py_zip("names.zip", zipfile.ZIP_STORED, [("café.txt", b"cp437"), ("あ.txt", b"utf8")])
