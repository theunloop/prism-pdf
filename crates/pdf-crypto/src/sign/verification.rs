use super::*;

/// Verify a detached CMS `SignedData` (`cms_der`) against `message` (the `/ByteRange` bytes,
/// §12.8.3.3): the signed message-digest attribute must equal `SHA-256(message)`, and the embedded
/// certificate's RSA key must verify the signature over the signed attributes. Total and panic-free.
#[must_use]
pub fn verify_detached(cms_der: &[u8], message: &[u8]) -> VerifiedSignature {
    verify_detached_with(cms_der, message, &VerifyOptions::default())
}

/// As [`verify_detached`], but additionally evaluating PAdES-B context from [`VerifyOptions`]: the
/// signer's certificate chain against a trust store, the signing-time attribute, and any embedded
/// RFC 3161 timestamp.
#[must_use]
pub fn verify_detached_with(
    cms_der: &[u8],
    message: &[u8],
    opts: &VerifyOptions,
) -> VerifiedSignature {
    verify_inner(cms_der, message, opts).unwrap_or_default()
}

/// Fallible body of [`verify_detached_with`]; `None` anywhere means "not a valid signature".
pub(super) fn verify_inner(
    cms_der: &[u8],
    message: &[u8],
    opts: &VerifyOptions,
) -> Option<VerifiedSignature> {
    let content_info = ContentInfo::from_der(cms_der).ok()?;
    let signed_data = content_info.content.decode_as::<SignedData>().ok()?;

    // The messageDigest attribute hashes the content with the signer's digest algorithm (SHA-256 for
    // RSA, SHA-512 for Ed25519 per RFC 8419) — compute the matching digest of `message`.
    let expected = signer_message_digest(&signed_data, message)?;
    let valid = verify_signer(&signed_data, &expected)?;
    let cert = embedded_leaf(&signed_data)?;
    let signer = Some(cert.tbs_certificate.subject.to_string());
    let signing_time = signer_signing_time(&signed_data);
    let timestamp_time = verify_timestamp(&signed_data);

    // PAdES-B chain validation: chain the leaf to a trusted root as of `validation_instant`.
    // With revocation material supplied (PAdES-LT), the built chain is then checked link by link.
    let mut revocation = None;
    let trusted = if opts.roots.is_empty() {
        None
    } else {
        let roots: Vec<Certificate> = opts
            .roots
            .iter()
            .filter_map(|der| Certificate::from_der(der).ok())
            .collect();
        let intermediates = embedded_certs(&signed_data);
        let at = validation_instant(timestamp_time, signing_time);
        let chain = build_chain(cert, &intermediates, &roots, at);
        if let (Some(chain), Some(data)) = (&chain, &opts.revocation) {
            revocation = Some(crate::chain_revocation(chain, data, at));
        }
        Some(chain.is_some())
    };

    let pades = verify_signing_cert_v2(&signed_data, cert);

    Some(VerifiedSignature {
        valid,
        signer,
        signing_time,
        trusted,
        timestamp_time,
        pades,
        revocation,
        signer_count: signer_count(&signed_data),
    })
}

/// Whether the first signer carries a `signing-certificate-v2` signed attribute (RFC 5035) whose
/// `certHash` matches `cert` (PAdES-B). False if absent or mismatched.
pub(super) fn verify_signing_cert_v2(signed_data: &SignedData, cert: &Certificate) -> bool {
    let Some(scv2) = (|| {
        let signer_info = signed_data.signer_infos.0.as_slice().first()?;
        let attr = signer_info
            .signed_attrs
            .as_ref()?
            .iter()
            .find(|a| a.oid == ID_AA_SIGNING_CERTIFICATE_V2)?;
        let value = attr.values.as_slice().first()?;
        SigningCertificateV2::from_der(&value.to_der().ok()?).ok()
    })() else {
        return false;
    };
    let Ok(cert_der) = cert.to_der() else {
        return false;
    };
    scv2.certs
        .first()
        .is_some_and(|id| id.cert_hash.as_bytes() == Sha256::digest(&cert_der).as_slice())
}

