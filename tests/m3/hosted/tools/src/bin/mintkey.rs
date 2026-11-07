//! The test install's stand-in for `graftyard mint-key rotate`'s key half: draws a signing key and a wrap key from
//! `/dev/urandom`, writes the `MintWrapKey` secret's text to `<out>` (mode 0600), and prints the `PutVerifyingKey`
//! command for `TokenShard(0)` with the public half. No secret reaches stdout or argv.
//!
//!   mintkey <kid> <out file>

use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use graftyard_identity::stateless::SigningSecret;
use graftyard_identity::{Kid, TokensCmd, TokensOp};
use graftyard_types::CredentialShard;

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("urandom");
    b
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [kid, out] = a.as_slice() else { panic!("usage: mintkey <kid> <out file>") };
    let kid = Kid(kid.parse().expect("kid"));
    let seed = SigningSecret(random::<32>());
    let secret = graftyard_ops::wrap(random::<32>(), kid, random::<12>(), &seed);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(out).expect("a new out file");
    f.write_all(serde_json::to_string(&secret).expect("json").as_bytes()).expect("write");
    let cmd = TokensCmd {
        cred: CredentialShard::NONE,
        op: TokensOp::PutVerifyingKey { kid, key: seed.public(), retired: None },
    };
    println!("{}", serde_json::to_string(&cmd).expect("json"));
}
