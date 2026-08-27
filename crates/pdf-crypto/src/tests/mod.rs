//! Unit tests for the standard security handler (ISO 32000-1 §7.6.4).
//!
//! Split into themed submodules; this module holds the fixtures and helpers they share.
//! Submodules reach crate internals (private fields, `Method`, `string_bytes`, the cipher
//! helpers) via `use crate::*;` and the fixtures here via `use super::*;`.

use super::*;

/// The `/Encrypt` dictionary from the reference generator (RC4, V2/R3, 128-bit).
fn rc4_encrypt_dict() -> Dictionary {
    let mut d = Dictionary::new();
    d.insert(Name::from("Filter"), Object::Name(Name::from("Standard")));
    d.insert(Name::from("V"), Object::Integer(2));
    d.insert(Name::from("R"), Object::Integer(3));
    d.insert(Name::from("Length"), Object::Integer(128));
    d.insert(Name::from("P"), Object::Integer(-44));
    let owner = hex("36451bd39d753b7c1d10922c28e6665aa4f3353fb0348b536893e3b1db5c579b");
    d.insert(Name::from("O"), Object::String(PdfString::from(owner)));
    // /U for the empty user password with this /O and reference_id (Algorithm 5).
    let user = hex("9ac5808bd5d95e6fdd1d2b55f1040d8600000000000000000000000000000000");
    d.insert(Name::from("U"), Object::String(PdfString::from(user)));
    d
}

fn reference_id() -> Vec<u8> {
    hex("0123456789abcdef0123456789abcdef")
}

/// The `/Encrypt` dictionary from the AES-256/R6 reference generator (V5/R6, `AESV3`).
fn aes256_encrypt_dict() -> Dictionary {
    let mut d = Dictionary::new();
    d.insert(Name::from("Filter"), Object::Name(Name::from("Standard")));
    d.insert(Name::from("V"), Object::Integer(5));
    d.insert(Name::from("R"), Object::Integer(6));
    d.insert(Name::from("Length"), Object::Integer(256));
    d.insert(Name::from("P"), Object::Integer(-44));
    let u = hex(
        "32cd1740f398f4b820b63b53a21df1540eed17327042270750620ebc8a8346df\
         11111111111111112222222222222222",
    );
    let ue = hex("600702014d8b5bcd69f1a4f4664f0a3c14dbec2b087d35443cdbd63ca6c6189e");
    d.insert(Name::from("U"), Object::String(PdfString::from(u)));
    d.insert(Name::from("UE"), Object::String(PdfString::from(ue)));
    let mut std_cf = Dictionary::new();
    std_cf.insert(Name::from("CFM"), Object::Name(Name::from("AESV3")));
    let mut cf = Dictionary::new();
    cf.insert(Name::from("StdCF"), Object::Dictionary(std_cf));
    d.insert(Name::from("CF"), Object::Dictionary(cf));
    d.insert(Name::from("StmF"), Object::Name(Name::from("StdCF")));
    d.insert(Name::from("StrF"), Object::Name(Name::from("StdCF")));
    d
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

mod aes256;
mod handler;
mod permissions;
mod rc4;