/// Verify the first signer's RSA-SHA256 signature over its signed attributes, the signed attributes
/// pinning `expected_digest` as the message digest. Returns `Some(valid)`, or `None` if the
/// structure is unusable (no signer, no embedded cert, no signed attributes).
pub(super) fn verify_signer(signed_data: &SignedData, expected_digest: &[u8]) -> Option<bool> {
    let signer_info = signed_data.signer_infos.0.as_slice().first()?;
    let cert = embedded_leaf(signed_data)?;

    let signed_attrs = signer_info.signed_attrs.as_ref()?;
    let claimed_digest = signed_attrs
        .iter()
        .find(|attr| attr.oid == ID_MESSAGE_DIGEST)
        .and_then(|attr| attr.values.as_slice().first())
        .and_then(|value| value.decode_as::<der::asn1::OctetString>().ok())?;
    if claimed_digest.as_bytes() != expected_digest {
        return Some(false);
    }

    // Re-encode the signed attributes as a DER SET OF (the tagging used when signed, RFC 5652 §5.4)
    // and verify the signature over them, dispatching on the signer's signature algorithm.
    let tbs = SetOfVec::from_iter(signed_attrs.iter().cloned())
        .ok()?
        .to_der()
        .ok()?;
    let spki_der = cert.tbs_certificate.subject_public_key_info.to_der().ok()?;

    let sig_oid = signer_info.signature_algorithm.oid;
    if sig_oid == ID_ED25519 {
        // Ed25519 (RFC 8419): PureEdDSA over the signed-attributes DER, no pre-hash.
        let key = Ed25519VerifyingKey::from_public_key_der(&spki_der).ok()?;
        let sig = Ed25519Signature::from_slice(signer_info.signature.as_bytes()).ok()?;
        Some(key.verify(&tbs, &sig).is_ok())
    } else if sig_oid == ID_ECDSA_SHA256 {
        // ECDSA P-256 over SHA-256; the signature is the DER ECDSA-Sig-Value.
        let key = p256::ecdsa::VerifyingKey::from_public_key_der(&spki_der).ok()?;
        let sig = p256::ecdsa::Signature::from_der(signer_info.signature.as_bytes()).ok()?;
        Some(key.verify(&tbs, &sig).is_ok())
    } else if sig_oid == ID_ECDSA_SHA384 {
        // ECDSA P-384 over SHA-384.
        let key = p384::ecdsa::VerifyingKey::from_public_key_der(&spki_der).ok()?;
        let sig = p384::ecdsa::Signature::from_der(signer_info.signature.as_bytes()).ok()?;
        Some(key.verify(&tbs, &sig).is_ok())
    } else if sig_oid == ID_ECDSA_SHA512 {
        // ECDSA P-521 over SHA-512 (ISO/TS 32002 Table 3), via the generic `ecdsa` types
        // (`p521` has no `DigestPrimitive`, so the SHA-512 prehash is computed here).
        use ecdsa::signature::hazmat::PrehashVerifier as _;
        let key = ecdsa::VerifyingKey::<p521::NistP521>::from_public_key_der(&spki_der).ok()?;
        let sig =
            ecdsa::Signature::<p521::NistP521>::from_der(signer_info.signature.as_bytes()).ok()?;
        Some(
            key.verify_prehash(Sha512::digest(&tbs).as_slice(), &sig)
                .is_ok(),
        )
    } else if sig_oid == ID_RSA_SHA3_256 || sig_oid == ID_RSA_SHA3_384 || sig_oid == ID_RSA_SHA3_512
    {
        // RSA PKCS#1 v1.5 over a SHA-3 digest (ISO/TS 32001 §5.1) — verify-side acceptance.
        let public_key = rsa::RsaPublicKey::from_public_key_der(&spki_der).ok()?;
        let signature = Signature::try_from(signer_info.signature.as_bytes()).ok()?;
        Some(if sig_oid == ID_RSA_SHA3_256 {
            VerifyingKey::<sha3::Sha3_256>::new(public_key)
                .verify(&tbs, &signature)
                .is_ok()
        } else if sig_oid == ID_RSA_SHA3_384 {
            VerifyingKey::<sha3::Sha3_384>::new(public_key)
                .verify(&tbs, &signature)
                .is_ok()
        } else {
            VerifyingKey::<sha3::Sha3_512>::new(public_key)
                .verify(&tbs, &signature)
                .is_ok()
        })
    } else if sig_oid == rfc5912::SHA_256_WITH_RSA_ENCRYPTION || sig_oid == ID_RSA_ENCRYPTION {
        // RSA PKCS#1 v1.5 over SHA-256. `rsaEncryption` is accepted alongside the explicit
        // sha256WithRSAEncryption OID because RFC 5652 §5.5 allows either in a SignerInfo, with
        // the digest taken from `digestAlgorithm`.
        let public_key = rsa::RsaPublicKey::from_public_key_der(&spki_der).ok()?;
        let signature = Signature::try_from(signer_info.signature.as_bytes()).ok()?;
        Some(
            VerifyingKey::<Sha256>::new(public_key)
                .verify(&tbs, &signature)
                .is_ok(),
        )
    } else {
        // An algorithm we do not implement. Refuse it rather than reinterpreting the signature
        // under a default: a catch-all `else` that verified everything as RSA-PKCS#1-v1.5/SHA-256
        // silently ignored the declared `signatureAlgorithm`, so a signature labelled with a
        // deprecated or mismatched OID was accepted as though it had been labelled correctly.
        Some(false)
    }
}

