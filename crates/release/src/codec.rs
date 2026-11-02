//! The canonical encoding a signed `Release` entry is stored, sent, and signed in. It follows `crates/raft`'s `Enc`
//! rule, so one value has one spelling:
//!
//! - integers are fixed width, big-endian;
//! - a byte string or `str` is its length as `u32` and then its bytes;
//! - a sequence is its count as `u32` and then each item;
//! - an `Option` is `0` for `None`, or `1` and then the value, and a `bool` is `0` or `1`;
//! - an enum is a `u8` tag in the design's declaration order and then its fields in order;
//! - a struct is its fields in declaration order.
//!
//! `Dec` is strict: an unknown tag, a `bool` or `Option` byte other than 0 or 1, a length past the end, invalid UTF-8,
//! and trailing bytes are all refused, so decoding and re-encoding gives back the same bytes.

use std::fmt;

/// Why bytes are not a canonical entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub Box<str>);

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn bad<T>(what: impl Into<String>) -> Result<T, Malformed> {
    Err(Malformed(what.into().into_boxed_str()))
}

pub(crate) struct Enc<'a>(pub &'a mut Vec<u8>);

impl Enc<'_> {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn u128(&mut self, v: u128) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.u8(u8::from(v));
    }
    pub fn raw(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    pub fn str(&mut self, s: &str) {
        self.count(s.len());
        self.raw(s.as_bytes());
    }
    pub fn count(&mut self, n: usize) {
        self.u32(u32::try_from(n).expect("a sequence over 2^32 items"));
    }
    pub fn option<T>(&mut self, v: Option<&T>, f: impl FnOnce(&mut Self, &T)) {
        match v {
            None => self.u8(0),
            Some(v) => {
                self.u8(1);
                f(self, v)
            }
        }
    }
}

pub(crate) struct Dec<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Dec<'a> {
    pub fn new(buf: &'a [u8]) -> Dec<'a> {
        Dec { buf, at: 0 }
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Malformed> {
        if self.buf.len() - self.at < n {
            return bad(format!("{n} bytes wanted at offset {}, {} left", self.at, self.buf.len() - self.at));
        }
        let s = &self.buf[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }

    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], Malformed> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    pub fn u8(&mut self) -> Result<u8, Malformed> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32, Malformed> {
        Ok(u32::from_be_bytes(self.array()?))
    }
    pub fn u64(&mut self) -> Result<u64, Malformed> {
        Ok(u64::from_be_bytes(self.array()?))
    }
    pub fn u128(&mut self) -> Result<u128, Malformed> {
        Ok(u128::from_be_bytes(self.array()?))
    }

    pub fn bool(&mut self) -> Result<bool, Malformed> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            b => bad(format!("bool byte {b} at offset {}", self.at - 1)),
        }
    }

    /// A count no larger than `max`, so a forged count cannot make the decoder allocate.
    pub fn count(&mut self, max: usize) -> Result<usize, Malformed> {
        let n = self.u32()? as usize;
        if n > max {
            return bad(format!("count {n} over {max} at offset {}", self.at - 4));
        }
        Ok(n)
    }

    pub fn str(&mut self, max: usize) -> Result<&'a str, Malformed> {
        let n = self.count(max)?;
        let at = self.at;
        std::str::from_utf8(self.take(n)?).or_else(|_| bad(format!("invalid UTF-8 at offset {at}")))
    }

    pub fn option<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, Malformed>) -> Result<Option<T>, Malformed> {
        match self.u8()? {
            0 => Ok(None),
            1 => f(self).map(Some),
            b => bad(format!("option byte {b} at offset {}", self.at - 1)),
        }
    }

    pub fn finish(&self) -> Result<(), Malformed> {
        if self.at != self.buf.len() {
            return bad(format!("{} trailing bytes", self.buf.len() - self.at));
        }
        Ok(())
    }
}
