#!/usr/bin/env python3
"""Produce corpus/valid/signed-openssl-cms.pdf — a signed PDF whose CMS this engine did not make.

The engine's signature revision is laid out by the engine itself (`prismpdf sign` with the
throwaway test signer), which gives a correct /ByteRange and a /Contents hole of the reserved
width. The detached CMS inside that hole is then replaced by one OpenSSL produces over exactly
the bytes /ByteRange covers (ISO 32000-1 §12.8.1), hex-encoded and zero-padded to the hole. The
result is a signature by another implementation, which is what the verifier had never been run
against before: its attribute set, its DER choices, and its certificate placement are OpenSSL's,
not ours.

Run from the repository root; needs `cargo` and `openssl` (3.x). Deterministic apart from the
CMS's signing time, so regenerate only on purpose.
"""
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SEED = ROOT / "corpus/valid/minimal-2page.pdf"
CERT = ROOT / "crates/pdf/examples/test-signer/cert.der"
KEY = ROOT / "crates/pdf/examples/test-signer/key.der"
OUT = ROOT / "corpus/valid/signed-openssl-cms.pdf"


def run(*args, **kw):
    print("$", " ".join(str(a) for a in args))
    return subprocess.run(args, check=True, **kw)


with tempfile.TemporaryDirectory() as tmp:
    tmp = Path(tmp)
    engine_signed = tmp / "engine-signed.pdf"
    run("cargo", "run", "-q", "-p", "prismpdf-cli", "--", "sign", SEED, engine_signed, CERT, KEY, cwd=ROOT)
    pdf = bytearray(engine_signed.read_bytes())

    # The last revision's /ByteRange [a b c d] and the /Contents hole between the two ranges.
    ranges = list(re.finditer(rb"/ByteRange \[(\d+) (\d+) (\d+) (\d+)\]", pdf))
    a, b, c, d = (int(x) for x in ranges[-1].groups())
    covered = bytes(pdf[a : a + b]) + bytes(pdf[c : c + d])
    assert pdf[a + b] == ord("<") and pdf[c - 1] == ord(">"), "the hole is the /Contents hex string"
    hole = c - (a + b) - 2

    covered_path = tmp / "covered.bin"
    covered_path.write_bytes(covered)
    # OpenSSL 3's `cms -sign` reads its signer in PEM only, so the DER pair is converted first.
    cert_pem, key_pem = tmp / "cert.pem", tmp / "key.pem"
    run("openssl", "x509", "-inform", "DER", "-in", CERT, "-out", cert_pem)
    run("openssl", "pkey", "-inform", "DER", "-in", KEY, "-out", key_pem)
    cms_path = tmp / "openssl.cms"
    run(
        "openssl", "cms", "-sign", "-binary", "-outform", "DER", "-md", "sha256", "-nosmimecap",
        "-in", covered_path, "-signer", cert_pem, "-inkey", key_pem, "-out", cms_path,
    )
    # OpenSSL's own reading of what it just produced, against the same bytes.
    run(
        "openssl", "cms", "-verify", "-inform", "DER", "-in", cms_path, "-content", covered_path,
        "-binary", "-noverify", "-out", tmp / "verified.bin",
    )
    assert (tmp / "verified.bin").read_bytes() == covered

    hexed = cms_path.read_bytes().hex().upper().encode()
    if len(hexed) > hole:
        sys.exit(f"OpenSSL's CMS ({len(hexed)} hex digits) exceeds the reserved hole ({hole})")
    pdf[a + b + 1 : c - 1] = hexed + b"0" * (hole - len(hexed))
    OUT.write_bytes(pdf)
    print(f"wrote {OUT.relative_to(ROOT)} ({len(pdf)} bytes, CMS {len(hexed) // 2} bytes in a {hole // 2}-byte hole)")
    run("cargo", "run", "-q", "-p", "prismpdf-cli", "--", "verify", OUT, cwd=ROOT)
