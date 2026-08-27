use super::*;

// ---------------------------------------------------------------------------------------------
// Access permissions (§7.6.3.2, Table 22)
//
// `Permissions` is a newtype over the `/P` flag word with no C representation, so it crosses as
// the raw `int32_t` and is composed with these functions: start from `restricted` (nothing
// allowed) or `all`, then grant one operation at a time. Passing an arbitrary integer is allowed —
// the value is stored as given, matching what §7.6.3.2 asks of readers.
// ---------------------------------------------------------------------------------------------

/// The `/P` word with every grantable bit cleared: nothing is allowed. The starting point for
/// composing a permission set.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_restricted() -> i32 {
    Permissions::RESTRICTED.bits()
}

/// The `/P` word with everything allowed — the common default.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_all() -> i32 {
    Permissions::ALL.bits()
}

/// Grant printing (bit 3); combine with [`prismpdf_permissions_allow_print_high_res`] for full
/// quality.
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_print(permissions: i32) -> i32 {
    Permissions::from_bits(permissions).allow_print().bits()
}

/// Grant modifying the document's contents (bit 4).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_modify(permissions: i32) -> i32 {
    Permissions::from_bits(permissions).allow_modify().bits()
}

/// Grant copying text and graphics out of the document (bit 5).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_copy(permissions: i32) -> i32 {
    Permissions::from_bits(permissions).allow_copy().bits()
}

/// Grant adding or modifying annotations (bit 6).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_annotate(permissions: i32) -> i32 {
    Permissions::from_bits(permissions).allow_annotate().bits()
}

/// Grant filling in interactive form fields (bit 9).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_fill_forms(permissions: i32) -> i32 {
    Permissions::from_bits(permissions)
        .allow_fill_forms()
        .bits()
}

/// Grant extracting content for accessibility (bit 10).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_accessibility(permissions: i32) -> i32 {
    Permissions::from_bits(permissions)
        .allow_accessibility()
        .bits()
}

/// Grant assembling the document — insert, rotate or delete pages (bit 11).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_assemble(permissions: i32) -> i32 {
    Permissions::from_bits(permissions).allow_assemble().bits()
}

/// Grant full-quality printing (bit 12).
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_permissions_allow_print_high_res(permissions: i32) -> i32 {
    Permissions::from_bits(permissions)
        .allow_print_high_res()
        .bits()
}

// ---------------------------------------------------------------------------------------------
// Encryption (§7.6)
// ---------------------------------------------------------------------------------------------

/// Map the stable integer algorithm selector onto [`Algorithm`]; `None` for an unknown value.
pub(crate) fn algorithm_from_code(code: u32) -> Option<Algorithm> {
    match code {
        0 => Some(Algorithm::Rc4),
        1 => Some(Algorithm::Aes128),
        2 => Some(Algorithm::Aes256),
        3 => Some(Algorithm::Aes256Gcm),
        _ => None,
    }
}

/// Encrypted full rewrite with explicit access permissions (§7.6.3.2) and metadata handling —
/// the complete form of [`prismpdf_document_save_encrypted`], which always grants everything.
///
/// `permissions` is a `/P` word from the `prismpdf_permissions_*` family. `encrypt_metadata`
/// false leaves the `/Metadata` stream in clear text (§7.6.3), as PDF/A requires.
/// `algorithm`: `0` = RC4-128, `1` = AES-128, `2` = AES-256.
///
/// # Safety
/// `user_password`/`owner_password` must each point to their stated length of readable bytes (or
/// be null with length 0). `doc` must be live; `out_data`/`out_len` writable. Release the buffer
/// with [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_encrypted_with(
    doc: *const PrismPdfDocument,
    user_password: *const u8,
    user_len: usize,
    owner_password: *const u8,
    owner_len: usize,
    permissions: i32,
    encrypt_metadata: bool,
    algorithm: u32,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(algorithm) = algorithm_from_code(algorithm) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let user = unsafe { slice_or_empty(user_password, user_len) };
        let owner_raw = unsafe { slice_or_empty(owner_password, owner_len) };
        // An empty owner password means "same as user", matching the simple entry point.
        let owner = if owner_raw.is_empty() {
            user.clone()
        } else {
            owner_raw
        };
        match document.save_encrypted_with(
            &user,
            &owner,
            Permissions::from_bits(permissions),
            encrypt_metadata,
            algorithm,
        ) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Public-key (certificate) encryption (§7.6.5): each recipient's X.509 certificate is given as