/// Compute the digest of `message` under the first signer's digest algorithm (SHA-2 or the
/// ISO/TS 32001 SHA-3 family), to compare against the `messageDigest` signed attribute. Defaults
/// to SHA-256 for an unknown OID.
pub(super) fn signer_message_digest(signed_data: &SignedData, message: &[u8]) -> Option<Vec<u8>> {
    let oid = signed_data
        .signer_infos
        .0
        .as_slice()
        .first()?
        .digest_alg
        .oid;
    Some(if oid == ID_SHA512 {
        Sha512::digest(message).to_vec()
    } else if oid == ID_SHA384 {
        Sha384::digest(message).to_vec()
    } else if oid == ID_SHA3_256 {
        sha3::Sha3_256::digest(message).to_vec()
    } else if oid == ID_SHA3_384 {
        sha3::Sha3_384::digest(message).to_vec()
    } else if oid == ID_SHA3_512 {
        sha3::Sha3_512::digest(message).to_vec()
    } else {
        Sha256::digest(message).to_vec()
    })
}

/// The raw signature bytes of the first signer in a CMS `SignedData` (what an RFC 3161 TSA stamps).
pub(super) fn signer_signature(cms_der: &[u8]) -> Option<Vec<u8>> {
    let content_info = ContentInfo::from_der(cms_der).ok()?;
    let signed_data = content_info.content.decode_as::<SignedData>().ok()?;
    Some(
        signed_data
            .signer_infos
            .0
            .as_slice()
            .first()?
            .signature
            .as_bytes()
            .to_vec(),
    )
}

/// The signer's `signingTime` attribute as Unix seconds (UTC), or `None` if absent/unreadable.
pub(super) fn signer_signing_time(signed_data: &SignedData) -> Option<i64> {
    let signer_info = signed_data.signer_infos.0.as_slice().first()?;
    let attr = signer_info
        .signed_attrs
        .as_ref()?
        .iter()
        .find(|attr| attr.oid == ID_SIGNING_TIME)?;
    let value_der = attr.values.as_slice().first()?.to_der().ok()?;
    let time = Time::from_der(&value_der).ok()?;
    i64::try_from(time.to_unix_duration().as_secs()).ok()
}

/// Verify an embedded RFC 3161 timestamp on the first signer and return its `genTime` (Unix seconds)
/// when the token's own CMS verifies and its imprint matches the signature it stamps. `None` if there
/// is no token or it does not check out.
pub(super) fn verify_timestamp(signed_data: &SignedData) -> Option<i64> {
    let signer_info = signed_data.signer_infos.0.as_slice().first()?;
    let token_attr = signer_info
        .unsigned_attrs
        .as_ref()?
        .iter()
        .find(|attr| attr.oid == ID_AA_TIMESTAMP_TOKEN)?;
    let token_der = token_attr.values.as_slice().first()?.to_der().ok()?;

    let token_ci = ContentInfo::from_der(&token_der).ok()?;
    let token_sd = token_ci.content.decode_as::<SignedData>().ok()?;
    let tst_der = token_sd.encap_content_info.econtent.as_ref()?.value();

    // The token's own signature must verify over the TSTInfo it carries.
    if !verify_signer(&token_sd, Sha256::digest(tst_der).as_slice())? {
        return None;
    }
    let tst = TstInfo::from_der(tst_der).ok()?;

    // The imprint must be SHA-256 of the signature this token stamps (RFC 3161 §2.4.2).
    let stamped_signature = signer_info.signature.as_bytes();
    if tst.message_imprint.hashed_message.as_bytes() != Sha256::digest(stamped_signature).as_slice()
    {
        return None;
    }
    i64::try_from(tst.gen_time.to_unix_duration().as_secs()).ok()
}

