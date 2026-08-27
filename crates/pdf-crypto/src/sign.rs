//! Digital signatures (ISO 32000-1 §12.8): the CMS/PKCS#7 cryptographic core for `signDocument`.
//!
//! A PDF signature's `/Contents` is a **detached** CMS `SignedData` (`adbe.pkcs7.detached`,
//! §12.8.3.3) over the bytes the `/ByteRange` covers. [`sign_digest`] builds one; [`verify_detached`]
//! checks one. Reuse over reimplementation (DESIGN.md §6): the CMS/X.509/RSA machinery is RustCrypto
//! (`cms`/`x509-cert`/`rsa`/`der`). Input is untrusted (DESIGN.md §3.4): verification never panics —
//! any malformed structure yields "not valid".
//!
//! Beyond the bare detached signature this module also carries the PAdES-B building blocks:
//! a **signing time** signed attribute (RFC 5652 §11.3), **certificate-chain validation** against a
//! caller-supplied trust store, and an **RFC 3161 timestamp** (`id-aa-timeStampToken`) that can be
//! embedded and verified. A network TSA is out of scope for the core (no I/O here, DESIGN.md §3);
//! [`make_timestamp_token`] mints a token from a local TSA key so the whole path is testable offline,
//! and the same verification accepts a token produced by any RFC 3161 authority.

use std::time::Duration;

use cms::builder::{SignedDataBuilder, SignerInfoBuilder};
use cms::cert::{CertificateChoices, IssuerAndSerialNumber};
use cms::content_info::ContentInfo;
use cms::signed_data::{EncapsulatedContentInfo, SignedData, SignerIdentifier};
use const_oid::db::rfc5912;
use der::asn1::{GeneralizedTime, OctetString, SetOfVec, UtcTime};
use der::oid::ObjectIdentifier;
use der::{Any, Decode, Encode, Sequence, Tag};
use ed25519_dalek::{
    Signature as Ed25519Signature, SigningKey as Ed25519SigningKey,
    VerifyingKey as Ed25519VerifyingKey,
};
use rsa::RsaPrivateKey;
use rsa::pkcs1v15::{Signature, SigningKey, VerifyingKey};
use rsa::pkcs8::DecodePrivateKey;
use rsa::signature::{Keypair, Signer, Verifier};
use sha2::{Digest, Sha256, Sha384, Sha512};
use x509_cert::Certificate;
use x509_cert::attr::Attribute;
use x509_cert::ext::pkix::{BasicConstraints, ExtendedKeyUsage, KeyUsage, SubjectKeyIdentifier};
use x509_cert::spki::{AlgorithmIdentifierOwned, DecodePublicKey};
use x509_cert::spki::{
    DynSignatureAlgorithmIdentifier, EncodePublicKey, SignatureBitStringEncoding,
};
use x509_cert::time::Time;

/// `id-data` (§7.4 RFC 5652): the CMS content type of ordinary document bytes.
const ID_DATA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.7.1");
/// `id-sha256`.
const ID_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.1");
/// `id-sha384`.
const ID_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.2");
/// `id-sha512` — the CMS digest algorithm paired with Ed25519 (RFC 8419).
const ID_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.3");
/// `id-Ed25519` (RFC 8410) — the Ed25519 signature algorithm.
const ID_ED25519: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.101.112");
/// `ecdsa-with-SHA256` (RFC 5758) — ECDSA over P-256.
const ID_ECDSA_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
/// `ecdsa-with-SHA384` (RFC 5758) — ECDSA over P-384.
const ID_ECDSA_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
/// `ecdsa-with-SHA512` (RFC 5758) — ECDSA over P-521 (ISO/TS 32002 Table 3).
const ID_ECDSA_SHA512: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.4");
/// `id-sha3-256` (ISO/TS 32001 §5.1 allows the SHA-3 family in PDF 2.0 signatures).
const ID_SHA3_256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.8");
/// `id-sha3-384`.
const ID_SHA3_384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.9");
/// `id-sha3-512`.
const ID_SHA3_512: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.2.10");
/// `id-rsassa-pkcs1-v1_5-with-sha3-256` (NIST sigAlgs arc).
const ID_RSA_SHA3_256: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.14");
/// `id-rsassa-pkcs1-v1_5-with-sha3-384`.
const ID_RSA_SHA3_384: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.15");
/// `id-rsassa-pkcs1-v1_5-with-sha3-512`.
const ID_RSA_SHA3_512: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.16.840.1.101.3.4.3.16");
/// `id-messageDigest` signed attribute.
const ID_MESSAGE_DIGEST: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.4");
/// `id-signingTime` signed attribute (RFC 5652 §11.3).
const ID_SIGNING_TIME: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.5");
/// `id-aa-timeStampToken` unsigned attribute (RFC 3161 §3.3.1).
const ID_AA_TIMESTAMP_TOKEN: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.14");
/// `id-ct-TSTInfo` — the CMS content type of a timestamp token's encapsulated content (RFC 3161).
const ID_CT_TSTINFO: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.1.4");
/// The TSA policy OID for tokens we mint locally (a placeholder under our own arc — a production
/// deployment fetches tokens from an external TSA, which carries its own policy).
const PRISMPDF_TSA_POLICY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.4.1.55555.1.1");
/// `id-aa-signingCertificateV2` signed attribute (RFC 5035) — the PAdES-B certificate binding.
const ID_AA_SIGNING_CERTIFICATE_V2: ObjectIdentifier =
    ObjectIdentifier::new_unwrap("1.2.840.113549.1.9.16.2.47");
