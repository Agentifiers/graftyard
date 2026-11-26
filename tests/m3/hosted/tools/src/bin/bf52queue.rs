//! BF-52's revocation half over SSH (B0-24a's row, `tests/m6/results/m6-wp2.md`): `n` format-32.26 pushes under one SSH
//! key, each with its own request id and its own `refs/heads/<tag>/q<i>`, for one `/commit_many` to the repository's
//! leader. The key's fingerprint is graft-sshd's, SHA-256 of the public key's wire blob (containers/ssh/server/main.go,
//! `authenticate`), and each push is encoded with `graftyard_git`'s own encoder (`Push::to_wire`), as the edge's SSH
//! path sends it (`workers/edge/src/ssh.rs`, `principal`). Prints `{"fingerprint", "cmds": [{"rid", "cmd"}, ..]}`.
//!
//!   bf52queue <account id> <public key file> <n> <tag>

use base64::Engine;
use graftyard_git::refs::{Atomicity, ObjectId, RefName, RefUpdate};
use graftyard_git::repo::{Push, REPO_FORMAT_SSH};
use graftyard_identity::{Actor, Credential, Fingerprint, Principal, ScopeSet, UserId};
use graftyard_raft::Command;
use graftyard_types::{AccountId, DbId, RequestId, Timestamp, Ulid};
use sha2::{Digest, Sha256};
use std::io::Read;

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).expect("urandom");
    b
}

/// graft-sshd's fingerprint of an OpenSSH public key line: SHA-256 of its base64 blob, decoded.
fn fingerprint(line: &str) -> Fingerprint {
    let blob = line.split_whitespace().nth(1).expect("a public key line: <type> <base64> [comment]");
    let blob = base64::engine::general_purpose::STANDARD.decode(blob).expect("the key's base64");
    Fingerprint(Sha256::digest(blob).into())
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [account, key, n, tag] = a.as_slice() else {
        panic!("usage: bf52queue <account id> <public key file> <n> <tag>")
    };
    let account = AccountId(DbId(account.parse().expect("account id")));
    let fp = fingerprint(&std::fs::read_to_string(key).expect("the public key file"));
    let n: u32 = n.parse().expect("n");
    let now = Timestamp(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as u64
    );
    // The edge's SSH principal: the key's owner, scope 1, the key's credential.
    let actor = Principal {
        actor: Actor::User(UserId(account)),
        scopes: ScopeSet(1),
        credential: Credential::Ssh(fp),
        sso_orgs: smallvec::SmallVec::new(),
    };
    let cmds: Vec<serde_json::Value> = (0..n)
        .map(|i| {
            let request = RequestId(Ulid::new(now, random()));
            let mut oid = random::<20>();
            oid[12..16].copy_from_slice(&i.to_be_bytes());
            let push = Push {
                updates: vec![RefUpdate {
                    name: RefName::parse(format!("refs/heads/{tag}/q{i}").as_bytes()).expect("a ref name"),
                    expect: None,
                    new: Some(ObjectId(oid)),
                }],
                atomicity: Atomicity::PerRef,
                pack: None,
                actor: actor.clone(),
                options: Vec::new(),
                request,
                agent: Some("git/bf52queue".into()),
                provenance: Vec::new(),
                protected: Vec::new(),
                changed: Vec::new(),
                leases: false,
                test_format: false,
            };
            assert_eq!(push.format(), REPO_FORMAT_SSH);
            assert_eq!(Push::from_wire(&push.to_wire()).as_ref(), Ok(&push), "the push round-trips");
            serde_json::json!({ "rid": request.to_string(), "cmd": push })
        })
        .collect();
    let hex: String = fp.0.iter().map(|b| format!("{b:02x}")).collect();
    println!("{}", serde_json::json!({ "fingerprint": hex, "cmds": cmds }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fingerprint graft-sshd sends for a fixed ed25519 key: SHA-256 of the blob, computed apart with
    /// `ssh-keygen -lf` (`SHA256:` base64 of the same 32 bytes, unpadded).
    #[test]
    fn fingerprint_is_sha256_of_the_key_blob() {
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPBYRkq+X/iO1fCyjKjM+Zegjr/ZtqUxn3FtQZArp0BT bf52";
        let want = "SHA256:pvmYTQJjKFgGZ3Mq9NiJqjyFwa++ON78Vta77gqGBSQ";
        let got = format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(fingerprint(line).0));
        assert_eq!(got, want);
    }
}
