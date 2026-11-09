//! A new repository's first commit, for `auto_init` on `POST /orgs/{org}/repos` and `POST /user/repos`: GitHub's
//! README-only commit, as a version 2 pack of three objects (blob, tree, commit). The edge pushes it as an ordinary
//! `Push` before the repository's `InitRepo`, so the git cache's `index-pack --strict` checks it like any push.

use flate2::Compression;
use flate2::write::ZlibEncoder;
use graftyard_types::Timestamp;
use sha1::{Digest, Sha1};
use std::io::Write;

use crate::refs::ObjectId;

/// The commit and the pack that carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitCommit {
    pub commit: ObjectId,
    pub pack: Vec<u8>,
}

/// Who the commit names as author and committer.
pub struct Signature<'a> {
    pub name: &'a str,
    pub email: &'a str,
    pub at: Timestamp,
}

fn object_id(kind: &str, body: &[u8]) -> ObjectId {
    let mut h = Sha1::new();
    h.update(format!("{kind} {}\0", body.len()).as_bytes());
    h.update(body);
    ObjectId(h.finalize().into())
}

/// A pack entry's header: type in bits 4-6 of the first byte, then the size as a little-endian base-128 varint.
fn entry(out: &mut Vec<u8>, ty: u8, body: &[u8]) {
    let mut size = body.len();
    let mut b = (ty << 4) | (size & 0x0f) as u8;
    size >>= 4;
    while size > 0 {
        out.push(b | 0x80);
        b = (size & 0x7f) as u8;
        size >>= 7;
    }
    out.push(b);
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(body).expect("writes to a Vec");
    out.extend(z.finish().expect("writes to a Vec"));
}

/// GitHub's README for a new repository: its name as a heading, then its description when it has one.
pub fn readme(name: &str, description: Option<&str>) -> String {
    match description.filter(|d| !d.is_empty()) {
        Some(d) => format!("# {name}\n{d}\n"),
        None => format!("# {name}\n"),
    }
}

/// The README commit: one root commit whose tree holds `README.md`, with the message "Initial commit".
pub fn readme_commit(name: &str, description: Option<&str>, who: &Signature<'_>) -> InitCommit {
    let blob = readme(name, description).into_bytes();
    let blob_id = object_id("blob", &blob);
    let mut tree = b"100644 README.md\0".to_vec();
    tree.extend_from_slice(&blob_id.0);
    let tree_id = object_id("tree", &tree);
    let sig = format!("{} <{}> {} +0000", who.name, who.email, who.at.0 / 1000);
    let commit = format!("tree {}\nauthor {sig}\ncommitter {sig}\n\nInitial commit\n", tree_id.hex()).into_bytes();
    let commit_id = object_id("commit", &commit);
    let mut pack = b"PACK".to_vec();
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&3u32.to_be_bytes());
    entry(&mut pack, 1, &commit);
    entry(&mut pack, 2, &tree);
    entry(&mut pack, 3, &blob);
    let trailer: [u8; 20] = Sha1::digest(&pack).into();
    pack.extend_from_slice(&trailer);
    InitCommit { commit: commit_id, pack }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// git itself indexes the pack and reads back the commit, its tree, and the README.
    #[test]
    fn git_reads_the_readme_commit() {
        let who =
            Signature { name: "m3-admin", email: "7100003+m3-admin@users.noreply.h", at: Timestamp(1_793_000_000_000) };
        let c = readme_commit("hello", Some("a test"), &who);
        let dir = std::env::temp_dir().join(format!("graftyard-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let git = |args: &[&str], input: Option<&[u8]>| {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(&dir).args(args);
            cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped());
            let mut p = cmd.spawn().expect("git runs");
            if let Some(i) = input {
                p.stdin.take().unwrap().write_all(i).unwrap();
            }
            let o = p.wait_with_output().unwrap();
            assert!(o.status.success(), "git {args:?} failed");
            String::from_utf8(o.stdout).unwrap()
        };
        std::fs::create_dir_all(&dir).unwrap();
        git(&["init", "-q", "--bare"], None);
        git(&["index-pack", "--strict", "--stdin"], Some(&c.pack));
        let hex = c.commit.hex();
        git(&["fsck", "--strict", "--no-dangling", &hex], None);
        assert_eq!(git(&["show", &format!("{hex}:README.md")], None), "# hello\na test\n");
        assert!(git(&["cat-file", "commit", &hex], None).ends_with("\n\nInitial commit\n"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_readme_without_a_description_is_its_heading() {
        assert_eq!(readme("x", None), "# x\n");
        assert_eq!(readme("x", Some("")), "# x\n");
    }
}
