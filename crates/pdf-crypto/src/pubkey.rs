//! Public-key security handler (ISO 32000-1 §7.6.5): `/Filter /Adobe.PPKLite`.
//!
//! Instead of a password, the file encryption key is wrapped — once per recipient — inside a
//! CMS/PKCS#7 *enveloped-data* message stored in `/Recipients`. Each message encrypts a 20-byte
//! random **seed** (plus the permission bytes) under a content-encryption key, which is in turn
//! RSA-encrypted to a recipient's certificate. A recipient holding the matching private key recovers
//! the seed; the file key is then `hash(seed ‖ all-recipient-bytes [‖ 0xFFFFFFFF])` — SHA-1 for V4,
//! SHA-256 for V5 — after which object data is decrypted exactly as for the standard handler.
//!
//! Reuse over reimplementation (DESIGN.md §6): the CMS/X.509/RSA machinery is RustCrypto
//! (`cms`/`x509-cert`/`rsa`/`der`). Input is untrusted (DESIGN.md §3.4): every parse step is
//! fallible and returns `None` rather than panicking.

use cms::cert::IssuerAndSerialNumber;
use cms::content_info::{CmsVersion, ContentInfo};
use cms::enveloped_data::{
    EncryptedContentInfo, EnvelopedData, KeyTransRecipientInfo, RecipientIdentifier, RecipientInfo,
    RecipientInfos,
};
use const_oid::db::{
    rfc5911::{ID_AES_128_CBC, ID_AES_256_CBC, ID_DATA, ID_ENVELOPED_DATA},
    rfc5912::RSA_ENCRYPTION,
};
use der::asn1::{Any, Null, OctetString};
use der::{Decode, Encode};
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use x509_cert::Certificate;
use x509_cert::spki::{AlgorithmIdentifierOwned, DecodePublicKey};

use pdf_cos::{Dictionary, Name, Object, PdfString};

use crate::cipher::{aes128_cbc_decrypt, aes128_cbc_encrypt, aes256_cbc_decrypt};
use crate::{Algorithm, Method, StandardSecurityHandler, random_bytes};

impl StandardSecurityHandler {
    /// Open a public-key-encrypted document (§7.6.5): recover the file key using the recipient's
    /// certificate (`cert_der`, DER X.509) and private key (`key_der`, PKCS#8 DER). Returns `None`
    /// if the handler is not `Adobe.PPKLite`, the certificate matches no recipient, or anything is
    /// malformed.
    #[must_use]
    pub fn open_public_key(encrypt: &Dictionary, cert_der: &[u8], key_der: &[u8]) -> Option<Self> {
        if encrypt.get_name(&Name::from("Filter"))?.as_bytes() != b"Adobe.PPKLite" {
            return None;
        }
        let cf = read_crypt_filter(encrypt)?;
        let private_key = rsa::RsaPrivateKey::from_pkcs8_der(key_der).ok()?;
        let cert = Certificate::from_der(cert_der).ok()?;
        let issuer = &cert.tbs_certificate.issuer;
        let serial = &cert.tbs_certificate.serial_number;

        // Find the recipient message addressed to our certificate and recover the seed.
        let seed = cf
            .recipients
            .iter()
            .find_map(|msg| recover_seed(msg, issuer, serial, &private_key))?;

        let key = derive_pubkey_key(
            &seed,
            &cf.recipients,
            cf.encrypt_metadata,
            cf.v5,
            cf.key_len,
        );
        Some(Self {
            key,
            method: cf.method,
        })
    }

