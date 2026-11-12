//! One commit object read from the git cache shard, for REST's `GET /repos/{o}/{r}/commits/{ref}`: a protocol v2 fetch
//! of the commit alone (`deepen 1`, `filter tree:0`, so the pack holds no tree, blob, or parent), and the commit's raw
//! content cut from the response's pack, checked against its id. The shard runs `git upload-pack` as it does for a
//! clone, so no new shard route is needed.

use std::io::Read;

use sha1::{Digest, Sha1};

use crate::pkt::{self, Pkt, Reader};
use crate::refs::ObjectId;

/// The `upload-pack` request body, sent with `Git-Protocol: version=2`.
pub fn request(oid: ObjectId) -> Vec<u8> {
    let mut b = Vec::new();
    pkt::line(&mut b, "command=fetch");
    pkt::line(&mut b, "object-format=sha1");
    pkt::delim(&mut b);
    for a in ["no-progress", &format!("want {}", oid.hex()), "deepen 1", "filter tree:0", "done"] {
        pkt::line(&mut b, a);
    }
    pkt::flush(&mut b);
    b
}

/// The raw content of commit `oid` (what follows `commit <size>\0`) from a v2 fetch response: the pack's data from
/// side-band channel 1 after the `packfile` line, whose first object must be a commit hashing to `oid`.
pub fn commit_of(response: &[u8], oid: ObjectId) -> Result<Vec<u8>, String> {
    let mut r = Reader::new(response);
    let (mut in_pack, mut pack) = (false, Vec::new());
    while let Some(p) = r.next_pkt().map_err(|e| e.to_string())? {
        match p {
            Pkt::Data(d) if in_pack => match d.split_first() {
                Some((1, rest)) => pack.extend_from_slice(rest),
                Some((3, rest)) => return Err(format!("upload-pack: {}", String::from_utf8_lossy(rest).trim_end())),
                _ => {}
            },
            Pkt::Data(_) if p.text() == Some(b"packfile") => in_pack = true,
            Pkt::Data(d) if d.starts_with(b"ERR ") => return Err(String::from_utf8_lossy(d).trim_end().into()),
            Pkt::Flush if in_pack => break,
            _ => {}
        }
    }
    if pack.len() < 12 || &pack[..4] != b"PACK" {
        return Err("the response holds no pack".into());
    }
    let mut at = 12;
    let byte = |at: &mut usize| -> Result<u8, String> {
        let b = *pack.get(*at).ok_or("the pack ends inside an object header")?;
        *at += 1;
        Ok(b)
    };
    let first = byte(&mut at)?;
    let (kind, mut size, mut more, mut shift) = ((first >> 4) & 7, u64::from(first & 15), first & 0x80 != 0, 4);
    while more {
        let b = byte(&mut at)?;
        size |= u64::from(b & 0x7f) << shift;
        (more, shift) = (b & 0x80 != 0, shift + 7);
    }
    if kind != 1 {
        return Err(format!("the pack's first object is of type {kind}, not a commit"));
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&pack[at..]).take(size).read_to_end(&mut raw).map_err(|e| e.to_string())?;
    if raw.len() as u64 != size {
        return Err("the commit object is shorter than its header".into());
    }
    let id: [u8; 20] = Sha1::new().chain_update(format!("commit {size}\0")).chain_update(&raw).finalize().into();
    if id != oid.0 {
        return Err(format!("the pack's commit is {}, not {}", ObjectId(id).hex(), oid.hex()));
    }
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn object(kind: u8, raw: &[u8]) -> Vec<u8> {
        let mut size = raw.len();
        let mut b = vec![(kind << 4) | (size & 15) as u8];
        size >>= 4;
        while size > 0 {
            *b.last_mut().unwrap() |= 0x80;
            b.push((size & 0x7f) as u8);
            size >>= 7;
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(raw).unwrap();
        b.extend(z.finish().unwrap());
        b
    }

    /// A v2 fetch response: a `shallow-info` section, then the pack split over channel 1 with a progress packet
    /// between its halves.
    fn response(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut pack = b"PACK\0\0\0\x02".to_vec();
        pack.extend((objects.len() as u32).to_be_bytes());
        objects.iter().for_each(|o| pack.extend(o));
        pack.extend([0u8; 20]);
        let mut b = Vec::new();
        pkt::line(&mut b, "shallow-info");
        pkt::line(&mut b, &format!("shallow {}", "ab".repeat(20)));
        pkt::delim(&mut b);
        pkt::line(&mut b, "packfile");
        let (a, z) = pack.split_at(7);
        pkt::sideband(&mut b, pkt::Band::Large, 1, a);
        pkt::sideband(&mut b, pkt::Band::Large, 2, b"Counting objects\n");
        pkt::sideband(&mut b, pkt::Band::Large, 1, z);
        pkt::flush(&mut b);
        b
    }

    fn id_of(kind: &str, raw: &[u8]) -> ObjectId {
        ObjectId(Sha1::new().chain_update(format!("{kind} {}\0", raw.len())).chain_update(raw).finalize().into())
    }

    /// A message over 15 bytes, so the size takes more than the first header byte, and over 127 so it takes three.
    const RAW: &[u8] = b"tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904\nauthor A <a@x> 1700000000 +0000\ncommitter A <a@x> 1700000000 +0000\n\nrun\xe2\x80\x8b the tests, a message long enough that its size needs a third header byte of the pack's varint encoding\n";

    #[test]
    fn the_commit_is_cut_from_the_pack_and_checked_against_its_id() {
        let id = id_of("commit", RAW);
        assert_eq!(commit_of(&response(&[object(1, RAW)]), id).unwrap(), RAW);
        // Another commit's id, a tree first, a truncated object, and an error packet are refused.
        assert!(commit_of(&response(&[object(1, RAW)]), ObjectId([7; 20])).unwrap_err().contains("not"));
        assert!(commit_of(&response(&[object(2, RAW)]), id).unwrap_err().contains("type 2"));
        let mut cut = object(1, RAW);
        cut.truncate(cut.len() - 8);
        assert!(commit_of(&response(&[cut]), id).is_err());
        let mut err = Vec::new();
        pkt::line(&mut err, "packfile");
        pkt::sideband(&mut err, pkt::Band::Large, 3, b"not our ref");
        assert!(commit_of(&err, id).unwrap_err().contains("not our ref"));
    }

    #[test]
    fn the_request_asks_for_the_commit_alone() {
        let req = crate::smart::parse_v2(&request(ObjectId([0xab; 20]))).unwrap();
        assert_eq!(req.command, "fetch");
        let args: Vec<String> = req.args.iter().map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        assert_eq!(args, ["no-progress", &format!("want {}", "ab".repeat(20)), "deepen 1", "filter tree:0", "done"]);
    }
}
