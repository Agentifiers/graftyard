//! The bytes of an `R2Key::Archive` object (`design/DESIGN.md`, Shape, "Packs"): a header, then each archived log entry
//! as the member stores it, its `graftyard_raft::codec` row, behind a 4-byte big-endian length. A reader streams the
//! object a range at a time and holds one entry, as `InstallSnapshot`'s loader and the fold audit do.

/// The first bytes of every archive object, then a format version.
const MAGIC: &[u8; 8] = b"gyfarch\0";
const ARCHIVE_FORMAT: u16 = 1;
/// The header's length: where the first entry starts.
pub const ARCHIVE_HEADER: usize = 10;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArchiveError {
    #[error("not an archive object")]
    Magic,
    #[error("archive format {0} is not one this release reads")]
    Format(u16),
}

pub fn archive_header() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&ARCHIVE_FORMAT.to_be_bytes());
    out
}

/// Checks an archive object's first `ARCHIVE_HEADER` bytes.
pub fn check_archive_header(b: &[u8]) -> Result<(), ArchiveError> {
    if b.len() < ARCHIVE_HEADER || &b[..8] != MAGIC {
        return Err(ArchiveError::Magic);
    }
    match u16::from_be_bytes([b[8], b[9]]) {
        ARCHIVE_FORMAT => Ok(()),
        v => Err(ArchiveError::Format(v)),
    }
}

pub fn encode_archive_row(row: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(row.len() as u32).to_be_bytes());
    out.extend_from_slice(row);
}

/// Splits the complete rows off the front of `buf`, which starts at a row boundary: the rows, how many bytes they
/// used, and, when not even one row is complete, the bytes the next row needs.
pub fn decode_archive_some(buf: &[u8]) -> (Vec<&[u8]>, usize, Option<usize>) {
    let mut at = 0;
    let mut out = Vec::new();
    while buf.len() >= at + 4 {
        let n = u32::from_be_bytes(buf[at..at + 4].try_into().expect("4")) as usize;
        // `at + 4 + n` can overflow a 32-bit usize for a forged length; the body's length cannot.
        let body = &buf[at + 4..];
        if body.len() < n {
            break;
        }
        out.push(&body[..n]);
        // Cannot overflow: `at + 4 + n <= buf.len()`.
        at += 4 + n;
    }
    let need = if out.is_empty() && buf.len() >= 4 {
        Some(4usize.saturating_add(u32::from_be_bytes(buf[..4].try_into().expect("4")) as usize))
    } else {
        None
    };
    (out, at, need)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_split_at_any_cut_and_a_short_buffer_names_what_it_needs() {
        let mut b = archive_header();
        for r in [&b"one"[..], b"", b"three!"] {
            encode_archive_row(r, &mut b);
        }
        check_archive_header(&b).unwrap();
        assert_eq!(check_archive_header(b"gyfsnap\0\0\x01"), Err(ArchiveError::Magic));
        let body = &b[ARCHIVE_HEADER..];
        let (rows, used, need) = decode_archive_some(body);
        assert_eq!(rows, [&b"one"[..], b"", b"three!"]);
        assert_eq!((used, need), (body.len(), None));
        let (rows, used, need) = decode_archive_some(&body[..5]);
        assert_eq!((rows.len(), used, need), (0, 0, Some(7)));
        let (rows, used, _) = decode_archive_some(&body[..12]);
        assert_eq!((rows, used), (vec![&b"one"[..], b""], 11));
    }

    /// A corrupted R2 object's length prefix near `u32::MAX` once overflowed `at + 4 + n` on wasm32 and panicked.
    #[test]
    fn a_forged_length_reads_as_incomplete() {
        let mut b = vec![0xFF; 4];
        b.extend_from_slice(b"abc");
        let (rows, used, need) = decode_archive_some(&b);
        assert_eq!((rows.len(), used), (0, 0));
        assert_eq!(need, Some(4usize.saturating_add(0xFFFF_FFFF)));
    }
}