/// DER, and any one of their private keys can open the result.
///
/// `certs` and `cert_lens` are parallel arrays of `count` entries. `permissions` is a `/P` word
/// from the `prismpdf_permissions_*` family; `algorithm` as in
/// [`prismpdf_document_save_encrypted_with`].
///
/// # Safety
/// `certs` must point to `count` non-null pointers, each with at least the matching `cert_lens`
/// readable bytes. `doc` must be live; `out_data`/`out_len` writable. Release the buffer with
/// [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_encrypted_public_key(
    doc: *const PrismPdfDocument,
    certs: *const *const u8,
    cert_lens: *const usize,
    count: usize,
    permissions: i32,
    encrypt_metadata: bool,
    algorithm: u32,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if count == 0 || certs.is_null() || cert_lens.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let Some(algorithm) = algorithm_from_code(algorithm) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let mut recipients: Vec<&[u8]> = Vec::with_capacity(count);
        for i in 0..count {
            let (ptr, len) = unsafe { (*certs.add(i), *cert_lens.add(i)) };
            if ptr.is_null() {
                return PrismPdfStatus::NullArgument;
            }
            recipients.push(unsafe { std::slice::from_raw_parts(ptr, len) });
        }
        match document.save_encrypted_public_key_with(
            &recipients,
            Permissions::from_bits(permissions),
            encrypt_metadata,
            algorithm,
        ) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Encrypted full rewrite carrying a **PDF MAC** (ISO/TS 32004): an authentication tag over the
/// whole file, so tampering is detectable and not merely undecryptable. Verify it with
/// [`prismpdf_document_verify_pdf_mac`].
///
/// Arguments match [`prismpdf_document_save_encrypted_with`].
///
/// # Safety
/// As [`prismpdf_document_save_encrypted_with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_save_encrypted_with_mac(
    doc: *const PrismPdfDocument,
    user_password: *const u8,
    user_len: usize,
    owner_password: *const u8,
    owner_len: usize,
    permissions: i32,
    encrypt_metadata: bool,
    algorithm: u32,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(algorithm) = algorithm_from_code(algorithm) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        let user = unsafe { slice_or_empty(user_password, user_len) };
        let owner_raw = unsafe { slice_or_empty(owner_password, owner_len) };
        let owner = if owner_raw.is_empty() {
            user.clone()
        } else {
            owner_raw
        };
        match document.save_encrypted_with_mac_full(
            &user,
            &owner,
            Permissions::from_bits(permissions),
            encrypt_metadata,
            algorithm,
        ) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Verify a document's PDF MAC (ISO/TS 32004) with `password`, writing the verdict to
