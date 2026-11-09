//! Password hashes and second-factor verifiers encode as before but never print through `Debug`.

use graftyard_identity::{CredentialEvent, IdentityEvent, IdentityFold, IdentityOp};

const HASH: &str = "$argon2id$v=19$secret-hash";

/// Postcard bytes of `HASH` as a `Box<str>`: its length, then its bytes.
fn hash_bytes() -> Vec<u8> {
    let mut b = vec![HASH.len() as u8];
    b.extend_from_slice(HASH.as_bytes());
    b
}

#[test]
fn secrets_encode_unchanged_and_debug_redacts_them() {
    let pw = IdentityEvent::Credential(CredentialEvent::Password { hash: Box::<str>::from(HASH).into() });
    let verifier: Box<[u8]> = vec![9u8, 8, 7].into();
    let sf =
        IdentityEvent::Credential(CredentialEvent::SecondFactor { id: 7, verifier: Some(verifier.clone().into()) });
    let op = IdentityOp::SetPassword { hash: Box::<str>::from(HASH).into() };
    let mut f = IdentityFold { password: Some(Box::<str>::from(HASH).into()), ..IdentityFold::default() };
    f.second_factors.insert(7, verifier.into());

    // The bytes these values encoded to before the hash and verifier fields were wrapped.
    let golden: [Vec<u8>; 4] = [
        [vec![1, 2], hash_bytes()].concat(),
        vec![1, 3, 7, 1, 3, 9, 8, 7],
        [vec![3], hash_bytes()].concat(),
        [vec![0, 0, 0, 1], hash_bytes(), vec![1, 7, 3, 9, 8, 7, 0, 0, 0]].concat(),
    ];
    let got = [
        postcard::to_allocvec(&pw).unwrap(),
        postcard::to_allocvec(&sf).unwrap(),
        postcard::to_allocvec(&op).unwrap(),
        postcard::to_allocvec(&f).unwrap(),
    ];
    assert_eq!(got, golden);
    assert_eq!(postcard::from_bytes::<IdentityEvent>(&got[0]).unwrap(), pw);
    assert_eq!(postcard::from_bytes::<IdentityEvent>(&got[1]).unwrap(), sf);
    assert_eq!(postcard::from_bytes::<IdentityFold>(&got[3]).unwrap(), f);
    assert_eq!(
        serde_json::to_string(&f).unwrap(),
        format!(r#"[null,null,{{}},"{HASH}",{{"7":[9,8,7]}},{{}},{{}},[]]"#)
    );

    let printed = format!("{pw:?} {sf:?} {op:?} {f:?}");
    assert!(!printed.contains("secret-hash"), "{printed}");
    assert!(!printed.contains("[9, 8, 7]"), "{printed}");
    assert!(printed.contains("Secret(..)"), "{printed}");
}
