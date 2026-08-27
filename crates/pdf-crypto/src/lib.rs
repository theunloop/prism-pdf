#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! pdf-crypto — encryption (EPIC 9, ISO 32000-1 §7.6).
//!
//! The standard security handler (§7.6.4), both directions. **Read:** derive the file encryption
//! key from the `/Encrypt` dictionary and a password, then decrypt object strings and streams —
//! the MD5-based RC4 (V1/V2, §7.6.4.3.2) and AES-128/`AESV2` (V4) with per-object keys (§7.6.4.3.1),
//! and the SHA-2-based AES-256 (`AESV3`, V5/R6, §7.6.4.3.3–.4) where the file key is used directly.
//! Each password is tried as both the user password and, via `/O`, the owner password (Algorithms
//! 6/7 for RC4, the owner 2.A variant for V5). **Write:** [`StandardSecurityHandler::new_encrypter`]
//! builds an `/Encrypt` dictionary and encrypts objects for RC4 (V2/R3), AES-128 (V4/R4) via
//! Algorithms 2/3/5, or AES-256 (V5/R6, `AESV3`) via Algorithms 8–10 (`/U`,`/UE`,`/O`,`/OE`,`/Perms`).
//!
//! Depends only on [`pdf_cos`] plus RustCrypto ciphers (`md-5`, `sha2`, `aes`, `cbc`) and
//! `getrandom` for IVs/IDs; reuse over reimplementation (DESIGN.md §6).

mod cipher;
mod mac;
mod pubkey;
mod revocation;
mod sign;

pub use mac::{
    attach_pdf_mac_to_signature, compose_pdf_mac_token, pdf_mac_wrap_kdf, verify_attached_pdf_mac,
    verify_pdf_mac_token,
};
pub use revocation::{
    RevocationData, RevocationStatus, RevocationSummary, cert_revocation, chain_revocation,
};
pub use sign::{
    SignOptions, TsaCredentials, VerifiedSignature, VerifyOptions, make_timestamp_token, pdf_date,
    sign_digest, sign_digest_with, verify_detached, verify_detached_with, verify_timestamp_token,
};

use md5::{Digest, Md5};
use pdf_cos::{Dictionary, Name, Object, PdfString};
use sha2::{Sha256, Sha384, Sha512};
use subtle::ConstantTimeEq;

use cipher::{
    GCM_NONCE_LEN, aes128_cbc_decrypt, aes128_cbc_encrypt, aes128_cbc_encrypt_nopad,
    aes256_cbc_decrypt, aes256_cbc_decrypt_nopad, aes256_cbc_encrypt, aes256_cbc_encrypt_nopad,
    aes256_gcm_decrypt, aes256_gcm_encrypt, rc4,
};

/// The 32-byte password-padding string (§7.6.4.3.2, step (a)).
const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// The cipher a handler applies to object data.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Method {
    /// RC4 (V1/V2, or V4 with `/CFM /V2`).
    Rc4,
    /// AES-128-CBC (V4 with `/CFM /AESV2`).
    AesV2,
    /// AES-256-CBC (V5/R6 with `/CFM /AESV3`): the file key is used directly, with no per-object
    /// derivation (§7.6.4.3.4).
    AesV3,
    /// AES-256-GCM (V5/R6 with `/CFM /AESV4`, ISO/TS 32003): authenticated encryption on the file
    /// key directly; the payload is `nonce ‖ ciphertext ‖ tag`.
    AesV4,
}

/// The encryption algorithm to apply when writing an encrypted document (§7.6.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Algorithm {
    /// RC4, 128-bit (V2/R3) — widely compatible but cryptographically weak.
    Rc4,
    /// AES-128-CBC (V4/R4, `AESV2`).
    Aes128,
    /// AES-256-CBC (V5/R6, `AESV3`) — the strongest standard handler (§7.6.4.3.3–.4).
    Aes256,
    /// AES-256-**GCM** (V5/R6, `AESV4`, ISO/TS 32003) — authenticated encryption; same key
    /// derivation as `Aes256`, with a GCM crypt filter (PDF 2.0).
    Aes256Gcm,
}

/// User access permissions for an encrypted document — the `/P` flag word (§7.6.3.2, Table 22).
///
/// `/P` is a 32-bit field where the listed bits *grant* an operation and all unlisted/reserved bits
/// are fixed (bits 1–2 = 0, bits 7–8 and 13–32 = 1). Build one from [`Permissions::RESTRICTED`]
/// (nothing allowed) with the `allow_*` methods, or use [`Permissions::ALL`] to grant everything.
/// We honour permissions on *write* (they are stored and, for AES-256/public-key, sealed); like
/// most engines we do not enforce them on read — that is the consuming viewer's responsibility.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Permissions(i32);