/// `*out_valid`. [`PrismPdfStatus::NotFound`] means the document carries no MAC at all — which is
/// not a failure, just an unprotected file.
///
/// # Safety
/// `password` must point to `password_len` readable bytes (or be null with length 0). `doc` must
/// be live and `out_valid` a writable `*mut bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_verify_pdf_mac(
    doc: *const PrismPdfDocument,
    password: *const u8,
    password_len: usize,
    out_valid: *mut bool,
) -> PrismPdfStatus {
    if out_valid.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_valid = false };
    if doc.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let document = unsafe { &(*doc).0 };
    guard(|| {
        let password = unsafe { slice_or_empty(password, password_len) };
        match document.verify_pdf_mac(&password) {
            Ok(Some(valid)) => {
                unsafe { *out_valid = valid };
                PrismPdfStatus::Ok
            }
            Ok(None) => PrismPdfStatus::NotFound,
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Digital signatures (§12.8)
//
// Signing takes more optional inputs than a C signature can carry, so `SignSettings` crosses as a
// **mutable handle**: create it, set what you need, sign with it, free it. This is the first
// mutable handle in the ABI and the pattern the authoring tranche will reuse for `Builder`.
// ---------------------------------------------------------------------------------------------

/// A mutable bag of optional signing parameters (§12.8.1). Created by
/// [`prismpdf_sign_settings_new`], released by [`prismpdf_sign_settings_free`].
pub struct PrismPdfSignSettings(pub(crate) SignSettings);

/// Create a settings handle with every option unset — equivalent to what
/// [`prismpdf_document_sign`] uses.
///
/// # Safety
/// The returned handle must be released with [`prismpdf_sign_settings_free`].
#[unsafe(no_mangle)]
pub extern "C" fn prismpdf_sign_settings_new() -> *mut PrismPdfSignSettings {
    guard_ptr(|| Box::into_raw(Box::new(PrismPdfSignSettings(SignSettings::default()))))
}

/// Release a settings handle. Freeing `NULL` is a no-op.
///
/// # Safety
/// `settings` must come from [`prismpdf_sign_settings_new`] and must not already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_free(settings: *mut PrismPdfSignSettings) {
    unsafe { free_handle(settings) }
}

/// Borrow a settings handle and read a C string argument, or `None` on a null/invalid argument.
///
/// # Safety
/// `settings` is a settings handle (or null); `value` is a C string pointer (or null).
pub(crate) unsafe fn settings_and_str<'a>(
    settings: *mut PrismPdfSignSettings,
    value: *const c_char,
) -> Option<(&'a mut SignSettings, String)> {
    if settings.is_null() || value.is_null() {
        return None;
    }
    let text = unsafe { utf8(value) }?;
    Some((unsafe { &mut (*settings).0 }, text.to_string()))
}

/// Set the signer's name (`/Name`, §12.8.1).
///
/// # Safety
/// `settings` must be live and `name` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_name(
    settings: *mut PrismPdfSignSettings,
    name: *const c_char,
) -> PrismPdfStatus {
    let Some((settings, value)) = (unsafe { settings_and_str(settings, name) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        settings.name = Some(value);
        PrismPdfStatus::Ok
    })
}

/// Set the reason for signing (`/Reason`, §12.8.1).
///
/// # Safety
/// `settings` must be live and `reason` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_reason(
    settings: *mut PrismPdfSignSettings,
    reason: *const c_char,
) -> PrismPdfStatus {
    let Some((settings, value)) = (unsafe { settings_and_str(settings, reason) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        settings.reason = Some(value);
        PrismPdfStatus::Ok
    })
}

/// Set the signing location (`/Location`, §12.8.1).
///
/// # Safety
/// `settings` must be live and `location` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_location(
    settings: *mut PrismPdfSignSettings,
    location: *const c_char,
) -> PrismPdfStatus {
    let Some((settings, value)) = (unsafe { settings_and_str(settings, location) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        settings.location = Some(value);
        PrismPdfStatus::Ok
    })
}

/// Set the signer's contact information (`/ContactInfo`, §12.8.1).
///
/// # Safety
/// `settings` must be live and `contact` a NUL-terminated UTF-8 C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_contact_info(
    settings: *mut PrismPdfSignSettings,
    contact: *const c_char,
) -> PrismPdfStatus {
    let Some((settings, value)) = (unsafe { settings_and_str(settings, contact) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| {
        settings.contact_info = Some(value);
        PrismPdfStatus::Ok
    })
}

/// Pin the signing time as a Unix timestamp, instead of taking the current clock. Passing this
/// makes signing deterministic, which is what a reproducible build or a test needs.
///
/// # Safety
/// `settings` must be a live settings handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_signing_time(
    settings: *mut PrismPdfSignSettings,
    unix_time: u64,
) -> PrismPdfStatus {
    if settings.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { (*settings).0.signing_time = Some(unix_time) };
        PrismPdfStatus::Ok
    })
}

