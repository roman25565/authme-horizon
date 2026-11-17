//! Password hashes.
//!
//! New passwords are stored as scrypt (memory-hard, in the same class as argon2 and bcrypt;
//! argon2 and bcrypt crates are not in Horizon's Cargo.lock, which is the allow list for a
//! native mod's dependencies). The default cost, N = 2^15, r = 8, p = 3 (32 MiB), is one of
//! OWASP's recommended scrypt settings.
//!
//! Format: `$scrypt$ln=15,r=8,p=3$<salt, base64>$<hash, base64>` (no padding).
//!
//! Imported AuthMe accounts may still carry AuthMe's `SHA256` format,
//! `$SHA$<salt>$<sha256(sha256(password) + salt)>`; they are verified once and rehashed with
//! scrypt on the next successful login (`Verified::needs_rehash`).

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD as B64};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// The scrypt cost of new hashes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cost {
    pub log_n: u8,
    pub r: u32,
    pub p: u32,
}

impl Default for Cost {
    fn default() -> Self {
        Self { log_n: 15, r: 8, p: 3 }
    }
}

impl Cost {
    /// Clamped to what is safe and what will not stall the server: N from 2^14 to 2^20,
    /// r from 8 to 16, p from 1 to 8.
    pub fn clamped(self) -> Self {
        Self {
            log_n: self.log_n.clamp(14, 20),
            r: self.r.clamp(8, 16),
            p: self.p.clamp(1, 8),
        }
    }
}

const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;

/// What verifying a password against a stored hash found.
#[derive(Debug, PartialEq, Eq)]
pub struct Verified {
    pub ok: bool,
    /// The stored hash is a legacy format or an older cost: store a new one.
    pub needs_rehash: bool,
}

/// A new scrypt hash of `password` with a fresh random salt.
pub fn hash(password: &str, cost: Cost) -> Result<String, String> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).map_err(|e| format!("no random salt: {e}"))?;
    hash_with_salt(password, &salt, cost)
}

fn hash_with_salt(password: &str, salt: &[u8], cost: Cost) -> Result<String, String> {
    let cost = cost.clamped();
    let out = derive(password.as_bytes(), salt, cost, HASH_LEN)?;
    Ok(format!(
        "$scrypt$ln={},r={},p={}${}${}",
        cost.log_n,
        cost.r,
        cost.p,
        B64.encode(salt),
        B64.encode(out)
    ))
}

fn derive(password: &[u8], salt: &[u8], cost: Cost, len: usize) -> Result<Vec<u8>, String> {
    let params = scrypt::Params::new(cost.log_n, cost.r, cost.p, len)
        .map_err(|e| format!("scrypt params: {e}"))?;
    let mut out = vec![0u8; len];
    scrypt::scrypt(password, salt, &params, &mut out).map_err(|e| format!("scrypt: {e}"))?;
    Ok(out)
}

/// Verifies `password` against `stored` (scrypt or AuthMe's SHA256). An unknown or broken
/// format never verifies.
pub fn verify(password: &str, stored: &str, cost: Cost) -> Verified {
    let no = Verified { ok: false, needs_rehash: false };
    if let Some(rest) = stored.strip_prefix("$scrypt$") {
        let parts: Vec<&str> = rest.split('$').collect();
        let [params, salt, expected] = parts[..] else {
            return no;
        };
        let Some(found) = parse_params(params) else {
            return no;
        };
        let (Ok(salt), Ok(expected)) = (B64.decode(salt), B64.decode(expected)) else {
            return no;
        };
        if expected.is_empty() || expected.len() > 64 || salt.is_empty() {
            return no;
        }
        let Ok(actual) = derive(password.as_bytes(), &salt, found, expected.len()) else {
            return no;
        };
        let ok: bool = actual.ct_eq(&expected).into();
        return Verified {
            ok,
            needs_rehash: ok && found != cost.clamped(),
        };
    }
    if let Some(rest) = stored.strip_prefix("$SHA$") {
        // AuthMe Sha256: $SHA$salt$sha256(sha256(password) + salt), lowercase hex.
        let Some((salt, expected)) = rest.split_once('$') else {
            return no;
        };
        let inner = hex::encode(Sha256::digest(password.as_bytes()));
        let outer = hex::encode(Sha256::digest(format!("{inner}{salt}").as_bytes()));
        let ok: bool = outer
            .as_bytes()
            .ct_eq(expected.to_ascii_lowercase().as_bytes())
            .into();
        return Verified { ok, needs_rehash: ok };
    }
    no
}

fn parse_params(s: &str) -> Option<Cost> {
    let mut cost = Cost { log_n: 0, r: 0, p: 0 };
    for part in s.split(',') {
        let (k, v) = part.split_once('=')?;
        match k {
            "ln" => cost.log_n = v.parse().ok()?,
            "r" => cost.r = v.parse().ok()?,
            "p" => cost.p = v.parse().ok()?,
            _ => return None,
        }
    }
    // Refuse costs a forged file could use to exhaust memory or time.
    (cost.log_n >= 10 && cost.log_n <= 20 && cost.r >= 1 && cost.r <= 32 && cost.p >= 1 && cost.p <= 16)
        .then_some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cheap() -> Cost {
        Cost { log_n: 14, r: 8, p: 1 }
    }

    #[test]
    fn scrypt_round_trip() {
        let h = hash("correct horse", cheap()).unwrap();
        assert!(h.starts_with("$scrypt$ln=14,r=8,p=1$"));
        assert_eq!(verify("correct horse", &h, cheap()), Verified { ok: true, needs_rehash: false });
        assert!(!verify("wrong horse", &h, cheap()).ok);
        assert!(!verify("", &h, cheap()).ok);
    }

    #[test]
    fn salts_differ() {
        assert_ne!(hash("same", cheap()).unwrap(), hash("same", cheap()).unwrap());
    }

    #[test]
    fn known_scrypt_vector() {
        // RFC 7914 test vector 2: P="password", S="NaCl", N=1024, r=8, p=16, 64 bytes.
        let out = derive(b"password", b"NaCl", Cost { log_n: 10, r: 8, p: 16 }, 64).unwrap();
        assert_eq!(
            hex::encode(out),
            "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b373162\
             2eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640"
        );
    }

    #[test]
    fn cost_change_asks_for_rehash() {
        let h = hash("pw-123456", cheap()).unwrap();
        let v = verify("pw-123456", &h, Cost::default());
        assert!(v.ok && v.needs_rehash);
    }

    #[test]
    fn authme_sha256_legacy() {
        // Built the way AuthMe's Sha256 does.
        let salt = "1234567890abcdef";
        let inner = hex::encode(Sha256::digest(b"secret"));
        let outer = hex::encode(Sha256::digest(format!("{inner}{salt}").as_bytes()));
        let stored = format!("$SHA${salt}${outer}");
        assert_eq!(verify("secret", &stored, cheap()), Verified { ok: true, needs_rehash: true });
        assert!(!verify("Secret", &stored, cheap()).ok);
    }

    #[test]
    fn garbage_never_verifies() {
        for s in ["", "plain", "$scrypt$", "$scrypt$ln=99,r=8,p=1$AA$AA", "$SHA$", "$2y$10$abc"] {
            assert!(!verify("x", s, cheap()).ok, "{s}");
        }
    }
}