impl Permissions {
    /// The mandatory-set bits (7–8, 13–32) with every grantable bit cleared: nothing is allowed.
    pub const RESTRICTED: Permissions = Permissions(0xFFFF_F0C0u32 as i32);
    /// Everything allowed (all bits set, the common default).
    pub const ALL: Permissions = Permissions(-1);

    fn with(self, bit: i32) -> Self {
        Permissions(self.0 | bit)
    }
    /// Print the document (bit 3); combine with [`Self::allow_print_high_res`] for full quality.
    #[must_use]
    pub fn allow_print(self) -> Self {
        self.with(1 << 2)
    }
    /// Modify the document's contents (bit 4).
    #[must_use]
    pub fn allow_modify(self) -> Self {
        self.with(1 << 3)
    }
    /// Copy or extract text and graphics (bit 5).
    #[must_use]
    pub fn allow_copy(self) -> Self {
        self.with(1 << 4)
    }
    /// Add or modify annotations and fill in interactive form fields (bit 6).
    #[must_use]
    pub fn allow_annotate(self) -> Self {
        self.with(1 << 5)
    }
    /// Fill in existing form fields, even without [`Self::allow_annotate`] (bit 9).
    #[must_use]
    pub fn allow_fill_forms(self) -> Self {
        self.with(1 << 8)
    }
    /// Extract text and graphics for accessibility (bit 10).
    #[must_use]
    pub fn allow_accessibility(self) -> Self {
        self.with(1 << 9)
    }
    /// Assemble the document — insert, rotate, delete pages (bit 11).
    #[must_use]
    pub fn allow_assemble(self) -> Self {
        self.with(1 << 10)
    }
    /// Print at full (high) resolution (bit 12).
    #[must_use]
    pub fn allow_print_high_res(self) -> Self {
        self.with(1 << 11)
    }
    /// The raw `/P` integer.
    #[must_use]
    pub fn bits(self) -> i32 {
        self.0
    }

    /// Rebuild a permission set from a raw `/P` integer — the inverse of [`Self::bits`].
    ///
    /// Needed wherever the value has to survive a round trip through a plain integer: reading `/P`
    /// back off an existing document, or carrying a permission set across the C ABI, which has no
    /// representation for this type. The reserved bits are *not* normalised: a value that did not
    /// come from `bits()` is stored as given, matching the permissiveness the spec asks of readers
    /// (§7.6.3.2).
    #[must_use]
    pub fn from_bits(bits: i32) -> Self {
        Permissions(bits)
    }
}

impl Default for Permissions {
    fn default() -> Self {
        Permissions::ALL
    }
}

/// The standard security handler (§7.6.4): holds the file encryption key and cipher, and decrypts
/// individual objects by deriving their per-object key (§7.6.4.3.1).
#[derive(Clone, Debug)]
pub struct StandardSecurityHandler {
    key: Vec<u8>,
    method: Method,
}

impl StandardSecurityHandler {
    /// Build a handler from the `/Encrypt` dictionary, the first element of the file `/ID`, and a
    /// password (empty for the common case). The password is tried first as the **user** password
    /// (Algorithm 6 / 2.A) and then, on failure, as the **owner** password (Algorithm 7 / owner
    /// 2.A). Returns `None` for handlers not yet supported (e.g. a public-key handler) or when the
    /// password validates as neither.
    #[must_use]
    pub fn open(encrypt: &Dictionary, id0: &[u8], password: &[u8]) -> Option<Self> {
        if !supports(encrypt) {
            return None;
        }
        let version = encrypt.get_integer(&Name::from("V")).unwrap_or(0);
        let revision = encrypt.get_integer(&Name::from("R"))?;

        // V5/R6 (AES-256) derives the file key from /U and /UE via SHA-2 (§7.6.4.3.3–.4), a
        // different scheme from the MD5-based versions below.
        if version == 5 {
            return open_v5(encrypt, revision, password);
        }

        let owner = string_bytes(encrypt, "O")?;
        let user = string_bytes(encrypt, "U")?;
        let permissions = encrypt.get_integer(&Name::from("P"))?;
        let length_bits = encrypt.get_integer(&Name::from("Length")).unwrap_or(40);
        let encrypt_metadata = bool_entry(encrypt, "EncryptMetadata").unwrap_or(true);

        let (method, key_len) = match version {
            1 => (Method::Rc4, 5),
            2 => (Method::Rc4, key_len_bytes(length_bits)),
            4 => (v4_method(encrypt)?, key_len_bytes(length_bits)),
            _ => return None,
        };

        let verify = |candidate_user: &[u8]| -> Option<Vec<u8>> {
            let key = derive_key(
                candidate_user,
                &owner,
                permissions,
                id0,
                revision,
                key_len,
                encrypt_metadata,
            );
            user_password_matches(&key, id0, revision, &user).then_some(key)
        };

        // Try the password as the user password (Algorithm 6); otherwise recover the user password
        // from /O with it as the owner password (Algorithm 7) and try that.
        let key = verify(password).or_else(|| {
            verify(&recover_user_from_owner(
                password, &owner, revision, key_len,
            ))
        })?;
        Some(Self { key, method })
    }