/// Request a **PAdES** (ETSI EN 319 142) signature rather than a plain CMS one.
///
/// # Safety
/// `settings` must be a live settings handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_pades(
    settings: *mut PrismPdfSignSettings,
    pades: bool,
) -> PrismPdfStatus {
    if settings.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { (*settings).0.pades = pades };
        PrismPdfStatus::Ok
    })
}

/// Give the signature a visible appearance: a widget on page `page_index` (0-based) at `rect`
/// (`[llx lly urx ury]`, four floats), optionally captioned with `text`.
///
/// Pass a null `text` for an unlabelled box.
///
/// # Safety
/// `settings` must be live, `rect` must point to 4 readable `float`s, and `text` must be a
/// NUL-terminated UTF-8 C string or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_appearance(
    settings: *mut PrismPdfSignSettings,
    page_index: usize,
    rect: *const f32,
    text: *const c_char,
) -> PrismPdfStatus {
    if settings.is_null() || rect.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let mut bounds = [0.0f32; 4];
        unsafe { std::ptr::copy_nonoverlapping(rect, bounds.as_mut_ptr(), 4) };
        let Ok(caption) = (unsafe { read_opt_str(text) }) else {
            return PrismPdfStatus::NullArgument;
        };
        unsafe {
            (*settings).0.appearance = Some(SignatureAppearance {
                page_index,
                rect: bounds,
                text: caption,
            });
        }
        PrismPdfStatus::Ok
    })
}

/// Embed a signature timestamp (§12.8.3.3) produced from the given TSA credentials.
///
/// # Safety
/// `cert_der`/`key_der` must point to their stated lengths of readable bytes. `settings` must be
/// a live settings handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_sign_settings_set_timestamp(
    settings: *mut PrismPdfSignSettings,
    cert_der: *const u8,
    cert_len: usize,
    key_der: *const u8,
    key_len: usize,
    gen_time: u64,
    serial: u64,
) -> PrismPdfStatus {
    if settings.is_null() || cert_der.is_null() || key_der.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let cert = unsafe { slice_or_empty(cert_der, cert_len) };
        let key = unsafe { slice_or_empty(key_der, key_len) };
        unsafe {
            (*settings).0.timestamp = Some(TsaCredentials {
                cert_der: cert.into_owned(),
                key_der: key.into_owned(),
                gen_time,
                serial,
            });
        }
        PrismPdfStatus::Ok
    })
}

/// Sign the document with an X.509 certificate and its private key, both DER-encoded, and return
/// the signed PDF as an incremental update (§7.5.6, §12.8).
///
/// Equivalent to [`prismpdf_document_sign_with`] with default settings.
///
/// # Safety
/// `cert_der`/`key_der` must point to their stated lengths of readable bytes. `doc` must be live;
/// `out_data`/`out_len` writable. Release the buffer with [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_sign(
    doc: *const PrismPdfDocument,
    cert_der: *const u8,
    cert_len: usize,
    key_der: *const u8,
    key_len: usize,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if cert_der.is_null() || key_der.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let cert = unsafe { slice_or_empty(cert_der, cert_len) };
        let key = unsafe { slice_or_empty(key_der, key_len) };
        match document.sign(&cert, &key) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Sign with explicit settings (name, reason, location, appearance, timestamp, PAdES, …).
///
/// # Safety
/// As [`prismpdf_document_sign`], plus `settings` must be a live settings handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_sign_with(
    doc: *const PrismPdfDocument,
    cert_der: *const u8,
    cert_len: usize,
    key_der: *const u8,
    key_len: usize,
    settings: *const PrismPdfSignSettings,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if cert_der.is_null() || key_der.is_null() || settings.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let cert = unsafe { slice_or_empty(cert_der, cert_len) };
        let key = unsafe { slice_or_empty(key_der, key_len) };
        let settings = unsafe { &(*settings).0 };
        match document.sign_with(&cert, &key, settings) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Sign an encrypted document and refresh its PDF MAC (ISO/TS 32004) in the same revision, so the
