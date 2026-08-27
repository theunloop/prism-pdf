#![no_main]
//! Fuzz target for signature verification over untrusted DER (EPIC 9, ISO 32000 §12.8).
//!
//! Verifying a signature means parsing a CMS `SignedData` (RFC 5652), the certificates inside it
//! (RFC 5280), its signed attributes, and any embedded RFC 3161 timestamp token — all of it taken
//! from the `/Contents` string of a PDF anyone can send us, and all of it parsed *before* anything
//! has been authenticated. That makes it the largest untrusted-DER surface in the engine.
//! Verification is total by construction (`verify_detached` returns a `VerifiedSignature`, never
//! an error), so what this target guards is the promise underneath: on ANY input it must return
//! without panicking, hanging, or allocating unboundedly (DESIGN.md §3.4, §7).
//!
//! The fuzzed bytes are also fed in as a trust anchor, so the chain-building path (PAdES-B) runs
//! on hostile certificates too, alongside the real test certificate.
//!
//! Run with: `cargo +nightly fuzz run cms` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_crypto::{VerifyOptions, verify_detached, verify_detached_with, verify_timestamp_token};

/// The throwaway self-signed test certificate (see `crates/pdf/examples/test-signer/README.md`),
/// so chain building has one well-formed anchor to walk as well as the fuzzer's.
const TEST_CERT: &[u8] = include_bytes!("../../crates/pdf/examples/test-signer/cert.der");

fuzz_target!(|input: (&[u8], &[u8])| {
    let (der, message) = input;

    // No trust store: parse + digest + signature check only.
    let _ = verify_detached(der, message);

    // With a trust store, so certificate-chain building runs as well.
    let opts = VerifyOptions {
        roots: vec![TEST_CERT.to_vec(), der.to_vec()],
        revocation: None,
    };
    let _ = verify_detached_with(der, message, &opts);

    // The RFC 3161 path (§12.8.3.3 document timestamps) parses a different token shape.
    let _ = verify_timestamp_token(der, message, &opts);
});