    /// Decrypt the data of object `number`/`generation` — a string's bytes or a stream's raw bytes
    /// (§7.6.2 / §7.6.4.3.1).
    ///
    /// `None` means an **authenticated** decryption failed, and the severity of that is why the
    /// return type is not a plain `Vec`.
    ///
    /// Only `AESV4` (AES-256-GCM, ISO/TS 32003) authenticates. There, a failed tag is proof the
    /// document was altered after it was encrypted, and reporting it is the entire reason the
    /// filter exists — so it becomes `None` and the reader turns it into a hard error rather than
    /// substituting empty content the caller cannot distinguish from an empty stream.
    ///
    /// The unauthenticated modes deliberately stay lenient. RC4 is a keystream cipher with nothing
    /// to check. AES-CBC can only fail on PKCS#7 padding, which carries no integrity guarantee: a
    /// wrong password is already caught by `/U` (Algorithm 6), so a padding failure here means a
    /// damaged or non-conforming file, not a detected attack. Refusing the whole document over one
    /// such object would break the reader's first-class recovery contract (DESIGN.md §3) on files
    /// that other engines read, so those degrade to empty bytes for that object, as before.
    #[must_use]
    pub fn decrypt(&self, number: u32, generation: u16, data: &[u8]) -> Option<Vec<u8>> {
        match self.method {
            Method::Rc4 => Some(rc4(&self.object_key(number, generation), data)),
            Method::AesV2 => Some(
                aes128_cbc_decrypt(&self.object_key(number, generation), data).unwrap_or_default(),
            ),
            // V5/AESV3 and V5/AESV4 use the file key directly — no per-object key (§7.6.4.3.4).
            Method::AesV3 => Some(aes256_cbc_decrypt(&self.key, data).unwrap_or_default()),
            // The one authenticated method: a tag mismatch propagates.
            Method::AesV4 => aes256_gcm_decrypt(&self.key, data),
        }
    }

    /// Whether this handler's crypt filter **authenticates** its ciphertext — true only for
    /// `AESV4` (AES-256-GCM, ISO/TS 32003). Only then does a decryption failure prove tampering
    /// rather than merely indicating a damaged file.
    #[must_use]
    pub fn is_authenticated(&self) -> bool {
        self.method == Method::AesV4
    }

    /// Build a handler for **writing** an encrypted document with the standard security handler
    /// (§7.6.4). Returns the handler, the `/Encrypt` dictionary to store in the document, and a
    /// freshly generated 16-byte file `/ID` element (which both key derivation and the trailer
    /// use). `permissions` is the `/P` flag word and `encrypt_metadata` selects whether the document
    /// metadata stream is encrypted (§7.6.4.3; ignored for RC4/V2, which always encrypts it).
    ///
    /// For RC4/AES-128 the file key, `/O` and `/U` are computed with the RC4/MD5 algorithms
    /// (Algorithms 2/3/5). [`Algorithm::Aes256`] instead generates a random 256-bit file key and the
    /// SHA-2 entries `/U`, `/UE`, `/O`, `/OE`, `/Perms` (V5/R6, Algorithms 8–10).
    ///
    /// `None` if the OS random number generator is unavailable : every
    /// algorithm here needs fresh randomness for a key, a salt or a file `/ID`, and producing a
    /// document under predictable material would be worse than producing none.
    #[must_use]
    pub fn new_encrypter(
        user_password: &[u8],
        owner_password: &[u8],
        permissions: i32,
        encrypt_metadata: bool,
        algorithm: Algorithm,
    ) -> Option<(Self, Dictionary, Vec<u8>)> {
        // V5/R6 ciphers (AES-256) share the key-wrapping path; only the crypt-filter method differs.
        match algorithm {
            Algorithm::Aes256 => {
                return Self::new_encrypter_v5(
                    user_password,
                    owner_password,
                    permissions,
                    encrypt_metadata,
                    Method::AesV3,
                );
            }
            Algorithm::Aes256Gcm => {
                return Self::new_encrypter_v5(
                    user_password,
                    owner_password,
                    permissions,
                    encrypt_metadata,
                    Method::AesV4,
                );
            }
            _ => {}
        }
        let (version, revision, method) = match algorithm {
            Algorithm::Rc4 => (2, 3, Method::Rc4),
            Algorithm::Aes128 => (4, 4, Method::AesV2),
            Algorithm::Aes256 | Algorithm::Aes256Gcm => unreachable!("handled above"),
        };
        let key_len = 16;
        // /EncryptMetadata only affects V≥4; RC4/V2 has no such switch.
        let encrypt_metadata = encrypt_metadata || version < 4;
        let permissions = permissions as i64;
        let id0 = random_bytes::<16>()?.to_vec();

        // /O (Algorithm 3): the owner password protects the user password; when no owner password
        // is given, the user password plays both roles.
        let owner_pw = if owner_password.is_empty() {
            user_password
        } else {
            owner_password
        };
        let o = compute_owner(owner_pw, user_password, revision, key_len);

        // File key (Algorithm 2) then /U (Algorithm 5).
        let key = derive_key(
            user_password,
            &o,
            permissions,
            &id0,
            revision,
            key_len,
            encrypt_metadata,
        );
        let u = compute_user(&key, &id0, revision);

        let mut dict = Dictionary::new();
        dict.insert(Name::from("Filter"), Object::Name(Name::from("Standard")));
        dict.insert(Name::from("V"), Object::Integer(version));
        dict.insert(Name::from("R"), Object::Integer(revision));
        dict.insert(Name::from("Length"), Object::Integer((key_len * 8) as i64));
        dict.insert(Name::from("O"), Object::String(PdfString::from(o)));
        dict.insert(Name::from("U"), Object::String(PdfString::from(u)));
        dict.insert(Name::from("P"), Object::Integer(permissions));
        if version == 4 {
            // §7.6.5: a single crypt filter `StdCF` applied to both streams and strings.
            let mut std_cf = Dictionary::new();
            std_cf.insert(Name::from("CFM"), Object::Name(Name::from("AESV2")));
            std_cf.insert(Name::from("Length"), Object::Integer(key_len as i64));
            let mut cf = Dictionary::new();
            cf.insert(Name::from("StdCF"), Object::Dictionary(std_cf));
            dict.insert(Name::from("CF"), Object::Dictionary(cf));
            dict.insert(Name::from("StmF"), Object::Name(Name::from("StdCF")));
            dict.insert(Name::from("StrF"), Object::Name(Name::from("StdCF")));
        }
        if !encrypt_metadata {
            dict.insert(Name::from("EncryptMetadata"), Object::Boolean(false));
        }
        Some((Self { key, method }, dict, id0))
    }