/// authentication tag still covers the file after the signature is appended.
///
/// # Safety
/// As [`prismpdf_document_sign_with`], plus `password` must point to `password_len` readable bytes
/// (or be null with length 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_sign_with_mac(
    doc: *const PrismPdfDocument,
    cert_der: *const u8,
    cert_len: usize,
    key_der: *const u8,
    key_len: usize,
    settings: *const PrismPdfSignSettings,
    password: *const u8,
    password_len: usize,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if cert_der.is_null() || key_der.is_null() || settings.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let cert = unsafe { slice_or_empty(cert_der, cert_len) };
        let key = unsafe { slice_or_empty(key_der, key_len) };
        let pass = unsafe { slice_or_empty(password, password_len) };
        let settings = unsafe { &(*settings).0 };
        match document.sign_with_mac(&cert, &key, settings, &pass) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

/// Append a **document timestamp** (§12.8.5) signed by the given TSA credentials — a signature
/// over the whole file that proves it existed at `gen_time`, with no signer identity attached.
///
/// Pass `has_gen_time` false to take the current clock.
///
/// # Safety
/// `tsa_cert_der`/`tsa_key_der` must point to their stated lengths of readable bytes. `doc` must
/// be live; `out_data`/`out_len` writable. Release the buffer with [`prismpdf_bytes_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_timestamp(
    doc: *const PrismPdfDocument,
    tsa_cert_der: *const u8,
    cert_len: usize,
    tsa_key_der: *const u8,
    key_len: usize,
    gen_time: u64,
    has_gen_time: bool,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_bytes_out(doc, out_data, out_len) }) else {
        return PrismPdfStatus::NullArgument;
    };
    if tsa_cert_der.is_null() || tsa_key_der.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        let cert = unsafe { slice_or_empty(tsa_cert_der, cert_len) };
        let key = unsafe { slice_or_empty(tsa_key_der, key_len) };
        let when = if has_gen_time { Some(gen_time) } else { None };
        match document.timestamp(&cert, &key, when) {
            Ok(bytes) => emit_bytes(bytes, out_data, out_len),
            Err(_) => PrismPdfStatus::Parse,
        }
    })
}

// --- Verification -----------------------------------------------------------------------------

/// Whether a signed document's certificate chain revocation state could be established (§12.8.4).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PrismPdfRevocation {
    /// Every non-anchor link is covered by verified material and none is revoked.
    Good = 0,
    /// At least one link is revoked.
    Revoked = 1,
    /// No link is revoked, but at least one has no usable material — the long-term claim is
    /// incomplete.
    Incomplete = 2,
}

/// The verification result for one signature. Borrowed from a [`PrismPdfSignatureList`].
#[repr(transparent)]
pub struct PrismPdfSignature(pub(crate) SignatureStatus);

/// An owned list of signature verification results. Released by
/// [`prismpdf_signature_list_free`].
pub struct PrismPdfSignatureList(pub(crate) Vec<PrismPdfSignature>);

/// Verify every signature in the document (§12.8), checking each one's byte coverage and CMS
/// integrity. Trust is not evaluated — see [`prismpdf_document_verify_signatures_with`].
///
/// # Safety
/// `doc` must be live and `out_list` writable. Release it with [`prismpdf_signature_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_verify_signatures(
    doc: *const PrismPdfDocument,
    out_list: *mut *mut PrismPdfSignatureList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.verify_signatures() {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfSignature).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfSignatureList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Read a `(ptr, len)` array pair as a vector of owned byte vectors — the shape every
/// trust-anchor argument takes.
///
/// # Safety
/// `items` must point to `count` non-null pointers, each with at least the matching `lens` bytes.
pub(crate) unsafe fn collect_der_list(
    items: *const *const u8,
    lens: *const usize,
    count: usize,
) -> Option<Vec<Vec<u8>>> {
    if count == 0 {
        return Some(Vec::new());
    }
    if items.is_null() || lens.is_null() {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (ptr, len) = unsafe { (*items.add(i), *lens.add(i)) };
        if ptr.is_null() {
            return None;
        }
        out.push(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec());
    }
    Some(out)
}