/// `id-kp-OCSPSigning` (RFC 6960 §4.2.2.2) — the extended key usage a delegated OCSP responder
/// certificate must carry.
const ID_KP_OCSP_SIGNING: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.9");
/// `rsaEncryption` (RFC 8017) — permitted as a SignerInfo `signatureAlgorithm` for RSA PKCS#1
/// v1.5, with the digest taken from `digestAlgorithm` (RFC 5652 §5.5).
const ID_RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");

/// RFC 5035 `ESSCertIDv2` with the SHA-256 default hash algorithm omitted: just the certificate
/// hash (`certHash`). `issuerSerial` is optional and not emitted.
#[derive(Sequence)]
struct EssCertIdV2 {
    cert_hash: OctetString,
}

/// RFC 5035 `SigningCertificateV2`: the `SEQUENCE OF ESSCertIDv2` binding the signer certificate
/// into the signed attributes (PAdES-B requires this when the subfilter is `ETSI.CAdES.detached`).
#[derive(Sequence)]
struct SigningCertificateV2 {
    certs: Vec<EssCertIdV2>,
}

/// The maximum certificate-chain depth we will walk when validating a trust path (anti-DoS).
const MAX_CHAIN_DEPTH: usize = 8;

/// Options controlling how a signature is built ([`sign_digest_with`]).
#[derive(Clone, Debug, Default)]
pub struct SignOptions {
    /// The signing time as Unix seconds (UTC). `Some` adds a `signingTime` signed attribute
    /// (RFC 5652 §11.3); `None` omits it (the byte-stable default of [`sign_digest`]).
    pub signing_time: Option<u64>,
    /// When set, a local TSA mints an RFC 3161 timestamp over the signature and embeds it as the
    /// `id-aa-timeStampToken` unsigned attribute. In production this token comes from a network TSA.
    pub timestamp: Option<TsaCredentials>,
    /// When set, build a **PAdES-B** signature (§12.8.3.3): add the `signing-certificate-v2` signed
    /// attribute binding the signer certificate (RFC 5035). The caller pairs this with the
    /// `/SubFilter /ETSI.CAdES.detached` signature-dictionary entry.
    pub pades: bool,
}

/// A local time-stamping authority's material — used to mint an RFC 3161 token over a signature so
/// the timestamp path is exercisable without network I/O (DESIGN.md §3).
#[derive(Clone, Debug)]
pub struct TsaCredentials {
    /// The TSA's X.509 certificate (DER).
    pub cert_der: Vec<u8>,
    /// The TSA's RSA private key (PKCS#8 DER).
    pub key_der: Vec<u8>,
    /// The token's `genTime`, as Unix seconds (UTC).
    pub gen_time: u64,
    /// The token's serial number.
    pub serial: u64,
}

/// Options controlling how a signature is verified ([`verify_detached_with`]).
#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    /// Trusted root certificates (DER X.509). When non-empty, the signer certificate is chained to
    /// one of these (PAdES-B); when empty, the chain is not checked ([`VerifiedSignature::trusted`]
    /// stays `None`).
    pub roots: Vec<Vec<u8>>,
    /// Revocation material (OCSP responses + CRLs, e.g. from a `/DSS`) for **PAdES-LT**
    /// checking: with `Some` and a trusted chain, every non-anchor link is checked and
    /// [`VerifiedSignature::revocation`] is populated. `None` skips revocation entirely.
    pub revocation: Option<crate::RevocationData>,
}

