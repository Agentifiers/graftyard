//! pkt-line, git's framing (https://git-scm.com/docs/gitprotocol-common): four hex digits giving the length of the
//! line including themselves, then the payload. `0000` is a flush, `0001` a delimiter, and `0002` a response end
//! (protocol v2). A payload is at most 65516 bytes (git's `LARGE_PACKET_MAX` of 65520 minus the length).

use thiserror::Error;

pub const MAX_PAYLOAD: usize = 65516;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pkt<'a> {
    Flush,
    Delim,
    End,
    Data(&'a [u8]),
}

impl<'a> Pkt<'a> {
    /// A data line without its trailing newline, which git writes on text lines and readers must not require.
    pub fn text(&self) -> Option<&'a [u8]> {
        match self {
            Pkt::Data(d) => Some(d.strip_suffix(b"\n").unwrap_or(d)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum PktError {
    #[error("pkt-line truncated at byte {0}")]
    Truncated(usize),
    #[error("pkt-line length is not four hex digits at byte {0}")]
    BadLength(usize),
    /// `0003` names no payload and is not a special packet.
    #[error("pkt-line length 3 at byte {0}")]
    Reserved(usize),
    #[error("pkt-line over {MAX_PAYLOAD} bytes at byte {0}")]
    TooLong(usize),
}

/// Reads packets from a buffer. It never reads past the buffer, and `rest` is whatever follows the last packet read,
/// such as the pack after a receive-pack request's commands.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

fn hex4(b: &[u8]) -> Option<usize> {
    let mut n = 0usize;
    for &c in b {
        let d = (c as char).to_digit(16)?;
        n = n * 16 + d as usize;
    }
    Some(n)
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    pub fn rest(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The next packet, or `None` at the end of the buffer.
    pub fn next_pkt(&mut self) -> Result<Option<Pkt<'a>>, PktError> {
        let at = self.pos;
        if at == self.buf.len() {
            return Ok(None);
        }
        let Some(head) = self.buf.get(at..at + 4) else { return Err(PktError::Truncated(at)) };
        let n = hex4(head).ok_or(PktError::BadLength(at))?;
        let pkt = match n {
            0 => Pkt::Flush,
            1 => Pkt::Delim,
            2 => Pkt::End,
            3 => return Err(PktError::Reserved(at)),
            _ if n - 4 > MAX_PAYLOAD => return Err(PktError::TooLong(at)),
            _ => Pkt::Data(self.buf.get(at + 4..at + n).ok_or(PktError::Truncated(at))?),
        };
        self.pos = at + n.max(4);
        Ok(Some(pkt))
    }
}

/// Appends one data packet. Panics on a payload over `MAX_PAYLOAD`: callers split large data with `sideband`.
pub fn data(out: &mut Vec<u8>, payload: &[u8]) {
    assert!(payload.len() <= MAX_PAYLOAD, "pkt-line payload of {} bytes", payload.len());
    out.extend_from_slice(format!("{:04x}", payload.len() + 4).as_bytes());
    out.extend_from_slice(payload);
}

/// A text line: the payload and a newline.
pub fn line(out: &mut Vec<u8>, s: &str) {
    let mut p = Vec::with_capacity(s.len() + 1);
    p.extend_from_slice(s.as_bytes());
    p.push(b'\n');
    data(out, &p);
}

pub fn flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

pub fn delim(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0001");
}

/// The side-band channels (`side-band` and `side-band-64k`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// `side-band`: packets of at most 1000 bytes.
    Small,
    /// `side-band-64k`: packets of at most 65520 bytes.
    Large,
}

impl Band {
    /// The most data bytes one packet carries after its band byte.
    pub fn chunk(self) -> usize {
        match self {
            Band::Small => 1000 - 4 - 1,
            Band::Large => MAX_PAYLOAD - 1,
        }
    }
}

/// `payload` on channel `channel` (1 data, 2 progress, 3 error), split into packets of the band's size.
pub fn sideband(out: &mut Vec<u8>, band: Band, channel: u8, payload: &[u8]) {
    for c in payload.chunks(band.chunk()) {
        let mut p = Vec::with_capacity(c.len() + 1);
        p.push(channel);
        p.extend_from_slice(c);
        data(out, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(b: &[u8]) -> Result<Vec<Pkt<'_>>, PktError> {
        let mut r = Reader::new(b);
        let mut v = Vec::new();
        while let Some(p) = r.next_pkt()? {
            v.push(p);
        }
        Ok(v)
    }

    #[test]
    fn round_trips_lines_and_special_packets() {
        let mut out = Vec::new();
        line(&mut out, "command=ls-refs");
        delim(&mut out);
        data(&mut out, b"");
        flush(&mut out);
        out.extend_from_slice(b"0002");
        assert_eq!(&out[..4], b"0014");
        let p = all(&out).unwrap();
        assert_eq!(p, vec![Pkt::Data(b"command=ls-refs\n"), Pkt::Delim, Pkt::Data(b""), Pkt::Flush, Pkt::End]);
        assert_eq!(p[0].text(), Some(&b"command=ls-refs"[..]));
        assert_eq!(Pkt::Flush.text(), None);
    }

    #[test]
    fn refuses_bad_lengths_without_reading_past_the_buffer() {
        assert_eq!(all(b"00"), Err(PktError::Truncated(0)));
        assert_eq!(all(b"0003"), Err(PktError::Reserved(0)));
        assert_eq!(all(b"00zz"), Err(PktError::BadLength(0)));
        assert_eq!(all(b"+00a"), Err(PktError::BadLength(0)), "from_str_radix would take a sign");
        assert_eq!(all(b"0009abc"), Err(PktError::Truncated(0)));
        assert_eq!(all(b"0000000Aabcdef"), Ok(vec![Pkt::Flush, Pkt::Data(b"abcdef")]), "upper-case hex is hex");
        assert_eq!(all(b"fff1"), Err(PktError::TooLong(0)));
        let mut max = Vec::new();
        data(&mut max, &vec![b'x'; MAX_PAYLOAD]);
        assert_eq!(&max[..4], b"fff0");
        assert_eq!(all(&max).unwrap().len(), 1);
    }

    #[test]
    fn rest_is_what_follows_the_last_packet() {
        let mut b = Vec::new();
        line(&mut b, "a b c");
        flush(&mut b);
        b.extend_from_slice(b"PACK\0\0\0\x02");
        let mut r = Reader::new(&b);
        assert!(matches!(r.next_pkt(), Ok(Some(Pkt::Data(_)))));
        assert_eq!(r.next_pkt(), Ok(Some(Pkt::Flush)));
        assert_eq!(r.rest(), b"PACK\0\0\0\x02");
    }

    #[test]
    fn sideband_splits_at_the_band_size() {
        let mut out = Vec::new();
        sideband(&mut out, Band::Small, 1, &vec![7u8; 2000]);
        let p = all(&out).unwrap();
        assert_eq!(p.len(), 3);
        for x in &p {
            let Pkt::Data(d) = x else { panic!() };
            assert!(d.len() <= 996 && d[0] == 1);
        }
        let mut out = Vec::new();
        sideband(&mut out, Band::Large, 2, &vec![7u8; 70_000]);
        assert_eq!(all(&out).unwrap().len(), 2);
    }

    /// Random bytes and random mutations of valid streams: the reader returns an error or packets, never panics,
    /// and never reports a position past the buffer.
    #[test]
    fn fuzz_never_panics_or_overreads() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut valid = Vec::new();
        line(&mut valid, "want 0123456789012345678901234567890123456789");
        delim(&mut valid);
        line(&mut valid, "done");
        flush(&mut valid);
        for i in 0..200_000 {
            let mut b = if i % 2 == 0 {
                (0..rnd() % 40).map(|_| b"0123456789abcdefABCDEFxyz\n"[(rnd() % 26) as usize]).collect::<Vec<u8>>()
            } else {
                valid.clone()
            };
            if i % 2 == 1 {
                let at = (rnd() as usize) % b.len();
                b[at] = rnd() as u8;
                b.truncate((rnd() as usize) % (b.len() + 1));
            }
            let mut r = Reader::new(&b);
            for _ in 0..64 {
                match r.next_pkt() {
                    Ok(Some(_)) => assert!(r.pos() <= b.len()),
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }
}
