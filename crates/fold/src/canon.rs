//! `crates/raft`'s canonical encoding rule (`graftyard_raft::Enc`) as a serde `Serializer`, so a stored-format type
//! derives `Serialize` and encodes byte for byte as a hand-written `Record::encode` would: fixed-width big-endian
//! integers, a string or byte sequence as its `u32` length and then its bytes, a sequence as its `u32` count and then
//! each item, an `Option` as 0 or 1 and then the value, an enum as a `u8` tag in declaration order and then its fields,
//! a struct or tuple as its fields in order. Floats are refused: no stored format has one.

use serde::Serialize;
use serde::ser;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct CanonError(String);

impl ser::Error for CanonError {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        CanonError(msg.to_string())
    }
}

/// Appends the canonical bytes of `v`. Panics on a type the rule does not cover (a float, an unsized sequence, or an
/// enum of more than 256 variants), which is a bug in a stored-format type, not in its data.
pub fn write<T: Serialize + ?Sized>(out: &mut Vec<u8>, v: &T) {
    v.serialize(&mut Canon(out)).expect("a stored-format type the canonical rule covers");
}

pub fn to_vec<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out, v);
    out
}

struct Canon<'a>(&'a mut Vec<u8>);

impl Canon<'_> {
    fn len(&mut self, n: usize) -> Result<(), CanonError> {
        let n = u32::try_from(n).map_err(|_| CanonError("a length over 2^32".into()))?;
        self.0.extend_from_slice(&n.to_be_bytes());
        Ok(())
    }
    fn tag(&mut self, i: u32) -> Result<(), CanonError> {
        self.0.push(u8::try_from(i).map_err(|_| CanonError("an enum of more than 256 variants".into()))?);
        Ok(())
    }
}

type Res = Result<(), CanonError>;

impl<'a, 'b> ser::Serializer for &'a mut Canon<'b> {
    type Ok = ();
    type Error = CanonError;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    /// A compact binary form: a type with a text form for JSON (`graftyard_identity::SessionId`) keeps its derived
    /// form here, as postcard does.
    fn is_human_readable(&self) -> bool {
        false
    }

    fn serialize_bool(self, v: bool) -> Res {
        self.0.push(u8::from(v));
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i128(self, v: i128) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> Res {
        self.0.push(v);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u128(self, v: u128) -> Res {
        self.0.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_f32(self, _: f32) -> Res {
        Err(CanonError("a float in a stored format".into()))
    }
    fn serialize_f64(self, _: f64) -> Res {
        Err(CanonError("a float in a stored format".into()))
    }
    fn serialize_char(self, v: char) -> Res {
        self.serialize_u32(u32::from(v))
    }
    fn serialize_str(self, v: &str) -> Res {
        self.serialize_bytes(v.as_bytes())
    }
    fn serialize_bytes(self, v: &[u8]) -> Res {
        self.len(v.len())?;
        self.0.extend_from_slice(v);
        Ok(())
    }
    fn serialize_none(self) -> Res {
        self.0.push(0);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Res {
        self.0.push(1);
        v.serialize(self)
    }
    fn serialize_unit(self) -> Res {
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Res {
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, i: u32, _: &'static str) -> Res {
        self.tag(i)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Res {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, i: u32, _: &'static str, v: &T) -> Res {
        self.tag(i)?;
        v.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self, CanonError> {
        self.len(len.ok_or_else(|| CanonError("a sequence of unknown length".into()))?)?;
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, CanonError> {
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, CanonError> {
        Ok(self)
    }
    fn serialize_tuple_variant(self, _: &'static str, i: u32, _: &'static str, _: usize) -> Result<Self, CanonError> {
        self.tag(i)?;
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, CanonError> {
        self.len(len.ok_or_else(|| CanonError("a map of unknown length".into()))?)?;
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, CanonError> {
        Ok(self)
    }
    fn serialize_struct_variant(self, _: &'static str, i: u32, _: &'static str, _: usize) -> Result<Self, CanonError> {
        self.tag(i)?;
        Ok(self)
    }
}

macro_rules! compound {
    ($($t:ident :: $f:ident),*) => {$(
        impl<'a, 'b> ser::$t for &'a mut Canon<'b> {
            type Ok = ();
            type Error = CanonError;
            fn $f<T: Serialize + ?Sized>(&mut self, v: &T) -> Res {
                v.serialize(&mut **self)
            }
            fn end(self) -> Res {
                Ok(())
            }
        }
    )*};
}
compound!(SerializeSeq::serialize_element, SerializeTuple::serialize_element, SerializeTupleStruct::serialize_field);
compound!(SerializeTupleVariant::serialize_field);

impl<'a, 'b> ser::SerializeMap for &'a mut Canon<'b> {
    type Ok = ();
    type Error = CanonError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Res {
        k.serialize(&mut **self)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Res {
        v.serialize(&mut **self)
    }
    fn end(self) -> Res {
        Ok(())
    }
}

impl<'a, 'b> ser::SerializeStruct for &'a mut Canon<'b> {
    type Ok = ();
    type Error = CanonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, _: &'static str, v: &T) -> Res {
        v.serialize(&mut **self)
    }
    fn end(self) -> Res {
        Ok(())
    }
}

impl<'a, 'b> ser::SerializeStructVariant for &'a mut Canon<'b> {
    type Ok = ();
    type Error = CanonError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, _: &'static str, v: &T) -> Res {
        v.serialize(&mut **self)
    }
    fn end(self) -> Res {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use graftyard_raft::Enc;
    use serde::Serialize;

    #[derive(Serialize)]
    enum E {
        A,
        B { n: u32, s: Box<str>, o: Option<u64>, v: Vec<u8>, a: [u8; 3] },
    }

    #[test]
    fn matches_the_hand_written_enc_rule() {
        let mut want = Vec::new();
        let mut e = Enc(&mut want);
        e.u8(1);
        e.u32(7);
        e.str("hi");
        e.option(Some(&9u64), |e, v| e.u64(*v));
        e.bytes(&[1, 2]);
        e.0.extend_from_slice(&[4, 5, 6]);
        let got = super::to_vec(&E::B { n: 7, s: "hi".into(), o: Some(9), v: vec![1, 2], a: [4, 5, 6] });
        assert_eq!(got, want);
        assert_eq!(super::to_vec(&E::A), vec![0]);
        assert_eq!(super::to_vec(&None::<u8>), vec![0]);
    }
}