/// Verify every signature and evaluate trust against the supplied DER root certificates, so each
/// result's `trusted` flag becomes meaningful.
///
/// # Safety
/// `roots`/`root_lens` must be parallel arrays of `count` entries, each pointer non-null with at
/// least its stated readable bytes. `doc` must be live and `out_list` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_verify_signatures_with(
    doc: *const PrismPdfDocument,
    roots: *const *const u8,
    root_lens: *const usize,
    count: usize,
    out_list: *mut *mut PrismPdfSignatureList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(anchors) = (unsafe { collect_der_list(roots, root_lens, count) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.verify_signatures_with(&anchors) {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfSignature).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfSignatureList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Verify with trust **and** long-term validation (§12.8.4): the document's DSS revocation
/// material is fed into the check, so each result's revocation summary becomes meaningful.
///
/// # Safety
/// As [`prismpdf_document_verify_signatures_with`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_document_verify_signatures_ltv(
    doc: *const PrismPdfDocument,
    roots: *const *const u8,
    root_lens: *const usize,
    count: usize,
    out_list: *mut *mut PrismPdfSignatureList,
) -> PrismPdfStatus {
    let Some(document) = (unsafe { prepare_list_out(doc, out_list) }) else {
        return PrismPdfStatus::NullArgument;
    };
    let Some(anchors) = (unsafe { collect_der_list(roots, root_lens, count) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| match document.verify_signatures_ltv(&anchors) {
        Ok(items) => {
            let list = items.into_iter().map(PrismPdfSignature).collect();
            unsafe { *out_list = Box::into_raw(Box::new(PrismPdfSignatureList(list))) };
            PrismPdfStatus::Ok
        }
        Err(_) => PrismPdfStatus::Parse,
    })
}

/// Number of signatures in `list`.
///
/// # Safety
/// `list` must be a live list handle and `out_len` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_list_len(
    list: *const PrismPdfSignatureList,
    out_len: *mut usize,
) -> PrismPdfStatus {
    if out_len.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_len = 0 };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_len = (*list).0.len() };
        PrismPdfStatus::Ok
    })
}

/// Lend signature result `index` from `list`. Borrowed — valid until the list is freed.
///
/// # Safety
/// `list` must be live and `out_item` a writable `*mut *const PrismPdfSignature`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_list_get(
    list: *const PrismPdfSignatureList,
    index: usize,
    out_item: *mut *const PrismPdfSignature,
) -> PrismPdfStatus {
    if out_item.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_item = std::ptr::null() };
    if list.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    let items = unsafe { &(*list).0 };
    guard(|| match items.get(index) {
        Some(item) => {
            unsafe { *out_item = item as *const PrismPdfSignature };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Release a signature list. Freeing `NULL` is a no-op.
///
/// # Safety
/// `list` must come from one of the `prismpdf_document_verify_signatures*` calls and must not
/// already be freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_list_free(list: *mut PrismPdfSignatureList) {
    unsafe { free_handle(list) }
}

/// Whether the signature's CMS verifies and covers the bytes it claims to cover.
///
/// This is integrity, not trust: a self-signed certificate can be `valid`. Check
/// [`prismpdf_signature_trusted`] as well.
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_valid` a writable `*mut bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_valid(
    sig: *const PrismPdfSignature,
    out_valid: *mut bool,
) -> PrismPdfStatus {
    if out_valid.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_valid = false };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_valid = (*sig).0.valid };
        PrismPdfStatus::Ok
    })
}

/// The signer's distinguished name from the signing certificate, or
/// [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `sig` must be borrowed from a live list; `out_text` must be writable. Release the string with
/// [`prismpdf_string_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_signer(
    sig: *const PrismPdfSignature,
    out_text: *mut *mut c_char,
) -> PrismPdfStatus {
    let Some(item) = (unsafe { prepare_string_out(sig, out_text) }) else {
        return PrismPdfStatus::NullArgument;
    };
    guard(|| unsafe { store_opt_string(item.0.signer.as_deref(), out_text) })
}

