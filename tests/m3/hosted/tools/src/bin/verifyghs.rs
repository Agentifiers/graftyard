//! The edge's `ghs_` verification (`graftyard_identity::stateless::verify`) over the hosted `TokenShard(0)` fold, for
//! kid 1's retire. The fold is its `dep` read's hex, from `<fold file>`; each token is the content of a file (mode
//! 0600), never argv. Prints each token's `kid` and `verify`'s result at each time given (ms), and no token text.
//!
//!   verifyghs <fold hex file> <at ms,...> <token file>...

use graftyard_identity::stateless::{parse, verify};
use graftyard_identity::{Kid, TokensFold, TokensRead};
use graftyard_types::Timestamp;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [fold, ats, tokens @ ..] = a.as_slice() else {
        panic!("usage: verifyghs <fold hex file> <at,...> <token file>...")
    };
    let hex = std::fs::read_to_string(fold).expect("fold file");
    let hex = hex.trim();
    let bytes: Vec<u8> =
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex")).collect();
    let f: TokensFold = graftyard_raft::codec::from_bytes(&bytes).expect("a Tokens fold");
    let keys: Vec<_> = [1, 2]
        .iter()
        .map(|k| serde_json::json!({ "kid": k, "retired": f.verifying_key(Kid(*k)).and_then(|r| r.retired).map(|t| t.0) }))
        .collect();
    println!("{}", serde_json::json!({ "keys": keys }));
    for path in tokens {
        let text = std::fs::read_to_string(path).expect("token file");
        let text = text.trim();
        let kid = parse(text).map(|t| t.kid().0).ok();
        for at in ats.split(',') {
            let at = Timestamp(at.parse().expect("ms"));
            let r = match verify(text, &f, at) {
                Ok(c) => format!("ok (kid {}, jti {})", c.kid.0, c.jti.0),
                Err(e) => format!("{e:?}"),
            };
            println!(
                "{}",
                serde_json::json!({ "token": path.rsplit('/').next(), "kid": kid, "at": at.0, "verify": r })
            );
        }
    }
}