    /// Build a V5/R6 (`AESV3`, AES-256) write handler (§7.6.4.3.3–.4). The 256-bit file key is
    /// random; `/U`+`/UE` (Algorithm 8) wrap it under the user password, `/O`+`/OE` (Algorithm 9)
    /// under the owner password (with the full `/U` as extra data), and `/Perms` (Algorithm 10)
    /// seals the permissions under the file key.
    fn new_encrypter_v5(
        user_password: &[u8],
        owner_password: &[u8],
        permissions: i32,
        encrypt_metadata: bool,
        method: Method,
    ) -> Option<(Self, Dictionary, Vec<u8>)> {
        const R6: i64 = 6;
        let permissions = permissions as i64;
        let file_key = random_bytes::<32>()?.to_vec();
        let owner_pw = if owner_password.is_empty() {
            user_password
        } else {
            owner_password
        };

        // Algorithm 8: /U = hash(pw + validation salt) ‖ validation salt ‖ key salt; /UE wraps the
        // file key under hash(pw + key salt). Salts are [0..8] validation, [8..16] key.
        let u_salts = random_bytes::<16>()?;
        let mut u = hash_2b(R6, user_password, &u_salts[..8], &[]);
        u.extend_from_slice(&u_salts);
        let u_key = hash_2b(R6, user_password, &u_salts[8..16], &[]);
        let ue = aes256_cbc_encrypt_nopad(&u_key, &[0u8; 16], &file_key);

        // Algorithm 9: as Algorithm 8, but the full 48-byte /U is appended as extra data.
        let o_salts = random_bytes::<16>()?;
        let mut o = hash_2b(R6, owner_pw, &o_salts[..8], &u[..48]);
        o.extend_from_slice(&o_salts);
        let o_key = hash_2b(R6, owner_pw, &o_salts[8..16], &u[..48]);
        let oe = aes256_cbc_encrypt_nopad(&o_key, &[0u8; 16], &file_key);

        // Algorithm 10: /Perms (byte 8 records the EncryptMetadata flag).
        let perms = compute_perms(permissions, encrypt_metadata, &file_key)?;

        let id0 = random_bytes::<16>()?.to_vec();

        // The crypt-filter method: AESV3 (CBC) or AESV4 (GCM, ISO/TS 32003) — both on the V5/R6 key.
        let cfm = if method == Method::AesV4 {
            "AESV4"
        } else {
            "AESV3"
        };
        let mut std_cf = Dictionary::new();
        std_cf.insert(Name::from("CFM"), Object::Name(Name::from(cfm)));
        std_cf.insert(Name::from("Length"), Object::Integer(32));
        let mut cf = Dictionary::new();
        cf.insert(Name::from("StdCF"), Object::Dictionary(std_cf));

        let mut dict = Dictionary::new();
        dict.insert(Name::from("Filter"), Object::Name(Name::from("Standard")));
        dict.insert(Name::from("V"), Object::Integer(5));
        dict.insert(Name::from("R"), Object::Integer(R6));
        dict.insert(Name::from("Length"), Object::Integer(256));
        dict.insert(Name::from("CF"), Object::Dictionary(cf));
        dict.insert(Name::from("StmF"), Object::Name(Name::from("StdCF")));
        dict.insert(Name::from("StrF"), Object::Name(Name::from("StdCF")));
        dict.insert(Name::from("O"), Object::String(PdfString::from(o)));
        dict.insert(Name::from("U"), Object::String(PdfString::from(u)));
        dict.insert(Name::from("OE"), Object::String(PdfString::from(oe)));
        dict.insert(Name::from("UE"), Object::String(PdfString::from(ue)));
        dict.insert(Name::from("Perms"), Object::String(PdfString::from(perms)));
        dict.insert(Name::from("P"), Object::Integer(permissions));
        if !encrypt_metadata {
            dict.insert(Name::from("EncryptMetadata"), Object::Boolean(false));
        }

        Some((
            Self {
                key: file_key,
                method,
            },
            dict,
            id0,
        ))
    }

