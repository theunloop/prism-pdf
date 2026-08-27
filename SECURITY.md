# Security Policy

Prism PDF parses **untrusted, frequently malformed, sometimes hostile** input — that is its whole
job (see `DESIGN.md` §3.4 and §7). Memory-safety and denial-of-service robustness are therefore
treated as correctness, not as nice-to-haves.

## Supported versions

The project is pre-1.0 (`0.x`). Only the latest released version (and `main`) receive security
fixes until a stable line is declared.

## Reporting a vulnerability

**Please do not open a public issue for security problems.** Use GitHub's private vulnerability
reporting on this repository (the **Security → Report a vulnerability** tab), which opens a
private advisory visible only to the maintainers.

Include, as far as you can: the affected version/commit, a minimal reproducing input (a PDF or
byte sequence), what happens (crash, hang, excessive memory, out-of-bounds, etc.), and the
impact you believe it has. A crashing or hanging input file is a perfectly good report on its
own.

We aim to acknowledge a report within a few days, agree an embargo window for a fix, and credit
reporters who want it.

## What counts as a vulnerability here

Because the threat model is hostile input crossing a trust boundary, the following are in scope
even without an obvious "exploit":

- Panics, aborts, or unwinding that reach across the FFI boundary (`DESIGN.md` §6.1).
- Out-of-bounds reads/writes or other memory-unsafety (should be impossible in the
  `#![forbid(unsafe_code)]` core — a report proving otherwise is high severity).
- Denial of service: unbounded memory (decompression bombs), non-terminating loops (reference
  cycles), or pathological CPU from a small input. Anti-DoS limits are a first-class feature, so
  a bypass of them is a vulnerability.
- Incorrect cryptographic verification: a signature (§12.8) that verifies when it should not, or an
  encryption/decryption flaw (§7.6) — both implemented (EPIC 9).

## What does not

- Failing to *parse* a malformed file is a bug, not a vulnerability, **unless** the failure mode
  is a crash/hang/OOM rather than a clean `Err`.
- Issues that require a modified build of Prism PDF itself.