/// Verify a **document timestamp** (`/DocTimeStamp`, §12.8.5 / RFC 3161): `token_der` is the bare
/// timestamp token (a CMS `SignedData` over `TSTInfo`) from the signature's `/Contents`, and
/// `message` is the bytes the `/ByteRange` covers. Valid when (a) the token's own CMS signature
/// verifies over its `TSTInfo` and (b) the token's `messageImprint` equals `SHA-256(message)`
/// (RFC 3161 §2.4.2). The verified time is the token's `genTime`. Total and panic-free.
#[must_use]
pub fn verify_timestamp_token(
    token_der: &[u8],
    message: &[u8],
    opts: &VerifyOptions,
) -> VerifiedSignature {
    verify_timestamp_token_inner(token_der, message, opts).unwrap_or_default()
}

pub(super) fn verify_timestamp_token_inner(
    token_der: &[u8],
    message: &[u8],
    opts: &VerifyOptions,
) -> Option<VerifiedSignature> {
    let token_ci = ContentInfo::from_der(token_der).ok()?;
    let token_sd = token_ci.content.decode_as::<SignedData>().ok()?;
    let tst_der = token_sd.encap_content_info.econtent.as_ref()?.value();

    // (a) the TSA's signature over the TSTInfo must verify.
    let sig_ok = verify_signer(&token_sd, Sha256::digest(tst_der).as_slice())?;
    // (b) the imprint must be SHA-256 of the /ByteRange bytes (what this token stamps).
    let tst = TstInfo::from_der(tst_der).ok()?;
    let imprint_ok =
        tst.message_imprint.hashed_message.as_bytes() == Sha256::digest(message).as_slice();

    let cert = embedded_leaf(&token_sd)?;
    let gen_time = i64::try_from(tst.gen_time.to_unix_duration().as_secs()).ok();
    // The TSA chain is validated like a signer chain; with revocation material (PAdES-LTA
    // context) its links are checked too.
    let mut revocation = None;
    let trusted = if opts.roots.is_empty() {
        None
    } else {
        let roots: Vec<Certificate> = opts
            .roots
            .iter()
            .filter_map(|der| Certificate::from_der(der).ok())
            .collect();
        // `gen_time` comes from a TSTInfo whose TSA signature was verified just above, so unlike a
        // signer-asserted `signingTime` it is not the subject's own claim.
        let at = gen_time
            .and_then(|t| u64::try_from(t).ok())
            .unwrap_or_else(now_secs);
        let chain = build_chain(cert, &embedded_certs(&token_sd), &roots, at);
        if let (Some(chain), Some(data)) = (&chain, &opts.revocation) {
            revocation = Some(crate::chain_revocation(chain, data, at));
        }
        Some(chain.is_some())
    };
    Some(VerifiedSignature {
        valid: sig_ok && imprint_ok,
        signer: Some(cert.tbs_certificate.subject.to_string()),
        signing_time: None,
        trusted,
        timestamp_time: gen_time,
        pades: false,
        revocation,
        signer_count: signer_count(&token_sd),
    })
}

/// The instant at which certificate validity and revocation are evaluated.
///
/// A **verified** RFC 3161 timestamp wins: its `genTime` is asserted by a third party whose
/// signature over it has been checked. The `signingTime` signed attribute is only the signer's own
/// claim — signed, but signed by the very party whose certificate is in question — so a holder of
/// an expired certificate could otherwise assert a time inside its validity window and have the
/// chain accepted. It is reported to the caller either way, but it does not anchor validation.
/// With neither available, fall back to the wall clock.
pub(super) fn validation_instant(timestamp_time: Option<i64>, signing_time: Option<i64>) -> u64 {
    timestamp_time
        .or(signing_time)
        .and_then(|t| u64::try_from(t).ok())
        .unwrap_or_else(now_secs)
}