/// The result of verifying a detached signature.
#[derive(Clone, Debug, Default)]
pub struct VerifiedSignature {
    /// Whether the signature covers `message` and the embedded certificate's key verifies it.
    pub valid: bool,
    /// The signer certificate's subject distinguished name, when it could be read.
    pub signer: Option<String>,
    /// The `signingTime` signed attribute as Unix seconds (UTC), when present.
    pub signing_time: Option<i64>,
    /// Whether the signer certificate chains to a trusted root: `Some(true)`/`Some(false)` when a
    /// trust store was supplied, `None` when none was (chain not evaluated).
    pub trusted: Option<bool>,
    /// The `genTime` of a verified embedded RFC 3161 timestamp, as Unix seconds (UTC), when present.
    pub timestamp_time: Option<i64>,
    /// Whether this is a **PAdES-B** signature: it carries a `signing-certificate-v2` signed
    /// attribute (RFC 5035) whose `certHash` matches the embedded signer certificate.
    pub pades: bool,
    /// The chain's revocation outcome (**PAdES-LT**), when [`VerifyOptions::revocation`] supplied
    /// material and the chain was trusted; `None` when revocation was not evaluated.
    pub revocation: Option<crate::RevocationSummary>,
    /// How many `SignerInfo` structures the CMS carries (RFC 5652 §5.1). Every other field
    /// describes the **first** one; a value above 1 means there are further signatures this result
    /// says nothing about, so a caller must not read `valid` as covering all of them.
    pub signer_count: usize,
}

/// RFC 3161 `MessageImprint`: the hash of the timestamped data.
#[derive(Sequence)]
struct MessageImprint {
    hash_algorithm: AlgorithmIdentifierOwned,
    hashed_message: OctetString,
}

/// RFC 3161 `TSTInfo` (mandatory fields only — enough to bind a time to a message imprint).
#[derive(Sequence)]
struct TstInfo {
    version: u8,
    policy: ObjectIdentifier,
    message_imprint: MessageImprint,
    serial_number: u64,
    gen_time: GeneralizedTime,
}

/// Build a detached CMS `SignedData` (DER) over `message` — the `/ByteRange` bytes — signed with the
/// RSA `key_der` (PKCS#8) and certified by `cert_der` (X.509), using SHA-256 (§12.8.3.3). `None` if
/// the key/cert can't be parsed or the CMS can't be assembled.
#[must_use]
pub fn sign_digest(message: &[u8], cert_der: &[u8], key_der: &[u8]) -> Option<Vec<u8>> {
    sign_digest_with(message, cert_der, key_der, &SignOptions::default())
}

/// As [`sign_digest`], but honouring [`SignOptions`]: an optional signing-time attribute and an
/// optional embedded RFC 3161 timestamp (§12.8.3.3, PAdES-B building blocks).
#[must_use]
pub fn sign_digest_with(
    message: &[u8],
    cert_der: &[u8],
    key_der: &[u8],
    opts: &SignOptions,
) -> Option<Vec<u8>> {
    let cert = Certificate::from_der(cert_der).ok()?;

    // First pass: the signature with no timestamp. If a timestamp is requested, this signature is
    // what the TSA stamps; both RSA PKCS#1 v1.5 and Ed25519 are deterministic, so re-signing with
    // the (unsigned) timestamp attribute added produces the identical signature, keeping the stamp's
    // imprint valid.
    let first = build_signed_data(
        message,
        &cert,
        cert_der,
        key_der,
        opts.signing_time,
        None,
        opts.pades,
    )?;

    let Some(tsa) = &opts.timestamp else {
        return Some(first);
    };

    let signature = signer_signature(&first)?;
    let token = make_timestamp_token(
        &signature,
        &tsa.cert_der,
        &tsa.key_der,
        tsa.gen_time,
        tsa.serial,
    )?;
    build_signed_data(
        message,
        &cert,
        cert_der,
        key_der,
        opts.signing_time,
        Some(&token),
        opts.pades,
    )
}