    /// Build a public-key write handler (§7.6.5): wrap a fresh file key for each `recipient_certs`
    /// (DER X.509) entry into a CMS message, and return the handler, the `/Encrypt` dictionary, and
    /// a random `/ID` element for the trailer. `algorithm` selects AES-128 (V4) or AES-256 (V5);
    /// returns `None` for RC4 (legacy public-key, not produced) or on a malformed certificate.
    #[must_use]
    pub fn new_encrypter_public_key(
        recipient_certs: &[&[u8]],
        permissions: i32,
        encrypt_metadata: bool,
        algorithm: Algorithm,
    ) -> Option<(Self, Dictionary, Vec<u8>)> {
        let (version, revision, method, cfm, key_len, v5) = match algorithm {
            Algorithm::Aes128 => (4, 4, Method::AesV2, "AESV2", 16usize, false),
            Algorithm::Aes256 => (5, 6, Method::AesV3, "AESV3", 32usize, true),
            Algorithm::Aes256Gcm => (5, 6, Method::AesV4, "AESV4", 32usize, true),
            Algorithm::Rc4 => return None,
        };

        // The CMS envelopes a 20-byte seed followed by the 4 permission bytes (little-endian).
        let seed = random_bytes::<20>()?;
        let mut content = seed.to_vec();
        content.extend_from_slice(&(permissions as u32).to_le_bytes());
        let cms = build_recipients_cms(recipient_certs, &content)?;

        let recipients = vec![cms.clone()];
        let key = derive_pubkey_key(&seed, &recipients, encrypt_metadata, v5, key_len);

        // §7.6.6: a single crypt filter holding the /Recipients, used for both streams and strings.
        let mut std_cf = Dictionary::new();
        std_cf.insert(Name::from("CFM"), Object::Name(Name::from(cfm)));
        std_cf.insert(Name::from("Length"), Object::Integer(key_len as i64));
        std_cf.insert(
            Name::from("Recipients"),
            Object::String(PdfString::from(cms)),
        );
        if !encrypt_metadata {
            std_cf.insert(Name::from("EncryptMetadata"), Object::Boolean(false));
        }
        let mut cf = Dictionary::new();
        cf.insert(Name::from("DefaultCryptFilter"), Object::Dictionary(std_cf));

        let mut dict = Dictionary::new();
        dict.insert(
            Name::from("Filter"),
            Object::Name(Name::from("Adobe.PPKLite")),
        );
        dict.insert(
            Name::from("SubFilter"),
            Object::Name(Name::from("adbe.pkcs7.s5")),
        );
        dict.insert(Name::from("V"), Object::Integer(version));
        dict.insert(Name::from("R"), Object::Integer(revision));
        dict.insert(Name::from("Length"), Object::Integer((key_len * 8) as i64));
        dict.insert(Name::from("CF"), Object::Dictionary(cf));
        dict.insert(
            Name::from("StmF"),
            Object::Name(Name::from("DefaultCryptFilter")),
        );
        dict.insert(
            Name::from("StrF"),
            Object::Name(Name::from("DefaultCryptFilter")),
        );
        dict.insert(Name::from("P"), Object::Integer(permissions as i64));

        let id0 = random_bytes::<16>()?.to_vec();
        Some((Self { key, method }, dict, id0))
    }
}

/// The crypt-filter parameters relevant to public-key key recovery.
struct CryptFilter {
    method: Method,
    encrypt_metadata: bool,
    recipients: Vec<Vec<u8>>,
    v5: bool,
    key_len: usize,
}