    /// The document's file encryption key (§7.6.4.3.4). For a V5/R6 handler this is the 256-bit
    /// key used directly to encrypt objects; it is also the input keying material for the PDF MAC
    /// key derivation (ISO/TS 32004 §6.4). Only meaningful when [`Self::is_v5`] is true.
    #[must_use]
    pub fn file_key(&self) -> &[u8] {
        &self.key
    }

    /// Whether this is a V5/R6 (AES-256, `AESV3`/`AESV4`) handler — the only kind whose `key` is a
    /// file encryption key per ISO/TS 32004 §3.3 (security handler version 5 or higher).
    #[must_use]
    pub fn is_v5(&self) -> bool {
        matches!(self.method, Method::AesV3 | Method::AesV4)
    }

    /// Encrypt object `number`/`generation`'s data — the inverse of [`decrypt`](Self::decrypt)
    /// (§7.6.2). RC4 is symmetric; AES prepends a fresh random IV. AES-256 (V5) uses the file key
    /// directly, with no per-object derivation (§7.6.4.3.4).
    ///
    /// `None` if the OS random number generator is unavailable while drawing an AES IV or GCM
    /// nonce. RC4 needs no per-object randomness and cannot fail here.
    #[must_use]
    pub fn encrypt(&self, number: u32, generation: u16, data: &[u8]) -> Option<Vec<u8>> {
        Some(match self.method {
            Method::Rc4 => rc4(&self.object_key(number, generation), data),
            Method::AesV2 => aes128_cbc_encrypt(
                &self.object_key(number, generation),
                &random_bytes::<16>()?,
                data,
            ),
            Method::AesV3 => aes256_cbc_encrypt(&self.key, &random_bytes::<16>()?, data),
            Method::AesV4 => aes256_gcm_encrypt(&self.key, &random_bytes::<GCM_NONCE_LEN>()?, data),
        })
    }

    /// Derive the per-object key (Algorithm 1, §7.6.4.3.1).
    fn object_key(&self, number: u32, generation: u16) -> Vec<u8> {
        let mut input = self.key.clone();
        input.extend_from_slice(&number.to_le_bytes()[..3]);
        input.extend_from_slice(&generation.to_le_bytes()[..2]);
        if self.method == Method::AesV2 {
            input.extend_from_slice(b"sAlT"); // §7.6.4.3.1 AES salt
        }
        let hash = Md5::digest(&input);
        let len = (self.key.len() + 5).min(16);
        hash[..len].to_vec()
    }
}

/// Key length in bytes from `/Length` (bits), clamped to a sane range.
fn key_len_bytes(length_bits: i64) -> usize {
    (length_bits / 8).clamp(5, 16) as usize
}

/// Determine the V4 stream cipher from `/StmF` and `/CF` (§7.6.5).
fn v4_method(encrypt: &Dictionary) -> Option<Method> {
    let stmf = encrypt.get_name(&Name::from("StmF"))?;
    let crypt_filters = encrypt.get_dict(&Name::from("CF"))?;
    let filter = crypt_filters.get_dict(stmf)?;
    match filter.get_name(&Name::from("CFM"))?.as_bytes() {
        b"V2" => Some(Method::Rc4),
        b"AESV2" => Some(Method::AesV2),
        b"AESV3" => Some(Method::AesV3),
        b"AESV4" => Some(Method::AesV4),
        _ => None,
    }
}

/// The V5 stream cipher from the crypt filter `/StmF` → `/CF`: AESV3 (CBC) or AESV4 (GCM). Defaults
/// to AESV3 when the crypt filter is absent or unrecognised (the common AES-256 case).
fn v5_method(encrypt: &Dictionary) -> Method {
    v4_method(encrypt)
        .filter(|m| matches!(m, Method::AesV3 | Method::AesV4))
        .unwrap_or(Method::AesV3)
}

