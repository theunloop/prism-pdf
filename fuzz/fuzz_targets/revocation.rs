#![no_main]
//! Fuzz target for OCSP and CRL parsing (EPIC 9, PAdES-LT — ISO 32000-2 §12.8.4.3).
//!
//! Long-term validation reads its revocation material from the document's own `/DSS` (§12.8.4.3),
//! so an OCSP response (RFC 6960) or a CRL (RFC 5280 §5) reaching this code is a blob a hostile
//! PDF chose. Both are parsed, then matched against a certificate by `CertID` and checked for a
//! valid signature and time window — DER walking plus digest comparisons over attacker-chosen
//! lengths. On ANY input the status must come back as `Unknown` rather than a panic, a hang or an
//! unbounded allocation (DESIGN.md §3.4, §7).
//!
//! Run with: `cargo +nightly fuzz run revocation` (needs `cargo install cargo-fuzz`).

use libfuzzer_sys::fuzz_target;
use pdf_crypto::{RevocationData, cert_revocation, chain_revocation};
use x509_cert::Certificate;
use x509_cert::der::Decode;

/// The throwaway self-signed test certificate (see `crates/pdf/examples/test-signer/README.md`).
/// Being self-signed it is its own issuer, which is the pair `cert_revocation` wants.
const TEST_CERT: &[u8] = include_bytes!("../../crates/pdf/examples/test-signer/cert.der");

fuzz_target!(|input: (u64, &[u8])| {
    let (at_secs, blob) = input;
    let Ok(cert) = Certificate::from_der(TEST_CERT) else {
        return; // the committed fixture is well-formed; nothing to fuzz if it ever is not
    };

    // The same bytes are offered as both an OCSP response and a CRL, so each parser sees them.
    let data = RevocationData {
        ocsps: vec![blob.to_vec()],
        crls: vec![blob.to_vec()],
    };
    let _ = cert_revocation(&cert, &cert, &data, at_secs);
    let _ = chain_revocation(&[(cert.clone(), cert)], &data, at_secs);
});