/// The certificate of the first `SignerInfo`, resolved through its `sid` (RFC 5652 §5.3).
///
/// The signer is identified by `issuerAndSerialNumber` or `subjectKeyIdentifier` — **not** by
/// position. Taking `certificates[0]` instead, as this once did, is wrong in both directions: a
/// perfectly valid CMS whose certificate set happens to be ordered differently fails to verify,
/// and the subject name reported back to the caller is not necessarily the certificate that
/// produced the signature. `SET OF` has no guaranteed order, so position carries no meaning.
///
/// Falls back to the sole embedded certificate when `sid` matches nothing — the common
/// single-certificate case, where there is no ambiguity to resolve.
pub(super) fn embedded_leaf(signed_data: &SignedData) -> Option<&Certificate> {
    let certs = embedded_certs(signed_data);
    let Some(signer_info) = signed_data.signer_infos.0.as_slice().first() else {
        return certs.into_iter().next();
    };
    let matched = certs.iter().copied().find(|cert| match &signer_info.sid {
        SignerIdentifier::IssuerAndSerialNumber(ias) => {
            cert.tbs_certificate.issuer == ias.issuer
                && cert.tbs_certificate.serial_number == ias.serial_number
        }
        SignerIdentifier::SubjectKeyIdentifier(ski) => cert
            .tbs_certificate
            .get::<SubjectKeyIdentifier>()
            .ok()
            .flatten()
            .is_some_and(|(_critical, found)| found.0.as_bytes() == ski.0.as_bytes()),
    });
    match matched {
        Some(cert) => Some(cert),
        // Exactly one certificate: `sid` has nothing to disambiguate, so accept it.
        None if certs.len() == 1 => certs.into_iter().next(),
        None => None,
    }
}

/// How many `SignerInfo` structures the CMS carries. Only the first is verified; a caller is told
/// the rest exist so it does not read a single `valid` as covering every signature present.
pub(super) fn signer_count(signed_data: &SignedData) -> usize {
    signed_data.signer_infos.0.as_slice().len()
}