/// Assemble a detached `SignedData` over `message`, dispatching on the private key's algorithm:
/// RSA PKCS#1 v1.5 over SHA-256, or **Ed25519** over SHA-512 (RFC 8419). Optionally carries a
/// signing-time attribute, a `signing-certificate-v2` attribute (PAdES), and a timestamp token.
#[allow(clippy::too_many_arguments)]
fn build_signed_data(
    message: &[u8],
    cert: &Certificate,
    cert_der: &[u8],
    key_der: &[u8],
    signing_time: Option<u64>,
    timestamp_token: Option<&[u8]>,
    pades: bool,
) -> Option<Vec<u8>> {
    // Dispatch on the PKCS#8 key algorithm: Ed25519 (1.3.101.112), ECDSA P-256/P-384/P-521
    // (id-ecPublicKey + curve), else RSA. Each key type only parses with its own loader.
    if let Ok(ed) = Ed25519SigningKey::from_pkcs8_der(key_der) {
        assemble_signed_data::<_, EdBitSig>(
            &EdSigner(ed),
            sha512_alg(),
            Sha512::digest(message).as_slice(),
            cert,
            cert_der,
            signing_time,
            timestamp_token,
            pades,
        )
    } else if let Ok(ec) = p256::ecdsa::SigningKey::from_pkcs8_der(key_der) {
        assemble_signed_data::<_, EcBitSig>(
            &P256Signer(ec),
            sha256_alg(),
            Sha256::digest(message).as_slice(),
            cert,
            cert_der,
            signing_time,
            timestamp_token,
            pades,
        )
    } else if let Ok(ec) = p384::ecdsa::SigningKey::from_pkcs8_der(key_der) {
        assemble_signed_data::<_, EcBitSig>(
            &P384Signer(ec),
            sha384_alg(),
            Sha384::digest(message).as_slice(),
            cert,
            cert_der,
            signing_time,
            timestamp_token,
            pades,
        )
    } else if let Ok(ec) = ecdsa::SigningKey::<p521::NistP521>::from_pkcs8_der(key_der) {
        assemble_signed_data::<_, EcBitSig>(
            &P521Signer(ec),
            sha512_alg(),
            Sha512::digest(message).as_slice(),
            cert,
            cert_der,
            signing_time,
            timestamp_token,
            pades,
        )
    } else {
        let rsa = RsaPrivateKey::from_pkcs8_der(key_der).ok()?;
        let signer = SigningKey::<Sha256>::new(rsa);
        assemble_signed_data::<_, Signature>(
            &signer,
            sha256_alg(),
            Sha256::digest(message).as_slice(),
            cert,
            cert_der,
            signing_time,
            timestamp_token,
            pades,
        )
    }
}

/// An Ed25519 signature that encodes for CMS. The `cms` builder requires the signature type to
/// implement [`SignatureBitStringEncoding`]; `ed25519_dalek::Signature` does not, so this newtype
/// bridges it (the 64 raw bytes become the BIT STRING value).
struct EdBitSig(Ed25519Signature);

impl SignatureBitStringEncoding for EdBitSig {
    fn to_bitstring(&self) -> der::Result<der::asn1::BitString> {
        der::asn1::BitString::from_bytes(&self.0.to_bytes())
    }
}

/// An Ed25519 signing key adapted to the `cms` builder's trait surface (`Keypair` +
/// `DynSignatureAlgorithmIdentifier` + `Signer<EdBitSig>`), emitting the `id-Ed25519` algorithm
/// identifier (RFC 8410).
struct EdSigner(Ed25519SigningKey);

impl Signer<EdBitSig> for EdSigner {
    fn try_sign(&self, msg: &[u8]) -> Result<EdBitSig, rsa::signature::Error> {
        Ok(EdBitSig(self.0.try_sign(msg)?))
    }
}

impl Keypair for EdSigner {
    type VerifyingKey = Ed25519VerifyingKey;
    fn verifying_key(&self) -> Ed25519VerifyingKey {
        self.0.verifying_key()
    }
}

impl DynSignatureAlgorithmIdentifier for EdSigner {
    fn signature_algorithm_identifier(
        &self,
    ) -> Result<AlgorithmIdentifierOwned, x509_cert::spki::Error> {
        Ok(AlgorithmIdentifierOwned {
            oid: ID_ED25519,
            parameters: None,
        })
    }
}

/// An ECDSA signature (the DER `ECDSA-Sig-Value`) that encodes for CMS via
/// [`SignatureBitStringEncoding`] — the same bridge as [`EdBitSig`].
struct EcBitSig(Vec<u8>);