/// Build an AES-256 (`AESV3`, V5/R6) handler by unwrapping the file key (§7.6.4.3.3–.4). The
/// password is tried as the user password (validated against `/U`, key from `/UE`) and then as the
/// owner password (validated against `/O` with `/U` as extra data, key from `/OE`). Returns `None`
/// if it validates as neither or the entries are malformed.
fn open_v5(
    encrypt: &Dictionary,
    revision: i64,
    password: &[u8],
) -> Option<StandardSecurityHandler> {
    let u = string_bytes(encrypt, "U")?;
    let ue = string_bytes(encrypt, "UE")?;
    if u.len() < 48 || ue.len() < 32 {
        return None;
    }

    // Algorithm 2.A: a salt/extra-data pair validates a password when hashing it matches the first
    // 32 bytes of the entry; a second hash then unwraps the file key from the matching `*E` entry.
    // `salts` is the 16-byte pair: validation salt (`[..8]`) then key salt (`[8..16]`).
    let unwrap = |salts: &[u8], extra: &[u8], expected: &[u8], wrapped: &[u8]| -> Option<Vec<u8>> {
        // Constant-time: the left operand is the hash of the caller's password (§7.6.4.3.4).
        let computed = hash_2b(revision, password, &salts[..8], extra);
        if computed.len() < 32 || !bool::from(computed[..32].ct_eq(&expected[..32])) {
            return None;
        }
        let intermediate = hash_2b(revision, password, &salts[8..16], extra);
        let key = aes256_cbc_decrypt_nopad(&intermediate, &[0u8; 16], &wrapped[..32])?;
        (key.len() == 32).then_some(key)
    };

    // User password: validation salt = U[32..40], key salt = U[40..48], no extra data.
    let key = unwrap(&u[32..48], &[], &u, &ue).or_else(|| {
        // Owner password: salts live in /O, and the full 48-byte /U is the extra data (§7.6.4.3.3).
        let o = string_bytes(encrypt, "O")?;
        let oe = string_bytes(encrypt, "OE")?;
        if o.len() < 48 || oe.len() < 32 {
            return None;
        }
        unwrap(&o[32..48], &u[..48], &o, &oe)
    })?;
    // Algorithm 13 (§7.6.4.3.4): /Perms seals /P under the file key, so a mismatch means the
    // permission word was altered after encryption. We do not *enforce* permissions on read — that
    // is the consuming viewer's job — but a document whose seal does not check out is not one we
    // will hand back as successfully opened.
    if !perms_seal_intact(encrypt, &key) {
        return None;
    }
    Some(StandardSecurityHandler {
        key,
        method: v5_method(encrypt),
    })
}

/// Verify the V5/R6 `/Perms` seal against `/P` and `/EncryptMetadata` (Algorithm 13, §7.6.4.3.4).
///
/// The 16-byte block decrypts (AES-256, zero IV, one block, no padding) to the permission word in
/// little-endian, `0xFFFFFFFF`, `T`/`F` for `EncryptMetadata`, then the literal `adb`. The `adb`
/// tag is what makes the check meaningful: without it, any 16 bytes would "decrypt" to something.
///
/// A **missing or malformed** `/Perms` is tolerated — plenty of real V5 files omit it, and refusing
/// them would break documents every other engine opens. Only a present-and-wrong seal is rejected.
fn perms_seal_intact(encrypt: &Dictionary, file_key: &[u8]) -> bool {
    let Some(perms) = string_bytes(encrypt, "Perms") else {
        return true; // absent: nothing to check against
    };
    if perms.len() != 16 {
        return true; // malformed: not a seal we can evaluate
    }
    let Some(block) = aes256_cbc_decrypt_nopad(file_key, &[0u8; 16], &perms) else {
        return true;
    };
    if block.len() != 16 || &block[9..12] != b"adb" {
        return true; // not a seal in the documented shape
    }
    // From here the block *is* a seal, so its contents must agree with the dictionary.
    let sealed = i32::from_le_bytes([block[0], block[1], block[2], block[3]]);
    let declared = encrypt
        .get_integer(&Name::from("P"))
        .and_then(|p| i32::try_from(p).ok());
    if declared != Some(sealed) {
        return false;
    }
    let sealed_metadata = block[8] == b'T';
    bool_entry(encrypt, "EncryptMetadata").unwrap_or(true) == sealed_metadata
}