/// How many bytes of the file this signature covers — compare against the file length to detect
/// content appended after signing.
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_bytes` a writable `*mut usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_covered_bytes(
    sig: *const PrismPdfSignature,
    out_bytes: *mut usize,
) -> PrismPdfStatus {
    if out_bytes.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_bytes = 0 };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_bytes = (*sig).0.covered_bytes };
        PrismPdfStatus::Ok
    })
}

/// The claimed signing time as a Unix timestamp, or [`PrismPdfStatus::NotFound`].
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_time` a writable `*mut i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_signing_time(
    sig: *const PrismPdfSignature,
    out_time: *mut i64,
) -> PrismPdfStatus {
    if out_time.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_time = 0 };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*sig).0.signing_time } {
        Some(time) => {
            unsafe { *out_time = time };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// The timestamp-token time as a Unix timestamp (§12.8.3.3), or [`PrismPdfStatus::NotFound`] when
/// the signature carries no timestamp.
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_time` a writable `*mut i64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_timestamp_time(
    sig: *const PrismPdfSignature,
    out_time: *mut i64,
) -> PrismPdfStatus {
    if out_time.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_time = 0 };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*sig).0.timestamp_time } {
        Some(time) => {
            unsafe { *out_time = time };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Whether the signing certificate chains to one of the roots supplied to
/// [`prismpdf_document_verify_signatures_with`]. [`PrismPdfStatus::NotFound`] means trust was
/// never evaluated — the plain [`prismpdf_document_verify_signatures`] was used.
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_trusted` a writable `*mut bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_trusted(
    sig: *const PrismPdfSignature,
    out_trusted: *mut bool,
) -> PrismPdfStatus {
    if out_trusted.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_trusted = false };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*sig).0.trusted } {
        Some(trusted) => {
            unsafe { *out_trusted = trusted };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

/// Whether the signature is a PAdES (ETSI EN 319 142) one.
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_pades` a writable `*mut bool`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_pades(
    sig: *const PrismPdfSignature,
    out_pades: *mut bool,
) -> PrismPdfStatus {
    if out_pades.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    unsafe { *out_pades = false };
    if sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| {
        unsafe { *out_pades = (*sig).0.pades };
        PrismPdfStatus::Ok
    })
}

/// The chain revocation summary (§12.8.4), or [`PrismPdfStatus::NotFound`] when revocation was not
/// evaluated — anything other than [`prismpdf_document_verify_signatures_ltv`].
///
/// # Safety
/// `sig` must be borrowed from a live list and `out_revocation` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn prismpdf_signature_revocation(
    sig: *const PrismPdfSignature,
    out_revocation: *mut PrismPdfRevocation,
) -> PrismPdfStatus {
    if out_revocation.is_null() || sig.is_null() {
        return PrismPdfStatus::NullArgument;
    }
    guard(|| match unsafe { (*sig).0.revocation } {
        Some(summary) => {
            let value = match summary {
                RevocationSummary::Good => PrismPdfRevocation::Good,
                RevocationSummary::Revoked => PrismPdfRevocation::Revoked,
                RevocationSummary::Incomplete => PrismPdfRevocation::Incomplete,
            };
            unsafe { *out_revocation = value };
            PrismPdfStatus::Ok
        }
        None => PrismPdfStatus::NotFound,
    })
}

// ---------------------------------------------------------------------------------------------
// Content streams (§8.2-§8.6, §9.4)
//
// `Content` is a byte builder for a page's operator stream, so it crosses as a mutable handle
// like `SignSettings`. Every operator takes numbers, strings or slices — nothing here needs a new
// convention. The assembled bytes are *lent* (`prismpdf_content_bytes`) rather than copied, then
// handed to `prismpdf_builder_add_page`.