impl SignatureBitStringEncoding for EcBitSig {
    fn to_bitstring(&self) -> der::Result<der::asn1::BitString> {
        der::asn1::BitString::from_bytes(&self.0)
    }
}

/// Generate an ECDSA signer newtype for one NIST curve, adapting its `ecdsa::SigningKey` to the
/// `cms` builder's trait surface (`Keypair` + `DynSignatureAlgorithmIdentifier` + `Signer`). Signing
/// is deterministic (RFC 6979); the signature is emitted as the DER `ECDSA-Sig-Value`.
macro_rules! ecdsa_signer {
    ($name:ident, $sk:ty, $vk:ty, $sig:ty, $alg_oid:expr) => {
        struct $name($sk);

        impl Signer<EcBitSig> for $name {
            fn try_sign(&self, msg: &[u8]) -> Result<EcBitSig, rsa::signature::Error> {
                let sig: $sig = self.0.try_sign(msg)?;
                Ok(EcBitSig(sig.to_der().as_bytes().to_vec()))
            }
        }

        impl Keypair for $name {
            type VerifyingKey = $vk;
            fn verifying_key(&self) -> $vk {
                *self.0.verifying_key()
            }
        }

        impl DynSignatureAlgorithmIdentifier for $name {
            fn signature_algorithm_identifier(
                &self,
            ) -> Result<AlgorithmIdentifierOwned, x509_cert::spki::Error> {
                Ok(AlgorithmIdentifierOwned {
                    oid: $alg_oid,
                    parameters: None,
                })
            }
        }
    };
}

ecdsa_signer!(
    P256Signer,
    p256::ecdsa::SigningKey,
    p256::ecdsa::VerifyingKey,
    p256::ecdsa::Signature,
    ID_ECDSA_SHA256
);
ecdsa_signer!(
    P384Signer,
    p384::ecdsa::SigningKey,
    p384::ecdsa::VerifyingKey,
    p384::ecdsa::Signature,
    ID_ECDSA_SHA384
);
/// The ECDSA/P-521 signer (ISO/TS 32002 Table 3), written out by hand because `p521` 0.13 has no
/// `DigestPrimitive` impl: its native key types are newtype wrappers without the pkcs8/CMS trait
/// surface the [`ecdsa_signer!`] macro relies on. The key is held as the *generic*
/// [`ecdsa::SigningKey`] (which carries pkcs8 decoding and the SPKI-encodable verifying key), and
/// signing goes through the wrapper's SHA-512 path. Unlike P-256/P-384 (RFC 6979 deterministic),
/// `p521` signing is randomized (hedged) — still spec-valid ECDSA.
struct P521Signer(ecdsa::SigningKey<p521::NistP521>);

impl Signer<EcBitSig> for P521Signer {
    fn try_sign(&self, msg: &[u8]) -> Result<EcBitSig, rsa::signature::Error> {
        let sig: p521::ecdsa::Signature =
            p521::ecdsa::SigningKey::from(self.0.clone()).try_sign(msg)?;
        Ok(EcBitSig(sig.to_der().as_bytes().to_vec()))
    }
}

impl Keypair for P521Signer {
    type VerifyingKey = ecdsa::VerifyingKey<p521::NistP521>;
    fn verifying_key(&self) -> Self::VerifyingKey {
        *self.0.verifying_key()
    }
}

impl DynSignatureAlgorithmIdentifier for P521Signer {
    fn signature_algorithm_identifier(
        &self,
    ) -> Result<AlgorithmIdentifierOwned, x509_cert::spki::Error> {
        Ok(AlgorithmIdentifierOwned {
            oid: ID_ECDSA_SHA512,
            parameters: None,
        })
    }
}