/// All embedded certificates (leaf first, in encoded order).
pub(super) fn embedded_certs(signed_data: &SignedData) -> Vec<&Certificate> {
    signed_data
        .certificates
        .as_ref()
        .map(|set| {
            set.0
                .as_slice()
                .iter()
                .filter_map(|choice| match choice {
                    CertificateChoices::Certificate(cert) => Some(cert),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RFC 5280 path building: each link's signature is verified by its issuer, every certificate is
/// within its validity window at `at_secs`, every issuer is a CA authorised to sign certificates
/// (see [`is_ca`]) and within its `pathLenConstraint`, and the path terminates at a certificate
/// that is byte-for-byte one of `roots`. Bounded depth. Walks from `leaf` towards a trust anchor,
/// returning the `(certificate, issuer)` pairs along the way — the anchor appears only as an
/// issuer, never as a checked certificate (a trust anchor is axiomatic). `None` = no trusted path.
/// The pairs are what PAdES-LT revocation checking consumes.
pub(crate) fn build_chain(
    leaf: &Certificate,
    intermediates: &[&Certificate],
    roots: &[Certificate],
    at_secs: u64,
) -> Option<Vec<(Certificate, Certificate)>> {
    let mut chain: Vec<(Certificate, Certificate)> = Vec::new();
    let mut current = leaf;
    for _ in 0..MAX_CHAIN_DEPTH {
        if !valid_at(current, at_secs) {
            return None;
        }
        // A certificate that is itself a trust anchor ends the path successfully.
        if roots.iter().any(|root| der_eq(root, current)) {
            return Some(chain);
        }
        // Find the issuer that signed `current`: a trusted root first, then an embedded
        // intermediate. `issued` requires the candidate to be a CA, so an ordinary end-entity
        // certificate cannot be used to mint subordinates (RFC 5280 §4.2.1.9).
        let issuer = roots
            .iter()
            .find(|cand| issued(current, cand))
            .or_else(|| {
                intermediates
                    .iter()
                    .copied()
                    .find(|cand| issued(current, cand))
            })?;

        // §4.2.1.9: pathLenConstraint caps how many *non-self-issued intermediates* may follow the
        // constrained CA in the path. `chain` already holds the links below this one; the leaf
        // itself is not an intermediate, hence the saturating subtraction.
        if let Some(limit) = path_len_constraint(issuer)
            && chain.len().saturating_sub(1) > usize::from(limit)
        {
            return None;
        }

        // Signed directly by a trust anchor (the common case) — the anchor's own validity is still
        // checked, but nothing above it is.
        if roots.iter().any(|root| der_eq(root, issuer)) {
            if !valid_at(issuer, at_secs) {
                return None;
            }
            chain.push((current.clone(), issuer.clone()));
            return Some(chain);
        }
        chain.push((current.clone(), issuer.clone()));
        current = issuer;
    }
    None
}

/// Whether `issuer` issued `child`: `issuer` is a CA permitted to sign certificates, the names
/// match, and its key verifies the child's signature. Shared with the revocation module (a
/// delegated OCSP responder must be issuer-certified).
pub(crate) fn issued(child: &Certificate, issuer: &Certificate) -> bool {
    certified_by(child, issuer) && is_ca(issuer)
}

/// Whether `issuer` signed `child`: the names line up and `issuer`'s key verifies the signature —
/// without asking whether `issuer` is a CA.
///
/// This is [`issued`] minus the CA requirement, and it exists for exactly one caller: a delegated
/// OCSP responder (RFC 6960 §4.2.2.2) is certified *by* the CA but is itself an end-entity
/// certificate, so demanding `cA=TRUE` of it would reject every legitimate delegated responder.
/// Its authority comes from [`is_ocsp_signer`] instead. Do not use this for path building.
pub(crate) fn certified_by(child: &Certificate, issuer: &Certificate) -> bool {
    child.tbs_certificate.issuer == issuer.tbs_certificate.subject && cert_signed_by(child, issuer)
}

/// Whether `cert` may act as a certificate issuer (RFC 5280 §4.2.1.9, §4.2.1.3): `basicConstraints`
/// must be present with `cA=TRUE`, and a `keyUsage` extension, if present, must grant
/// `keyCertSign`.
///
/// The `basicConstraints` requirement is deliberately strict. Accepting a certificate that lacks it
/// — or that carries `cA=FALSE` — is the classic basic-constraints bypass: any end-entity
/// certificate a trusted CA has issued (a routine TLS or S/MIME certificate) could otherwise be
/// used to mint a subordinate for **any** subject, and that subordinate would chain to the trusted
/// root. A self-signed certificate in the caller's trust store is unaffected: `build_chain` matches
/// trust anchors by DER equality before it ever asks whether they are CAs.
pub(crate) fn is_ca(cert: &Certificate) -> bool {
    let Ok(Some((_critical, basic))) = cert.tbs_certificate.get::<BasicConstraints>() else {
        return false;
    };
    if !basic.ca {
        return false;
    }
    // keyUsage is optional; when present it is authoritative.
    match cert.tbs_certificate.get::<KeyUsage>() {
        Ok(Some((_critical, usage))) => usage.key_cert_sign(),
        Ok(None) => true,
        Err(_) => false,
    }
}

/// The `pathLenConstraint` of `cert`'s `basicConstraints`, when it sets one (RFC 5280 §4.2.1.9).
fn path_len_constraint(cert: &Certificate) -> Option<u8> {
    cert.tbs_certificate
        .get::<BasicConstraints>()
        .ok()
        .flatten()
        .and_then(|(_critical, basic)| basic.path_len_constraint)
}

/// Whether `cert` carries the `id-kp-OCSPSigning` extended key usage — what RFC 6960 §4.2.2.2
/// requires of a **delegated** OCSP responder, i.e. one that is not the CA itself.
pub(crate) fn is_ocsp_signer(cert: &Certificate) -> bool {
    matches!(
        cert.tbs_certificate.get::<ExtendedKeyUsage>(),
        Ok(Some((_critical, eku))) if eku.0.contains(&ID_KP_OCSP_SIGNING)
    )
}

/// Whether `issuer`'s RSA public key verifies `child`'s certificate signature.
pub(super) fn cert_signed_by(child: &Certificate, issuer: &Certificate) -> bool {
    let Ok(tbs) = child.tbs_certificate.to_der() else {
        return false;
    };
    rsa_verifies(
        &tbs,
        &child.signature_algorithm.oid,
        child.signature.raw_bytes(),
        issuer,
    )
}

/// Whether `signer`'s RSA public key verifies `signature` over `tbs` with the `alg` signature
/// algorithm (RSA PKCS#1 v1.5 with SHA-256/384/512 — the algorithms our signers use; anything
/// else yields `false`). Shared by certificate, CRL and OCSP signature checks.
pub(crate) fn rsa_verifies(
    tbs: &[u8],
    alg: &der::asn1::ObjectIdentifier,
    signature: &[u8],
    signer: &Certificate,
) -> bool {
    let Ok(spki) = signer.tbs_certificate.subject_public_key_info.to_der() else {
        return false;
    };
    let Ok(public_key) = rsa::RsaPublicKey::from_public_key_der(&spki) else {
        return false;
    };
    let Ok(signature) = Signature::try_from(signature) else {
        return false;
    };
    match *alg {
        rfc5912::SHA_256_WITH_RSA_ENCRYPTION => VerifyingKey::<Sha256>::new(public_key)
            .verify(tbs, &signature)
            .is_ok(),
        rfc5912::SHA_384_WITH_RSA_ENCRYPTION => VerifyingKey::<Sha384>::new(public_key)
            .verify(tbs, &signature)
            .is_ok(),
        rfc5912::SHA_512_WITH_RSA_ENCRYPTION => VerifyingKey::<Sha512>::new(public_key)
            .verify(tbs, &signature)
            .is_ok(),
        _ => false,
    }
}

/// Whether `cert` is within its validity window at `at_secs` (§7.6 / RFC 5280 §4.1.2.5).
pub(super) fn valid_at(cert: &Certificate, at_secs: u64) -> bool {
    let not_before = cert
        .tbs_certificate
        .validity
        .not_before
        .to_unix_duration()
        .as_secs();
    let not_after = cert
        .tbs_certificate
        .validity
        .not_after
        .to_unix_duration()
        .as_secs();
    (not_before..=not_after).contains(&at_secs)
}

/// Byte-for-byte certificate equality (DER), used to recognise a trust anchor.
pub(super) fn der_eq(a: &Certificate, b: &Certificate) -> bool {
    match (a.to_der(), b.to_der()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Format `unix_secs` as a PDF date string `D:YYYYMMDDHHmmSSZ` (§7.9.4), in UTC. Used for the
/// signature dictionary's `/M` entry so it agrees with the CMS `signingTime` attribute.
#[must_use]
pub fn pdf_date(unix_secs: u64) -> String {
    match der::DateTime::from_unix_duration(Duration::from_secs(unix_secs)) {
        Ok(dt) => format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}Z",
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minutes(),
            dt.seconds()
        ),
        Err(_) => "D:19700101000000Z".to_string(),
    }
}

/// The current time as Unix seconds, or 0 if the clock is unavailable.
pub(super) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Build the `signingTime` signed attribute for `secs` (UTCTime for 1950–2049, else GeneralizedTime,
/// RFC 5652 §11.3).
pub(super) fn signing_time_attribute(secs: u64) -> Option<Attribute> {
    let datetime = der::DateTime::from_unix_duration(Duration::from_secs(secs)).ok()?;
    let value = if (1950..=2049).contains(&datetime.year()) {
        Any::encode_from(&UtcTime::from_date_time(datetime).ok()?).ok()?
    } else {
        Any::encode_from(&GeneralizedTime::from_date_time(datetime)).ok()?
    };
    attribute(ID_SIGNING_TIME, value)
}

/// The `signing-certificate-v2` signed attribute (RFC 5035) binding `cert_der` by its SHA-256 hash —
/// the PAdES-B certificate reference.
pub(super) fn signing_cert_v2_attribute(cert_der: &[u8]) -> Option<Attribute> {
    let cert_hash = Sha256::digest(cert_der);
    let scv2 = SigningCertificateV2 {
        certs: vec![EssCertIdV2 {
            cert_hash: OctetString::new(cert_hash.to_vec()).ok()?,
        }],
    };
    attribute(
        ID_AA_SIGNING_CERTIFICATE_V2,
        Any::from_der(&scv2.to_der().ok()?).ok()?,
    )
}

/// A single-valued CMS attribute.
pub(super) fn attribute(oid: ObjectIdentifier, value: Any) -> Option<Attribute> {
    let mut values = SetOfVec::new();
    values.insert(value).ok()?;
    Some(Attribute { oid, values })
}

/// The `AlgorithmIdentifier` for SHA-256 with absent parameters.
pub(super) fn sha256_alg() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: ID_SHA256,
        parameters: None,
    }
}

/// The `AlgorithmIdentifier` for SHA-384 with absent parameters (ECDSA P-384's digest algorithm).
pub(super) fn sha384_alg() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: ID_SHA384,
        parameters: None,
    }
}

/// The `AlgorithmIdentifier` for SHA-512 with absent parameters (Ed25519's digest algorithm).
pub(super) fn sha512_alg() -> AlgorithmIdentifierOwned {
    AlgorithmIdentifierOwned {
        oid: ID_SHA512,
        parameters: None,
    }
}