/// Read the cipher, recipients and metadata flag from `/Encrypt`. For V≥4 (`adbe.pkcs7.s5`) these
/// live in the `/StmF` crypt filter; for the legacy `s3`/`s4` they are top-level and RC4.
fn read_crypt_filter(encrypt: &Dictionary) -> Option<CryptFilter> {
    let version = encrypt.get_integer(&Name::from("V")).unwrap_or(0);
    if version >= 4 {
        let stmf = encrypt.get_name(&Name::from("StmF"))?;
        let filter = encrypt.get_dict(&Name::from("CF"))?.get_dict(stmf)?;
        let method = match filter.get_name(&Name::from("CFM"))?.as_bytes() {
            b"V2" => Method::Rc4,
            b"AESV2" => Method::AesV2,
            b"AESV3" => Method::AesV3,
            b"AESV4" => Method::AesV4,
            _ => return None,
        };
        let recipients = recipient_strings(filter)?;
        let encrypt_metadata = bool_entry(filter, "EncryptMetadata")
            .or_else(|| bool_entry(encrypt, "EncryptMetadata"))
            .unwrap_or(true);
        let v5 = version == 5;
        let key_len = if v5 {
            32
        } else {
            (encrypt.get_integer(&Name::from("Length")).unwrap_or(128) / 8).clamp(5, 16) as usize
        };
        Some(CryptFilter {
            method,
            encrypt_metadata,
            recipients,
            v5,
            key_len,
        })
    } else {
        let recipients = recipient_strings(encrypt)?;
        let key_len = (encrypt.get_integer(&Name::from("Length")).unwrap_or(40) / 8).clamp(5, 16);
        Some(CryptFilter {
            method: Method::Rc4,
            encrypt_metadata: bool_entry(encrypt, "EncryptMetadata").unwrap_or(true),
            recipients,
            v5: false,
            key_len: key_len as usize,
        })
    }
}