/// The password hash of §7.6.4.3.4 (Algorithm 2.B). For R6 this is the hardened iterative hash
/// that alternates SHA-256/384/512; for the deprecated R5 it is a single SHA-256.
fn hash_2b(revision: i64, password: &[u8], salt: &[u8], udata: &[u8]) -> Vec<u8> {
    let mut k = {
        let mut h = Sha256::new();
        h.update(password);
        h.update(salt);
        h.update(udata);
        h.finalize().to_vec()
    };
    if revision < 6 {
        return k; // R5: plain SHA-256.
    }
    let mut round = 0usize;
    loop {
        // K1 = (password || K || udata) repeated 64 times; its length is always a multiple of 16.
        let mut k1 = Vec::with_capacity((password.len() + k.len() + udata.len()) * 64);
        for _ in 0..64 {
            k1.extend_from_slice(password);
            k1.extend_from_slice(&k);
            k1.extend_from_slice(udata);
        }
        let e = aes128_cbc_encrypt_nopad(&k[..16], &k[16..32], &k1);
        // The first 16 bytes of E, summed mod 3, select the hash for this round.
        let modulus: u32 = e[..16].iter().map(|&b| b as u32).sum::<u32>() % 3;
        k = match modulus {
            0 => Sha256::digest(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        round += 1;
        // Stop after at least 64 rounds, once the last byte of E is small enough (§7.6.4.3.4).
        if round >= 64 && *e.last().unwrap_or(&0) as usize <= round - 32 {
            break;
        }
    }
    k.truncate(32);
    k
}

/// Compute `/Perms`, the V5/R6 permissions seal (Algorithm 10, §7.6.4.3.4). A 16-byte block —
/// permissions (low 32 bits, little-endian) ‖ `0xFFFFFFFF` ‖ `T`/`F` for `EncryptMetadata` ‖ "adb"
/// ‖ 4 random bytes — encrypted with the file key under AES-256 ECB (CBC with a zero IV, one block).
fn compute_perms(permissions: i64, encrypt_metadata: bool, file_key: &[u8]) -> Option<Vec<u8>> {
    let mut block = [0u8; 16];
    block[..4].copy_from_slice(&(permissions as i32 as u32).to_le_bytes());
    block[4..8].copy_from_slice(&[0xFF; 4]);
    block[8] = if encrypt_metadata { b'T' } else { b'F' };
    block[9..12].copy_from_slice(b"adb");
    block[12..16].copy_from_slice(&random_bytes::<4>()?);
    Some(aes256_cbc_encrypt_nopad(file_key, &[0u8; 16], &block))
}

/// Generate `N` cryptographically random bytes (file keys, password salts, file IDs, AES IVs and
/// GCM nonces), or `None` if the OS RNG is unavailable.
///
/// The failure **must** propagate. Returning a zero buffer instead — as this once did, on the
/// reasoning that `getrandom` failing is effectively impossible — turns an unavailable RNG into
/// silent, total loss of confidentiality: the AES-256 file key becomes 32 zero bytes, so the
/// document is "encrypted" under a publicly known key and reported as saved successfully. For the
/// AES-GCM crypt filter it is worse than a weak key, because a nonce repeated under one key breaks
/// GCM outright — it leaks the authentication subkey, so an attacker can forge as well as read.
/// `getrandom` does fail in real deployments: a seccomp policy without the syscall, a chroot with
/// no `/dev/urandom`, an exhausted descriptor table on a platform that opens a file. A loud failure
/// is always better than an absent guarantee.
fn random_bytes<const N: usize>() -> Option<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).ok()?;
    Some(buf)
}

/// A fresh 32-byte `KDFSalt` for the PDF MAC key derivation (ISO/TS 32004 §5.1.1, Table 2). The
/// writer stores it as a direct byte string in the `/Encrypt` dictionary and feeds it to
/// [`pdf_mac_wrap_kdf`]. `None` if the OS RNG is unavailable.
#[must_use]
pub fn random_kdf_salt() -> Option<[u8; 32]> {
    random_bytes::<32>()
}

/// SHA-256 of `data` — the digest the PDF MAC token computes over a document's `ByteRange`
/// (ISO/TS 32004 §6.6, Table 8). Exposed so the document writer need not depend on `sha2`.
#[must_use]
pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// SHA-1 of `data` — the digest a DSS keys each VRI entry on (ISO 32000-2 §12.8.4.3, Table 261:
/// the base-16-encoded SHA-1 of the signature's `/Contents` hex string). Exposed so the document
/// layer can compute VRI keys without depending on `sha1` directly.
#[must_use]
pub fn sha1(data: &[u8]) -> [u8; 20] {
    sha1::Sha1::digest(data).into()
}

/// The RC4 key derived from the owner password, used to encrypt/decrypt `/O` (Algorithms 3 & 7).
fn owner_rc4_key(owner_pw: &[u8], revision: i64, key_len: usize) -> Vec<u8> {
    let mut hash = Md5::digest(pad_password(owner_pw)).to_vec();
    if revision >= 3 {
        for _ in 0..50 {
            hash = Md5::digest(&hash[..key_len]).to_vec();
        }
    }
    hash.truncate(key_len);
    hash
}

