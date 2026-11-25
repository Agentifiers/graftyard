//! Streams GitHub-style `actions-artifacts` downloads for
//! `GET /repos/{owner}/{repo}/actions/artifacts/{id}/zip` without buffering whole files
//! (`decisions/DECISIONS.md`, "`actions-artifacts`"). Plain files use STORED and an incremental CRC; stored gzip
//! members contribute their raw deflate bytes and trailer CRC. Data descriptors let the caller emit each header
//! before streaming the payload.
//!
//! The records follow APPNOTE 6.3.10: sections 4.3.7 (local header), 4.3.9 (data descriptor), 4.3.12 (central
//! directory), 4.3.14-4.3.16 (ZIP64 and EOCD), and 4.5.3 (ZIP64 extra field).

use std::fmt;

const FLAGS: u16 = 0x0808;
const DOS_DATE: u16 = 0x0021;
const ZIP64: u16 = 0x0001;
const LIMIT32: u64 = 0xffff_ffff;

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < table.len() {
        let mut r = i as u32;
        let mut j = 0;
        while j < 8 {
            r = if r & 1 == 1 { (r >> 1) ^ 0xedb8_8320 } else { r >> 1 };
            j += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
}

const CRC_TABLE: [u32; 256] = crc_table();

/// IEEE CRC-32 (the zip and gzip one), table-driven, incremental.
#[derive(Clone, Copy, Debug)]
pub struct Crc32(u32);

impl Crc32 {
    pub fn new() -> Self {
        Self(0xffff_ffff)
    }

    pub fn update(&mut self, b: &[u8]) {
        for &byte in b {
            self.0 = (self.0 >> 8) ^ CRC_TABLE[((self.0 ^ u32::from(byte)) & 0xff) as usize];
        }
    }

    pub fn finish(&self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflate,
}

impl Method {
    fn number(self) -> u16 {
        match self {
            Self::Stored => 0,
            Self::Deflate => 8,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ZipError {
    NotGzip,
    Truncated,
    Order,
    Length,
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotGzip => "not a gzip member",
            Self::Truncated => "truncated gzip member",
            Self::Order => "zip entries are out of order",
            Self::Length => "file length does not match",
        })
    }
}

impl std::error::Error for ZipError {}

#[derive(Debug)]
struct Entry {
    name: Vec<u8>,
    method: Method,
    big: bool,
    offset: u64,
    crc: u32,
    compressed: u64,
    size: u64,
}

#[derive(Debug)]
struct Open {
    entry: Entry,
    data: u64,
}

/// Tracks offsets and entries; the caller emits the returned bytes and the payload bytes in order.
#[derive(Debug)]
pub struct ZipWriter {
    offset: u64,
    entries: Vec<Entry>,
    open: Option<Open>,
}

impl ZipWriter {
    pub fn new() -> Self {
        Self { offset: 0, entries: Vec::new(), open: None }
    }

    /// Emits a local file header and opens the next entry. A `big` entry carries zero ZIP64 sizes locally.
    pub fn local_header(&mut self, name: &str, method: Method, big: bool) -> Result<Vec<u8>, ZipError> {
        if self.open.is_some() {
            return Err(ZipError::Order);
        }

        let offset = self.offset;
        let name = name.as_bytes().to_vec();
        let mut out = Vec::with_capacity(30 + name.len() + if big { 20 } else { 0 });
        put32(&mut out, 0x0403_4b50);
        put16(&mut out, if big { 45 } else { 20 });
        put16(&mut out, FLAGS);
        put16(&mut out, method.number());
        put16(&mut out, 0);
        put16(&mut out, DOS_DATE);
        put32(&mut out, 0);
        put32(&mut out, if big { 0xffff_ffff } else { 0 });
        put32(&mut out, if big { 0xffff_ffff } else { 0 });
        put16(&mut out, name.len() as u16);
        put16(&mut out, if big { 20 } else { 0 });
        out.extend_from_slice(&name);
        if big {
            put16(&mut out, ZIP64);
            put16(&mut out, 16);
            put64(&mut out, 0);
            put64(&mut out, 0);
        }
        self.offset += out.len() as u64;
        self.open = Some(Open { entry: Entry { name, method, big, offset, crc: 0, compressed: 0, size: 0 }, data: 0 });
        Ok(out)
    }

    /// Counts `n` payload bytes of the current entry.
    pub fn data(&mut self, n: u64) {
        self.offset += n;
        if let Some(open) = &mut self.open {
            open.data += n;
        }
    }

    /// Emits the descriptor closing the current entry after checking its counted compressed size.
    pub fn descriptor(&mut self, crc: u32, compressed: u64, size: u64) -> Result<Vec<u8>, ZipError> {
        let Some(mut open) = self.open.take() else { return Err(ZipError::Order) };
        if open.data != compressed {
            self.open = Some(open);
            return Err(ZipError::Order);
        }

        open.entry.crc = crc;
        open.entry.compressed = compressed;
        open.entry.size = size;
        let mut out = Vec::with_capacity(if open.entry.big { 24 } else { 16 });
        put32(&mut out, 0x0807_4b50);
        put32(&mut out, crc);
        if open.entry.big {
            put64(&mut out, compressed);
            put64(&mut out, size);
        } else {
            put32(&mut out, compressed as u32);
            put32(&mut out, size as u32);
        }
        self.offset += out.len() as u64;
        self.entries.push(open.entry);
        Ok(out)
    }

    /// Emits the central directory, optional ZIP64 records, and the end-of-central-directory record.
    pub fn finish(self) -> Result<Vec<u8>, ZipError> {
        if self.open.is_some() {
            return Err(ZipError::Order);
        }

        let central_offset = self.offset;
        let mut out = Vec::new();
        for entry in &self.entries {
            let size64 = entry.size >= LIMIT32;
            let compressed64 = entry.compressed >= LIMIT32;
            let offset64 = entry.offset >= LIMIT32;
            let extra64 = 8 * (usize::from(size64) + usize::from(compressed64) + usize::from(offset64));

            put32(&mut out, 0x0201_4b50);
            put16(&mut out, if entry.big { 0x032d } else { 20 });
            put16(&mut out, if entry.big { 45 } else { 20 });
            put16(&mut out, FLAGS);
            put16(&mut out, entry.method.number());
            put16(&mut out, 0);
            put16(&mut out, DOS_DATE);
            put32(&mut out, entry.crc);
            put32(&mut out, if compressed64 { 0xffff_ffff } else { entry.compressed as u32 });
            put32(&mut out, if size64 { 0xffff_ffff } else { entry.size as u32 });
            put16(&mut out, entry.name.len() as u16);
            put16(&mut out, if extra64 == 0 { 0 } else { extra64 + 4 } as u16);
            put16(&mut out, 0);
            put16(&mut out, 0);
            put16(&mut out, 0);
            put32(&mut out, 0o100644 << 16);
            put32(&mut out, if offset64 { 0xffff_ffff } else { entry.offset as u32 });
            out.extend_from_slice(&entry.name);
            if extra64 != 0 {
                put16(&mut out, ZIP64);
                put16(&mut out, extra64 as u16);
                if size64 {
                    put64(&mut out, entry.size);
                }
                if compressed64 {
                    put64(&mut out, entry.compressed);
                }
                if offset64 {
                    put64(&mut out, entry.offset);
                }
            }
        }

        let central_size = out.len() as u64;
        let zip64 = self.entries.len() >= 0xffff || central_offset >= LIMIT32 || central_size >= LIMIT32;
        if zip64 {
            let zip64_offset = central_offset + central_size;
            put32(&mut out, 0x0606_4b50);
            put64(&mut out, 44);
            put16(&mut out, 0x032d);
            put16(&mut out, 45);
            put32(&mut out, 0);
            put32(&mut out, 0);
            put64(&mut out, self.entries.len() as u64);
            put64(&mut out, self.entries.len() as u64);
            put64(&mut out, central_size);
            put64(&mut out, central_offset);
            put32(&mut out, 0x0706_4b50);
            put32(&mut out, 0);
            put64(&mut out, zip64_offset);
            put32(&mut out, 1);
        }

        put32(&mut out, 0x0605_4b50);
        put16(&mut out, 0);
        put16(&mut out, 0);
        put16(&mut out, if zip64 { 0xffff } else { self.entries.len() as u16 });
        put16(&mut out, if zip64 { 0xffff } else { self.entries.len() as u16 });
        put32(&mut out, if zip64 { 0xffff_ffff } else { central_size as u32 });
        put32(&mut out, if zip64 { 0xffff_ffff } else { central_offset as u32 });
        put16(&mut out, 0);
        Ok(out)
    }

    /// Bytes emitted so far (headers, payload, descriptors).
    pub fn offset(&self) -> u64 {
        self.offset
    }

    #[cfg(test)]
    pub(crate) fn starting_at(offset: u64) -> Self {
        Self { offset, ..Self::new() }
    }
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum ComposerOpen {
    Plain { crc: Crc32, length: u64, pushed: u64 },
    Gzip { gzip: Gunwrap, length: u64 },
}

/// One artifact's files as one streamed zip: begin, push, and end each file, then finish.
/// Gzip files become DEFLATE using their own CRC and size; plain files become STORED with a streamed CRC.
#[derive(Debug)]
pub struct Composer {
    writer: ZipWriter,
    open: Option<ComposerOpen>,
}

impl Composer {
    pub fn new() -> Self {
        Self { writer: ZipWriter::new(), open: None }
    }

    /// Opens `name`; `length` is uncompressed and `size` is the stored byte length.
    /// `gzip` marks one stored gzip member. ZIP64 is used when either size reaches `0xffff_ffff`.
    pub fn begin(&mut self, name: &str, gzip: bool, length: u64, size: u64) -> Result<Vec<u8>, ZipError> {
        let method = if gzip { Method::Deflate } else { Method::Stored };
        let big = length >= LIMIT32 || size >= LIMIT32;
        let header = self.writer.local_header(name, method, big)?;
        self.open = Some(if gzip {
            ComposerOpen::Gzip { gzip: Gunwrap::new(), length }
        } else {
            ComposerOpen::Plain { crc: Crc32::new(), length, pushed: 0 }
        });
        Ok(header)
    }

    /// Pushes stored bytes and returns the zip payload bytes to emit now.
    pub fn push(&mut self, b: &[u8]) -> Result<Vec<u8>, ZipError> {
        let out = match self.open.as_mut() {
            Some(ComposerOpen::Plain { crc, pushed, .. }) => {
                crc.update(b);
                *pushed += b.len() as u64;
                b.to_vec()
            }
            Some(ComposerOpen::Gzip { gzip, .. }) => gzip.push(b)?,
            None => return Err(ZipError::Order),
        };
        self.writer.data(out.len() as u64);
        Ok(out)
    }

    /// Closes the open file and returns its data descriptor.
    /// Returns `Length` for a length mismatch and `Truncated` for a short gzip member.
    pub fn end(&mut self) -> Result<Vec<u8>, ZipError> {
        let Some(open) = self.open.take() else { return Err(ZipError::Order) };
        match open {
            ComposerOpen::Plain { crc, length, pushed } => {
                if pushed != length {
                    self.open = Some(ComposerOpen::Plain { crc, length, pushed });
                    return Err(ZipError::Length);
                }
                self.writer.descriptor(crc.finish(), pushed, length)
            }
            ComposerOpen::Gzip { gzip, length } => {
                let (crc, isize, emitted) = gzip.finish()?;
                if isize != length as u32 {
                    return Err(ZipError::Length);
                }
                self.writer.descriptor(crc, emitted, length)
            }
        }
    }

    /// Returns the central directory and end records.
    pub fn finish(self) -> Result<Vec<u8>, ZipError> {
        if self.open.is_some() {
            return Err(ZipError::Order);
        }
        self.writer.finish()
    }
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

fn put16(out: &mut Vec<u8>, n: u16) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, n: u32) {
    out.extend_from_slice(&n.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, n: u64) {
    out.extend_from_slice(&n.to_le_bytes());
}

#[derive(Clone, Copy, Debug)]
enum GzipState {
    Id1,
    Id2,
    Method,
    Flags,
    Rest(u8),
    ExtraLow,
    ExtraHigh(u8),
    Extra(u16),
    Name,
    Comment,
    HeaderCrc(u8),
    Body,
}

/// Strips one gzip member to its raw deflate data, streaming.
#[derive(Debug)]
pub struct Gunwrap {
    state: GzipState,
    flags: u8,
    tail: [u8; 8],
    tail_len: usize,
    emitted: u64,
}

impl Gunwrap {
    pub fn new() -> Self {
        Self { state: GzipState::Id1, flags: 0, tail: [0; 8], tail_len: 0, emitted: 0 }
    }

    /// Feeds gzip bytes and returns deflate bytes older than the possible eight-byte trailer.
    pub fn push(&mut self, b: &[u8]) -> Result<Vec<u8>, ZipError> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < b.len() {
            if matches!(self.state, GzipState::Body) {
                self.push_body(&mut out, &b[at..]);
                return Ok(out);
            }

            let byte = b[at];
            at += 1;
            self.state = match self.state {
                GzipState::Id1 => {
                    if byte != 0x1f {
                        return Err(ZipError::NotGzip);
                    }
                    GzipState::Id2
                }
                GzipState::Id2 => {
                    if byte != 0x8b {
                        return Err(ZipError::NotGzip);
                    }
                    GzipState::Method
                }
                GzipState::Method => {
                    if byte != 8 {
                        return Err(ZipError::NotGzip);
                    }
                    GzipState::Flags
                }
                GzipState::Flags => {
                    self.flags = byte;
                    GzipState::Rest(6)
                }
                GzipState::Rest(left) => {
                    if left == 1 {
                        if self.flags & 0x04 == 0 { after_extra(self.flags) } else { GzipState::ExtraLow }
                    } else {
                        GzipState::Rest(left - 1)
                    }
                }
                GzipState::ExtraLow => GzipState::ExtraHigh(byte),
                GzipState::ExtraHigh(low) => {
                    let len = u16::from_le_bytes([low, byte]);
                    if len == 0 { after_extra(self.flags) } else { GzipState::Extra(len) }
                }
                GzipState::Extra(left) => {
                    if left == 1 {
                        after_extra(self.flags)
                    } else {
                        GzipState::Extra(left - 1)
                    }
                }
                GzipState::Name => {
                    if byte == 0 {
                        after_name(self.flags)
                    } else {
                        GzipState::Name
                    }
                }
                GzipState::Comment => {
                    if byte == 0 {
                        after_comment(self.flags)
                    } else {
                        GzipState::Comment
                    }
                }
                GzipState::HeaderCrc(left) => {
                    if left == 1 {
                        GzipState::Body
                    } else {
                        GzipState::HeaderCrc(left - 1)
                    }
                }
                GzipState::Body => unreachable!("body input is handled in bulk"),
            };
        }
        Ok(out)
    }

    fn push_body(&mut self, out: &mut Vec<u8>, b: &[u8]) {
        let emit = (self.tail_len + b.len()).saturating_sub(8);
        let from_tail = emit.min(self.tail_len);
        let from_input = emit - from_tail;
        out.reserve(emit);
        out.extend_from_slice(&self.tail[..from_tail]);
        out.extend_from_slice(&b[..from_input]);
        self.emitted += emit as u64;

        let old_left = self.tail_len - from_tail;
        self.tail.copy_within(from_tail..self.tail_len, 0);
        self.tail[old_left..old_left + b.len() - from_input].copy_from_slice(&b[from_input..]);
        self.tail_len = old_left + b.len() - from_input;
    }

    /// Returns the trailer CRC and ISIZE and the exact number of deflate bytes emitted by `push`.
    pub fn finish(self) -> Result<(u32, u32, u64), ZipError> {
        if !matches!(self.state, GzipState::Body) || self.tail_len < self.tail.len() {
            return Err(ZipError::Truncated);
        }
        let crc = u32::from_le_bytes(self.tail[..4].try_into().expect("four-byte CRC"));
        let isize = u32::from_le_bytes(self.tail[4..].try_into().expect("four-byte ISIZE"));
        Ok((crc, isize, self.emitted))
    }
}

impl Default for Gunwrap {
    fn default() -> Self {
        Self::new()
    }
}

fn after_extra(flags: u8) -> GzipState {
    if flags & 0x08 != 0 { GzipState::Name } else { after_name(flags) }
}

fn after_name(flags: u8) -> GzipState {
    if flags & 0x10 != 0 { GzipState::Comment } else { after_comment(flags) }
}

fn after_comment(flags: u8) -> GzipState {
    if flags & 0x02 != 0 { GzipState::HeaderCrc(2) } else { GzipState::Body }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use flate2::read::DeflateDecoder;
    use flate2::write::{DeflateEncoder, GzEncoder};
    use flate2::{Compression, GzBuilder};

    use super::*;

    struct ReadEntry<'a> {
        name: String,
        method: Method,
        crc: u32,
        compressed: u64,
        size: u64,
        offset: u64,
        payload: &'a [u8],
        descriptor: &'a [u8],
    }

    struct Archive<'a> {
        entries: Vec<ReadEntry<'a>>,
        zip64: bool,
    }

    fn u16_at(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    fn physical(logical: u64, base: u64, len: usize) -> usize {
        let at = usize::try_from(logical.checked_sub(base).unwrap()).unwrap();
        assert!(at <= len);
        at
    }

    fn zip64_data(mut extra: &[u8]) -> &[u8] {
        while extra.len() >= 4 {
            let id = u16_at(extra, 0);
            let len = usize::from(u16_at(extra, 2));
            assert!(extra.len() >= 4 + len);
            if id == ZIP64 {
                return &extra[4..4 + len];
            }
            extra = &extra[4 + len..];
        }
        panic!("central entry has no ZIP64 extra field")
    }

    /// Locates the EOCD and central records without using a zip implementation.
    fn read_zip(bytes: &[u8], base: u64) -> Archive<'_> {
        assert!(bytes.len() >= 22);
        let first = bytes.len().saturating_sub(22 + 65_535);
        let eocd = (first..=bytes.len() - 22)
            .rev()
            .find(|&at| {
                u32_at(bytes, at) == 0x0605_4b50 && at + 22 + usize::from(u16_at(bytes, at + 20)) == bytes.len()
            })
            .expect("EOCD");
        let sentinel = u16_at(bytes, eocd + 8) == 0xffff
            || u16_at(bytes, eocd + 10) == 0xffff
            || u32_at(bytes, eocd + 12) == 0xffff_ffff
            || u32_at(bytes, eocd + 16) == 0xffff_ffff;
        let (count, central_size, central_offset) = if sentinel {
            let locator = eocd.checked_sub(20).unwrap();
            assert_eq!(u32_at(bytes, locator), 0x0706_4b50, "ZIP64 locator");
            assert_eq!(u32_at(bytes, locator + 4), 0);
            assert_eq!(u32_at(bytes, locator + 16), 1);
            let record_logical = u64_at(bytes, locator + 8);
            let record = physical(record_logical, base, bytes.len());
            assert_eq!(u32_at(bytes, record), 0x0606_4b50, "ZIP64 EOCD");
            assert_eq!(u64_at(bytes, record + 4), 44);
            assert_eq!(u16_at(bytes, record + 14), 45);
            let count = u64_at(bytes, record + 32);
            let size = u64_at(bytes, record + 40);
            let offset = u64_at(bytes, record + 48);
            assert_eq!(offset + size, record_logical);
            (count, size, offset)
        } else {
            let count = u64::from(u16_at(bytes, eocd + 10));
            let size = u64::from(u32_at(bytes, eocd + 12));
            let offset = u64::from(u32_at(bytes, eocd + 16));
            assert_eq!(offset + size, (eocd as u64) + base);
            (count, size, offset)
        };

        let central = physical(central_offset, base, bytes.len());
        let mut at = central;
        let mut entries = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(bytes, at), 0x0201_4b50, "central header");
            assert_eq!(u16_at(bytes, at + 8), FLAGS);
            let method = match u16_at(bytes, at + 10) {
                0 => Method::Stored,
                8 => Method::Deflate,
                other => panic!("unexpected method {other}"),
            };
            let crc = u32_at(bytes, at + 16);
            let compressed32 = u32_at(bytes, at + 20);
            let size32 = u32_at(bytes, at + 24);
            let name_len = usize::from(u16_at(bytes, at + 28));
            let extra_len = usize::from(u16_at(bytes, at + 30));
            let comment_len = usize::from(u16_at(bytes, at + 32));
            assert_eq!(u32_at(bytes, at + 38), 0o100644 << 16);
            let offset32 = u32_at(bytes, at + 42);
            let name_at = at + 46;
            let extra_at = name_at + name_len;
            let name = std::str::from_utf8(&bytes[name_at..extra_at]).unwrap().to_owned();

            let needs64 = compressed32 == 0xffff_ffff || size32 == 0xffff_ffff || offset32 == 0xffff_ffff;
            let extra = if needs64 { zip64_data(&bytes[extra_at..extra_at + extra_len]) } else { &[][..] };
            let mut z = 0;
            let mut take64 = |present: bool, small: u32| {
                if present {
                    let value = u64_at(extra, z);
                    z += 8;
                    value
                } else {
                    u64::from(small)
                }
            };
            let size = take64(size32 == 0xffff_ffff, size32);
            let compressed = take64(compressed32 == 0xffff_ffff, compressed32);
            let offset = take64(offset32 == 0xffff_ffff, offset32);
            assert_eq!(z, extra.len(), "only overflowing fields are in the ZIP64 extra");

            let local = physical(offset, base, bytes.len());
            assert_eq!(u32_at(bytes, local), 0x0403_4b50, "local header at central offset");
            assert_eq!(u16_at(bytes, local + 4), u16_at(bytes, at + 6));
            assert_eq!(u16_at(bytes, local + 6), FLAGS);
            let local_name_len = usize::from(u16_at(bytes, local + 26));
            let local_extra_len = usize::from(u16_at(bytes, local + 28));
            assert_eq!(&bytes[local + 30..local + 30 + local_name_len], name.as_bytes());
            let payload_at = local + 30 + local_name_len + local_extra_len;
            let payload_len = usize::try_from(compressed).unwrap();
            let payload = &bytes[payload_at..payload_at + payload_len];
            let descriptor_at = payload_at + payload_len;
            let big = u32_at(bytes, local + 18) == 0xffff_ffff;
            let descriptor_len = if big { 24 } else { 16 };
            let descriptor = &bytes[descriptor_at..descriptor_at + descriptor_len];
            assert_eq!(u32_at(descriptor, 0), 0x0807_4b50, "descriptor follows payload");
            assert_eq!(u32_at(descriptor, 4), crc);
            if big {
                assert_eq!(u64_at(descriptor, 8), compressed);
                assert_eq!(u64_at(descriptor, 16), size);
            } else {
                assert_eq!(u32_at(descriptor, 8) as u64, compressed);
                assert_eq!(u32_at(descriptor, 12), size as u32);
            }
            entries.push(ReadEntry { name, method, crc, compressed, size, offset, payload, descriptor });
            at = extra_at + extra_len + comment_len;
        }
        assert_eq!((at - central) as u64, central_size);
        Archive { entries, zip64: sentinel }
    }

    fn emit(writer: &mut ZipWriter, zip: &mut Vec<u8>, bytes: Vec<u8>) {
        zip.extend_from_slice(&bytes);
        assert_eq!(writer.offset(), zip.len() as u64);
    }

    fn stored(writer: &mut ZipWriter, zip: &mut Vec<u8>, name: &str, contents: &[u8], big: bool) {
        let header = writer.local_header(name, Method::Stored, big).unwrap();
        emit(writer, zip, header);
        zip.extend_from_slice(contents);
        writer.data(contents.len() as u64);
        let mut crc = Crc32::new();
        crc.update(contents);
        let descriptor = writer.descriptor(crc.finish(), contents.len() as u64, contents.len() as u64).unwrap();
        emit(writer, zip, descriptor);
    }

    fn push_pattern(composer: &mut Composer, zip: &mut Vec<u8>, bytes: &[u8], pattern: &[usize]) {
        let mut at = 0;
        let mut piece = 0;
        while at < bytes.len() {
            let len = pattern[piece % pattern.len()].min(bytes.len() - at);
            zip.extend(composer.push(&bytes[at..at + len]).unwrap());
            at += len;
            piece += 1;
        }
    }

    fn composed_zip(gzip: &[u8], text_len: usize, plain: &[u8], pattern: &[usize]) -> Vec<u8> {
        let mut composer = Composer::new();
        let mut zip = composer.begin("random.txt", true, text_len as u64, gzip.len() as u64).unwrap();
        push_pattern(&mut composer, &mut zip, gzip, pattern);
        zip.extend(composer.end().unwrap());
        zip.extend(composer.begin("plain.bin", false, plain.len() as u64, plain.len() as u64).unwrap());
        push_pattern(&mut composer, &mut zip, plain, pattern);
        zip.extend(composer.end().unwrap());
        zip.extend(composer.begin("empty", false, 0, 0).unwrap());
        zip.extend(composer.end().unwrap());
        zip.extend(composer.finish().unwrap());
        zip
    }

    #[test]
    fn crc32_matches_the_ieee_check_value_at_every_split() {
        let input = b"123456789";
        let mut one = Crc32::new();
        one.update(input);
        assert_eq!(one.finish(), 0xcbf4_3926);
        for split in 0..=input.len() {
            let mut incremental = Crc32::new();
            incremental.update(&input[..split]);
            incremental.update(&input[split..]);
            assert_eq!(incremental.finish(), one.finish(), "split at {split}");
        }
    }

    #[test]
    fn stored_deflate_and_empty_entries_round_trip_through_a_minimal_reader() {
        let plain = b"hello zip";
        let empty = b"";
        let mut random = 0x1234_5678u32;
        let binary: Vec<u8> = (0..300 * 1024)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                b'a' + (random % 16) as u8
            })
            .collect();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&binary).unwrap();
        let gzip = encoder.finish().unwrap();
        let mut gunwrap = Gunwrap::new();
        let mut deflate = gunwrap.push(&gzip[..1]).unwrap();
        deflate.extend(gunwrap.push(&gzip[1..8]).unwrap());
        for piece in gzip[8..].chunks(4096) {
            deflate.extend(gunwrap.push(piece).unwrap());
        }
        let (gzip_crc, isize, emitted) = gunwrap.finish().unwrap();
        assert_eq!(emitted, deflate.len() as u64);
        assert_eq!(isize, binary.len() as u32);
        let mut binary_crc = Crc32::new();
        binary_crc.update(&binary);
        assert_eq!(gzip_crc, binary_crc.finish());

        let mut writer = ZipWriter::new();
        let mut zip = Vec::new();
        stored(&mut writer, &mut zip, "a.txt", plain, false);
        let header = writer.local_header("dir/b.bin", Method::Deflate, false).unwrap();
        emit(&mut writer, &mut zip, header);
        zip.extend_from_slice(&deflate);
        writer.data(deflate.len() as u64);
        let descriptor = writer.descriptor(gzip_crc, deflate.len() as u64, binary.len() as u64).unwrap();
        emit(&mut writer, &mut zip, descriptor);
        stored(&mut writer, &mut zip, "empty", empty, false);
        let end = writer.finish().unwrap();
        zip.extend_from_slice(&end);

        let archive = read_zip(&zip, 0);
        assert!(!archive.zip64);
        assert_eq!(archive.entries.len(), 3);
        let expected: [(&str, Method, &[u8]); 3] = [
            ("a.txt", Method::Stored, plain),
            ("dir/b.bin", Method::Deflate, &binary),
            ("empty", Method::Stored, empty),
        ];
        for (entry, (name, method, original)) in archive.entries.iter().zip(expected) {
            assert_eq!(entry.name, name);
            assert_eq!(entry.method, method);
            assert_eq!(entry.size, original.len() as u64);
            assert_eq!(entry.compressed, entry.payload.len() as u64);
            let actual = match method {
                Method::Stored => entry.payload.to_vec(),
                Method::Deflate => {
                    let mut decoder = DeflateDecoder::new(entry.payload);
                    let mut inflated = Vec::new();
                    decoder.read_to_end(&mut inflated).unwrap();
                    inflated
                }
            };
            assert_eq!(actual, original);
            let mut crc = Crc32::new();
            crc.update(original);
            assert_eq!(entry.crc, crc.finish());
            assert_eq!(entry.descriptor[0..4], 0x0807_4b50u32.to_le_bytes());
        }
    }

    #[test]
    fn composer_round_trips_gzip_plain_and_empty_files_at_every_chunking() {
        let mut random = 0x6d2b_79f5u32;
        let text: Vec<u8> = (0..300 * 1024)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                b'a' + (random % 26) as u8
            })
            .collect();
        let plain: Vec<u8> = (0..4099)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                random as u8
            })
            .collect();
        let mut encoder = GzBuilder::new().filename("x").write(Vec::new(), Compression::default());
        encoder.write_all(&text).unwrap();
        let gzip = encoder.finish().unwrap();
        assert_eq!(&gzip[..4], &[0x1f, 0x8b, 8, 0x08], "FNAME is present");
        assert_eq!(&gzip[10..12], b"x\0");

        let expected = composed_zip(&gzip, text.len(), &plain, &[usize::MAX]);
        let patterns =
            [vec![1], vec![2, 3, 5, 7, 4096], vec![10, 1, 1], vec![gzip.len() - 4, 1, 3], vec![4096, 1, 17, 31]];
        for pattern in patterns {
            assert_eq!(composed_zip(&gzip, text.len(), &plain, &pattern), expected, "chunk pattern {pattern:?}");
        }

        let archive = read_zip(&expected, 0);
        assert!(!archive.zip64);
        assert_eq!(archive.entries.len(), 3);
        let originals: [(&str, Method, &[u8]); 3] = [
            ("random.txt", Method::Deflate, &text),
            ("plain.bin", Method::Stored, &plain),
            ("empty", Method::Stored, &[]),
        ];
        for (entry, (name, method, original)) in archive.entries.iter().zip(originals) {
            assert_eq!(entry.name, name);
            assert_eq!(entry.method, method);
            assert_eq!(entry.size, original.len() as u64);
            assert_eq!(entry.compressed, entry.payload.len() as u64);
            let actual = match method {
                Method::Stored => entry.payload.to_vec(),
                Method::Deflate => {
                    let mut decoder = DeflateDecoder::new(entry.payload);
                    let mut inflated = Vec::new();
                    decoder.read_to_end(&mut inflated).unwrap();
                    inflated
                }
            };
            assert_eq!(actual, original);
            let mut crc = Crc32::new();
            crc.update(original);
            assert_eq!(entry.crc, crc.finish());
        }
    }

    #[test]
    fn composer_rejects_length_mismatches_and_truncated_gzip() {
        let original = b"length checked";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let gzip = encoder.finish().unwrap();

        let mut bad_gzip = Composer::new();
        bad_gzip.begin("bad", true, original.len() as u64 + 1, gzip.len() as u64).unwrap();
        bad_gzip.push(&gzip).unwrap();
        assert_eq!(bad_gzip.end(), Err(ZipError::Length));

        let mut short_plain = Composer::new();
        short_plain.begin("short", false, original.len() as u64, original.len() as u64).unwrap();
        short_plain.push(&original[..original.len() - 1]).unwrap();
        assert_eq!(short_plain.end(), Err(ZipError::Length));

        let mut truncated = Composer::new();
        truncated.begin("short.gz", true, original.len() as u64, gzip.len() as u64).unwrap();
        truncated.push(&gzip[..17]).unwrap();
        assert_eq!(truncated.end(), Err(ZipError::Truncated));
    }

    #[test]
    fn gunwrap_body_bulk_matches_bytewise_at_every_split() {
        let mut random = 0x9e37_79b9u32;
        let original: Vec<u8> = (0..1025)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                random as u8
            })
            .collect();
        let mut encoder = GzBuilder::new().filename("bulk").write(Vec::new(), Compression::default());
        encoder.write_all(&original).unwrap();
        let gzip = encoder.finish().unwrap();

        let mut bytewise = Gunwrap::new();
        let mut expected = Vec::new();
        for byte in &gzip {
            expected.extend(bytewise.push(std::slice::from_ref(byte)).unwrap());
        }
        let expected_finish = bytewise.finish().unwrap();
        assert_eq!(expected_finish.2, expected.len() as u64);

        for split in 0..=gzip.len() {
            let mut bulk = Gunwrap::new();
            let mut actual = bulk.push(&gzip[..split]).unwrap();
            actual.extend(bulk.push(&gzip[split..]).unwrap());
            assert_eq!(actual, expected, "split at {split}");
            assert_eq!(bulk.finish(), Ok(expected_finish), "split at {split}");
        }
    }

    #[test]
    fn gunwrap_handles_split_optional_headers_and_rejects_bad_or_short_input() {
        let original = b"optional gzip fields retain the deflate stream";
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let deflate = encoder.finish().unwrap();
        let mut crc = Crc32::new();
        crc.update(original);

        let mut gzip = vec![0x1f, 0x8b, 8, 0x0c, 0, 0, 0, 0, 0, 255, 3, 0, 1, 2, 3];
        gzip.extend_from_slice(b"file.bin\0");
        gzip.extend_from_slice(&deflate);
        gzip.extend_from_slice(&crc.finish().to_le_bytes());
        gzip.extend_from_slice(&(original.len() as u32).to_le_bytes());

        let mut gunwrap = Gunwrap::new();
        let mut actual = Vec::new();
        for byte in &gzip {
            actual.extend(gunwrap.push(std::slice::from_ref(byte)).unwrap());
        }
        let (trailer_crc, isize, emitted) = gunwrap.finish().unwrap();
        assert_eq!(actual, deflate);
        assert_eq!(emitted, deflate.len() as u64);
        assert_eq!(trailer_crc, crc.finish());
        assert_eq!(isize, original.len() as u32);

        let mut truncated = Gunwrap::new();
        truncated.push(&gzip[..5]).unwrap();
        assert_eq!(truncated.finish(), Err(ZipError::Truncated));
        let mut short_tail = Gunwrap::new();
        short_tail.push(&[0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255, 1, 2, 3]).unwrap();
        assert_eq!(short_tail.finish(), Err(ZipError::Truncated));
        assert_eq!(Gunwrap::new().push(b"not gzip"), Err(ZipError::NotGzip));
    }

    #[test]
    fn zip64_offsets_are_discoverable_through_the_locator_and_extra_field() {
        const BASE: u64 = 0x1_0000_0000;
        let mut writer = ZipWriter::starting_at(BASE);
        let mut zip = Vec::new();
        zip.extend_from_slice(&writer.local_header("big", Method::Stored, true).unwrap());
        zip.extend_from_slice(b"small");
        writer.data(5);
        let mut crc = Crc32::new();
        crc.update(b"small");
        zip.extend_from_slice(&writer.descriptor(crc.finish(), 5, 5).unwrap());
        assert_eq!(writer.offset(), BASE + zip.len() as u64);
        zip.extend_from_slice(&writer.finish().unwrap());

        let archive = read_zip(&zip, BASE);
        assert!(archive.zip64);
        assert_eq!(archive.entries.len(), 1);
        let entry = &archive.entries[0];
        assert!(entry.offset >= BASE);
        assert_eq!(entry.offset, BASE);
        assert_eq!(entry.payload, b"small");
        assert_eq!(entry.descriptor.len(), 24);
    }

    #[test]
    fn entry_order_and_counted_sizes_are_enforced() {
        let mut writer = ZipWriter::new();
        assert_eq!(writer.descriptor(0, 0, 0), Err(ZipError::Order));
        writer.local_header("open", Method::Stored, false).unwrap();
        assert_eq!(writer.local_header("next", Method::Stored, false), Err(ZipError::Order));
        writer.data(2);
        assert_eq!(writer.descriptor(0, 3, 2), Err(ZipError::Order));
        assert_eq!(writer.finish(), Err(ZipError::Order));

        let mut open = ZipWriter::new();
        open.local_header("open", Method::Stored, false).unwrap();
        assert_eq!(open.finish(), Err(ZipError::Order));
    }
}