/// Read `/Recipients` (a string or an array of strings) into raw CMS byte vectors.
fn recipient_strings(dict: &Dictionary) -> Option<Vec<Vec<u8>>> {
    match dict.get(&Name::from("Recipients"))? {
        Object::String(s) => Some(vec![s.as_bytes().to_vec()]),
        Object::Array(arr) => arr
            .iter()
            .map(|o| match o {
                Object::String(s) => Some(s.as_bytes().to_vec()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// Recover the 20-byte seed from one CMS message if it is addressed to our (issuer, serial).
fn recover_seed(
    cms_bytes: &[u8],
    issuer: &x509_cert::name::Name,
    serial: &x509_cert::serial_number::SerialNumber,
    private_key: &rsa::RsaPrivateKey,
) -> Option<Vec<u8>> {
    let content_info = ContentInfo::from_der(cms_bytes).ok()?;
    if content_info.content_type != ID_ENVELOPED_DATA {
        return None;
    }
    let enveloped = EnvelopedData::from_der(&content_info.content.to_der().ok()?).ok()?;

    // RSA-decrypt the content-encryption key from the matching KeyTrans recipient.
    let mut cek = None;
    for info in enveloped.recip_infos.0.iter() {
        let RecipientInfo::Ktri(ktri) = info else {
            continue;
        };
        let RecipientIdentifier::IssuerAndSerialNumber(isn) = &ktri.rid else {
            continue;
        };
        if &isn.issuer == issuer && &isn.serial_number == serial {
            cek = private_key
                .decrypt(Pkcs1v15Encrypt, ktri.enc_key.as_bytes())
                .ok();
            if cek.is_some() {
                break;
            }
        }
    }
    let cek = cek?;

    // Decrypt the enveloped content (CBC, IV in the algorithm parameters) to get seed ‖ perms.
    let eci = &enveloped.encrypted_content;
    let ciphertext = eci.encrypted_content.as_ref()?.as_bytes();
    let iv = cbc_iv(&eci.content_enc_alg)?;
    let mut iv_and_ct = iv;
    iv_and_ct.extend_from_slice(ciphertext);
    let plain = match eci.content_enc_alg.oid {
        ID_AES_128_CBC => aes128_cbc_decrypt(&cek, &iv_and_ct)?,
        ID_AES_256_CBC => aes256_cbc_decrypt(&cek, &iv_and_ct)?,
        _ => return None,
    };
    (plain.len() >= 20).then(|| plain[..20].to_vec())
}

/// The 16-byte CBC IV carried in an AES content-encryption algorithm identifier (an `OCTET STRING`).
fn cbc_iv(alg: &AlgorithmIdentifierOwned) -> Option<Vec<u8>> {
    let params = alg.parameters.as_ref()?;
    let iv = OctetString::from_der(&params.to_der().ok()?).ok()?;
    Some(iv.as_bytes().to_vec())
}

/// Derive the file encryption key from the seed and the recipient messages (§7.6.5.3): SHA-256 for
/// V5/AES-256, SHA-1 (truncated to the key length) otherwise. If metadata is not encrypted, four
/// `0xFF` bytes are appended.
fn derive_pubkey_key(
    seed: &[u8],
    recipients: &[Vec<u8>],
    encrypt_metadata: bool,
    v5: bool,
    key_len: usize,
) -> Vec<u8> {
    if v5 {
        let mut h = Sha256::new();
        h.update(seed);
        for r in recipients {
            h.update(r);
        }
        if !encrypt_metadata {
            h.update([0xFFu8; 4]);
        }
        h.finalize()[..32].to_vec()
    } else {
        let mut h = Sha1::new();
        h.update(seed);
        for r in recipients {
            h.update(r);
        }
        if !encrypt_metadata {
            h.update([0xFFu8; 4]);
        }
        let digest = h.finalize();
        digest[..key_len.min(digest.len())].to_vec()
    }
}

/// Build a CMS `ContentInfo`/`EnvelopedData` (DER) enveloping `content` for every recipient
/// certificate: AES-128-CBC content encryption, the content-encryption key RSA-wrapped per recipient
/// (identified by issuer-and-serial). Returns the DER bytes for `/Recipients`.
fn build_recipients_cms(certs: &[&[u8]], content: &[u8]) -> Option<Vec<u8>> {
    let cek = random_bytes::<16>()?;
    let iv = random_bytes::<16>()?;
    // aes128_cbc_encrypt returns iv ‖ ciphertext; CMS stores them separately.
    let iv_and_ct = aes128_cbc_encrypt(&cek, &iv, content);
    let ciphertext = iv_and_ct.get(16..)?.to_vec();

    let content_enc_alg = AlgorithmIdentifierOwned {
        oid: ID_AES_128_CBC,
        parameters: Some(Any::encode_from(&OctetString::new(iv.to_vec()).ok()?).ok()?),
    };
    let encrypted_content = EncryptedContentInfo {
        content_type: ID_DATA,
        content_enc_alg,
        encrypted_content: Some(OctetString::new(ciphertext).ok()?),
    };

    let mut rng = rand_core::OsRng;
    let mut infos = Vec::with_capacity(certs.len());
    for cert_der in certs {
        let cert = Certificate::from_der(cert_der).ok()?;
        let spki_der = cert.tbs_certificate.subject_public_key_info.to_der().ok()?;
        let public_key = RsaPublicKey::from_public_key_der(&spki_der).ok()?;
        let enc_key = public_key.encrypt(&mut rng, Pkcs1v15Encrypt, &cek).ok()?;
        infos.push(RecipientInfo::Ktri(KeyTransRecipientInfo {
            version: CmsVersion::V0,
            rid: RecipientIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
                issuer: cert.tbs_certificate.issuer.clone(),
                serial_number: cert.tbs_certificate.serial_number.clone(),
            }),
            key_enc_alg: AlgorithmIdentifierOwned {
                oid: RSA_ENCRYPTION,
                parameters: Some(Any::encode_from(&Null).ok()?),
            },
            enc_key: OctetString::new(enc_key).ok()?,
        }));
    }

    let enveloped = EnvelopedData {
        version: CmsVersion::V0,
        originator_info: None,
        recip_infos: RecipientInfos::try_from(infos).ok()?,
        encrypted_content,
        unprotected_attrs: None,
    };
    let content_info = ContentInfo {
        content_type: ID_ENVELOPED_DATA,
        content: Any::encode_from(&enveloped).ok()?,
    };
    content_info.to_der().ok()
}

/// Read a boolean dictionary entry.
fn bool_entry(dict: &Dictionary, key: &str) -> Option<bool> {
    dict.get(&Name::from(key))?.as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::EncodePrivateKey;
    use sha2::Sha256;
    use std::str::FromStr;
    use std::time::Duration;
    use x509_cert::builder::{Builder, CertificateBuilder, Profile};
    use x509_cert::name::Name as X509Name;
    use x509_cert::serial_number::SerialNumber;
    use x509_cert::spki::{EncodePublicKey, SubjectPublicKeyInfoOwned};
    use x509_cert::time::Validity;

    /// Generate an RSA-2048 keypair + self-signed cert; return (cert DER, PKCS#8 key DER).
    fn make_recipient(serial: u32) -> (Vec<u8>, Vec<u8>) {
        let mut rng = rand_core::OsRng;
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let public_key = RsaPublicKey::from(&private_key);
        let spki =
            SubjectPublicKeyInfoOwned::try_from(public_key.to_public_key_der().unwrap().as_bytes())
                .unwrap();
        let subject = X509Name::from_str("CN=Prism PDF Test Recipient").unwrap();
        let signer = SigningKey::<Sha256>::new(private_key.clone());
        let cert = CertificateBuilder::new(
            Profile::Root,
            SerialNumber::from(serial),
            Validity::from_now(Duration::from_secs(3600)).unwrap(),
            subject,
            spki,
            &signer,
        )
        .unwrap()
        .build()
        .unwrap();
        (
            cert.to_der().unwrap(),
            private_key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
        )
    }

    fn round_trip(algorithm: Algorithm) {
        let (cert, key) = make_recipient(7);
        let (writer, dict, _id0) =
            StandardSecurityHandler::new_encrypter_public_key(&[&cert], -1, true, algorithm)
                .unwrap();

        let plaintext = b"BT (public-key secret) Tj ET";
        let ciphertext = writer.encrypt(4, 0, plaintext).expect("rng available");
        assert_ne!(ciphertext, plaintext);

        let reader = StandardSecurityHandler::open_public_key(&dict, &cert, &key).unwrap();
        assert_eq!(reader.key, writer.key);
        assert_eq!(reader.decrypt(4, 0, &ciphertext).unwrap(), plaintext);
    }

    #[test]
    fn public_key_aes128_round_trips() {
        round_trip(Algorithm::Aes128);
    }

    #[test]
    fn public_key_aes256_round_trips() {
        round_trip(Algorithm::Aes256);
    }

    #[test]
    fn wrong_recipient_cannot_open() {
        let (cert, _key) = make_recipient(1);
        let (_writer, dict, _id0) = StandardSecurityHandler::new_encrypter_public_key(
            &[&cert],
            -1,
            true,
            Algorithm::Aes256,
        )
        .unwrap();
        // A different keypair/cert is not a listed recipient.
        let (other_cert, other_key) = make_recipient(2);
        assert!(StandardSecurityHandler::open_public_key(&dict, &other_cert, &other_key).is_none());
    }

    #[test]
    fn multiple_recipients_each_open() {
        let (cert_a, key_a) = make_recipient(10);
        let (cert_b, key_b) = make_recipient(11);
        let (writer, dict, _id0) = StandardSecurityHandler::new_encrypter_public_key(
            &[&cert_a, &cert_b],
            -1,
            true,
            Algorithm::Aes256,
        )
        .unwrap();
        for (cert, key) in [(&cert_a, &key_a), (&cert_b, &key_b)] {
            let reader = StandardSecurityHandler::open_public_key(&dict, cert, key).unwrap();
            assert_eq!(reader.key, writer.key);
        }
    }

    #[test]
    fn rc4_public_key_write_is_unsupported() {
        let (cert, _key) = make_recipient(1);
        assert!(
            StandardSecurityHandler::new_encrypter_public_key(&[&cert], -1, true, Algorithm::Rc4)
                .is_none()
        );
    }
}