/// Compute `/O`, the owner-password entry (Algorithm 3, §7.6.4.4.2): RC4 the padded user password
/// under the owner key, 20 rounds for R≥3.
fn compute_owner(owner_pw: &[u8], user_pw: &[u8], revision: i64, key_len: usize) -> Vec<u8> {
    let rc4_key = owner_rc4_key(owner_pw, revision, key_len);
    let mut o = rc4(&rc4_key, &pad_password(user_pw));
    if revision >= 3 {
        for i in 1..=19u8 {
            let k: Vec<u8> = rc4_key.iter().map(|b| b ^ i).collect();
            o = rc4(&k, &o);
        }
    }
    o
}

/// Recover the (padded) user password from `/O` given the owner password (Algorithm 7, §7.6.4.4.8):
/// the inverse of [`compute_owner`] — the RC4 rounds run in reverse for R≥3.
fn recover_user_from_owner(
    owner_pw: &[u8],
    owner: &[u8],
    revision: i64,
    key_len: usize,
) -> Vec<u8> {
    let rc4_key = owner_rc4_key(owner_pw, revision, key_len);
    if revision < 3 {
        return rc4(&rc4_key, owner); // R2: a single RC4 pass.
    }
    let mut data = owner.to_vec();
    for i in (0..=19u8).rev() {
        let k: Vec<u8> = rc4_key.iter().map(|b| b ^ i).collect();
        data = rc4(&k, &data);
    }
    data
}

/// Whether `key` is the right file key: does the `/U` it produces (Algorithm 4/5) match the stored
/// `/U`? For R≥3 only the first 16 bytes are significant (the rest is arbitrary padding).
///
/// The comparison is constant-time. One operand is derived from the caller's password, so a
/// timing-variable `==` leaks how many leading bytes of the derived value are correct.
fn user_password_matches(key: &[u8], id0: &[u8], revision: i64, stored_u: &[u8]) -> bool {
    let computed = compute_user(key, id0, revision);
    let n = if revision >= 3 { 16 } else { 32 };
    computed.len() >= n && stored_u.len() >= n && bool::from(computed[..n].ct_eq(&stored_u[..n]))
}

/// Whether the `/Encrypt` dictionary is a standard security handler this crate can open (used to
/// distinguish a wrong password from an unsupported handler).
#[must_use]
pub fn supports(encrypt: &Dictionary) -> bool {
    if encrypt.get_name(&Name::from("Filter")).map(Name::as_bytes) != Some(b"Standard") {
        return false;
    }
    match encrypt.get_integer(&Name::from("V")).unwrap_or(0) {
        1 | 2 | 5 => true,
        4 => v4_method(encrypt).is_some(),
        _ => false,
    }
}

/// Compute `/U`, the user-password entry (Algorithm 4/5, §7.6.4.4.3–.4).
fn compute_user(file_key: &[u8], id0: &[u8], revision: i64) -> Vec<u8> {
    if revision < 3 {
        return rc4(file_key, &PAD); // R2: Algorithm 4.
    }
    let mut h = Md5::new();
    h.update(PAD);
    h.update(id0);
    let mut u = rc4(file_key, &h.finalize());
    for i in 1..=19u8 {
        let k: Vec<u8> = file_key.iter().map(|b| b ^ i).collect();
        u = rc4(&k, &u);
    }
    u.resize(32, 0); // pad to 32 bytes (§7.6.4.4.4, step (f))
    u
}

/// Compute the file encryption key (Algorithm 2, §7.6.4.3.2).
fn derive_key(
    user_password: &[u8],
    owner: &[u8],
    permissions: i64,
    id0: &[u8],
    revision: i64,
    key_len: usize,
    encrypt_metadata: bool,
) -> Vec<u8> {
    let mut input = Vec::new();
    input.extend_from_slice(&pad_password(user_password));
    let mut owner_32 = owner.to_vec();
    owner_32.resize(32, 0);
    input.extend_from_slice(&owner_32[..32]);
    input.extend_from_slice(&(permissions as u32).to_le_bytes());
    input.extend_from_slice(id0);
    if revision >= 4 && !encrypt_metadata {
        input.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    let mut hash = Md5::digest(&input).to_vec();
    if revision >= 3 {
        for _ in 0..50 {
            hash = Md5::digest(&hash[..key_len]).to_vec();
        }
    }
    hash.truncate(key_len);
    hash
}

/// Pad or truncate a password to the 32-byte form (§7.6.4.3.2, step (a)).
fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = password.len().min(32);
    out[..n].copy_from_slice(&password[..n]);
    out[n..].copy_from_slice(&PAD[..32 - n]);
    out
}

/// Read a dictionary entry as raw string bytes.
fn string_bytes(dict: &Dictionary, key: &str) -> Option<Vec<u8>> {
    match dict.get(&Name::from(key))? {
        Object::String(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// Read a boolean dictionary entry.
fn bool_entry(dict: &Dictionary, key: &str) -> Option<bool> {
    dict.get(&Name::from(key))?.as_bool()
}

#[cfg(test)]
mod tests;