/// The signer-generic body of [`build_signed_data`]: build the detached `SignerInfo` (with the
/// external message digest and any signed/unsigned attributes) and wrap it in a `SignedData`.
#[allow(clippy::too_many_arguments)]
fn assemble_signed_data<S, Sig>(
    signer: &S,
    digest_alg: AlgorithmIdentifierOwned,
    digest: &[u8],
    cert: &Certificate,
    cert_der: &[u8],
    signing_time: Option<u64>,
    timestamp_token: Option<&[u8]>,
    pades: bool,
) -> Option<Vec<u8>>
where
    S: Keypair + DynSignatureAlgorithmIdentifier + Signer<Sig>,
    S::VerifyingKey: EncodePublicKey,
    Sig: SignatureBitStringEncoding,
{
    // Detached: the content type is id-data with no eContent; the signed message-digest attribute
    // (from `external_message_digest`) ties the signature to the document bytes.
    let encap = EncapsulatedContentInfo {
        econtent_type: ID_DATA,
        econtent: None,
    };
    let sid = SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
        issuer: cert.tbs_certificate.issuer.clone(),
        serial_number: cert.tbs_certificate.serial_number.clone(),
    });
    let mut signer_info =
        SignerInfoBuilder::new(signer, sid, digest_alg.clone(), &encap, Some(digest)).ok()?;
    if let Some(secs) = signing_time {
        signer_info
            .add_signed_attribute(signing_time_attribute(secs)?)
            .ok()?;
    }
    if pades {
        signer_info
            .add_signed_attribute(signing_cert_v2_attribute(cert_der)?)
            .ok()?;
    }
    if let Some(token) = timestamp_token {
        signer_info
            .add_unsigned_attribute(attribute(
                ID_AA_TIMESTAMP_TOKEN,
                Any::from_der(token).ok()?,
            )?)
            .ok()?;
    }

    let content_info = SignedDataBuilder::new(&encap)
        .add_digest_algorithm(digest_alg)
        .ok()?
        .add_certificate(CertificateChoices::Certificate(cert.clone()))
        .ok()?
        .add_signer_info::<S, Sig>(signer_info)
        .ok()?
        .build()
        .ok()?;

    content_info.to_der().ok()
}

/// Mint an RFC 3161 timestamp token (DER, a CMS `SignedData` over `TSTInfo`) attesting that
/// `target_signature` existed at `gen_time`. The TSA certificate is embedded; `None` on any failure.
/// For self-contained, offline timestamping (§12.8); a production deployment fetches an equivalent
/// token from a network TSA.
#[must_use]
pub fn make_timestamp_token(
    target_signature: &[u8],
    tsa_cert_der: &[u8],
    tsa_key_der: &[u8],
    gen_time: u64,
    serial: u64,
) -> Option<Vec<u8>> {
    let tsa_cert = Certificate::from_der(tsa_cert_der).ok()?;
    let tsa_key = RsaPrivateKey::from_pkcs8_der(tsa_key_der).ok()?;
    let imprint = Sha256::digest(target_signature);

    let tst = TstInfo {
        version: 1,
        policy: PRISMPDF_TSA_POLICY,
        message_imprint: MessageImprint {
            hash_algorithm: sha256_alg(),
            hashed_message: OctetString::new(imprint.to_vec()).ok()?,
        },
        serial_number: serial,
        gen_time: GeneralizedTime::from_unix_duration(Duration::from_secs(gen_time)).ok()?,
    };
    let tst_der = tst.to_der().ok()?;

    // The TSTInfo is the encapsulated (attached) content, wrapped in an OCTET STRING per CMS.
    let encap = EncapsulatedContentInfo {
        econtent_type: ID_CT_TSTINFO,
        econtent: Some(Any::new(Tag::OctetString, tst_der).ok()?),
    };
    let signer = SigningKey::<Sha256>::new(tsa_key);
    let sid = SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
        issuer: tsa_cert.tbs_certificate.issuer.clone(),
        serial_number: tsa_cert.tbs_certificate.serial_number.clone(),
    });
    let signer_info = SignerInfoBuilder::new(&signer, sid, sha256_alg(), &encap, None).ok()?;

    let content_info = SignedDataBuilder::new(&encap)
        .add_digest_algorithm(sha256_alg())
        .ok()?
        .add_certificate(CertificateChoices::Certificate(tsa_cert))
        .ok()?
        .add_signer_info::<SigningKey<Sha256>, Signature>(signer_info)
        .ok()?
        .build()
        .ok()?;
    content_info.to_der().ok()
}

mod verification;
#[cfg(test)]
use verification::now_secs;
use verification::{
    attribute, sha256_alg, sha384_alg, sha512_alg, signer_signature, signing_cert_v2_attribute,
    signing_time_attribute,
};
pub(crate) use verification::{certified_by, is_ocsp_signer, rsa_verifies};
pub use verification::{pdf_date, verify_detached, verify_detached_with, verify_timestamp_token};

#[cfg(test)]
#[path = "sign/tests.rs"]
mod tests;
