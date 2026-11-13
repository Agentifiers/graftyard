//! Password hashes for the web sign-in (`design/SHARED.md`, "Frontend and native app", sign-in and the session
//! cookie): PBKDF2-HMAC-SHA256 as `$pbkdf2-sha256$<iterations>$<salt b64>$<hash b64>`, the string the `Identity`
//! fold's `Password` event holds. The edge hashes and checks; `decide` only stores the string.

use crate::sealed::hmac_sha256;

/// Iterations for a new hash [guess: OWASP's 2023 figure is 600,000, but the edge's CPU per sign-in is billed, and the
/// hash never leaves the residency domain's `Identity` chain].
pub const PASSWORD_ITERATIONS: u32 = 100_000;
/// The shortest password `SetPassword` accepts at the edge [guess, GitHub's minimum].
pub const PASSWORD_MIN: usize = 8;

const PREFIX: &str = "$pbkdf2-sha256$";

/// PBKDF2-HMAC-SHA256 (RFC 8018) with one 32-byte block, which is all a 32-byte hash needs.
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut first = salt.to_vec();
    first.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac_sha256(password, &first);
    let mut out = u;
    for _ in 1..iterations {
        u = hmac_sha256(password, &u);
        for (o, x) in out.iter_mut().zip(u) {
            *o ^= x;
        }
    }
    out
}

/// The stored string for `password` under `salt`.
pub fn hash_password(password: &str, salt: &[u8; 16]) -> String {
    let h = pbkdf2_sha256(password.as_bytes(), salt, PASSWORD_ITERATIONS);
    format!("{PREFIX}{PASSWORD_ITERATIONS}${}${}", b64(salt), b64(&h))
}

/// Whether `password` matches `stored`; a string in any other form never matches. The comparison runs over every
/// byte, so its time does not say where a wrong guess first differs.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Some(rest) = stored.strip_prefix(PREFIX) else { return false };
    let mut parts = rest.split('$');
    let (Some(n), Some(salt), Some(want), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(n), Some(salt), Some(want)) = (n.parse::<u32>(), unb64(salt), unb64(want)) else { return false };
    if n == 0 || want.len() != 32 {
        return false;
    }
    let got = pbkdf2_sha256(password.as_bytes(), &salt, n);
    got.iter().zip(&want).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 without padding.
fn b64(b: &[u8]) -> String {
    let mut s = String::new();
    for c in b.chunks(3) {
        let n = c.iter().enumerate().fold(0u32, |n, (i, x)| n | (u32::from(*x) << (16 - 8 * i)));
        for i in 0..=c.len() {
            s.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
    }
    s
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for ch in s.bytes() {
        acc = (acc << 6) | B64.iter().position(|b| *b == ch)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7914, section 11's PBKDF2-HMAC-SHA256 vectors (first 32 bytes of each `dkLen = 64` output).
    #[test]
    fn matches_the_rfc_7914_vectors() {
        let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(pbkdf2_sha256(b"passwd", b"salt", 1)),
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
        );
        assert_eq!(
            hex(pbkdf2_sha256(b"Password", b"NaCl", 80_000)),
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"
        );
    }

    /// A hash verifies its own password and refuses a one-character change, another salt's hash, and a string that
    /// is not this form.
    #[test]
    fn a_hash_verifies_only_its_own_password() {
        let h = hash_password("correct horse", &[7; 16]);
        assert!(h.starts_with("$pbkdf2-sha256$100000$"));
        assert!(verify_password("correct horse", &h));
        assert!(!verify_password("correct hors", &h));
        assert!(!verify_password("correct horse", &hash_password("x", &[7; 16])));
        assert!(!verify_password("correct horse", "$argon2id$v=19$secret-hash"));
        assert!(!verify_password("correct horse", &h.replace("100000", "0")));
    }
}
