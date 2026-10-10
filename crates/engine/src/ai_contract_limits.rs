//! Allocation-free validation before serializing host-owned native objects for graph discovery.
use super::{ContractError, ContractResult};
use serde::ser::{self, Serialize, Serializer};

#[derive(Debug)]
pub(super) struct Error;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("native resource limit")
    }
}
impl std::error::Error for Error {}
impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}

pub(super) fn check<T: Serialize>(value: &T) -> ContractResult<()> {
    let mut budget = Budget { nodes: 0, bytes: 0, depth: 0 };
    value.serialize(&mut budget).map_err(|_| ContractError::Limit("native resource size/depth/numbers"))
}
struct Budget {
    nodes: usize,
    bytes: usize,
    depth: usize,
}
impl Budget {
    fn scalar(&mut self, bytes: usize) -> Result<(), Error> {
        self.nodes = self.nodes.checked_add(1).ok_or(Error)?;
        self.bytes = self.bytes.checked_add(bytes).ok_or(Error)?;
        if self.nodes > 100_000 || self.bytes > 2 * 1024 * 1024 {
            return Err(Error);
        }
        Ok(())
    }
    fn compound(&mut self, len: Option<usize>) -> Result<Compound<'_>, Error> {
        if len.is_some_and(|n| n > 4096) || self.depth >= 32 {
            return Err(Error);
        }
        self.scalar(8)?;
        self.depth += 1;
        Ok(Compound(self))
    }
}
struct Compound<'a>(&'a mut Budget);
impl Drop for Compound<'_> {
    fn drop(&mut self) {
        self.0.depth = self.0.depth.saturating_sub(1);
    }
}
impl<'a> Serializer for &'a mut Budget {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;
    fn serialize_bool(self, _: bool) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_i8(self, _: i8) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_i16(self, _: i16) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_i32(self, _: i32) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_i64(self, _: i64) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_i128(self, _: i128) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_u8(self, _: u8) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_u16(self, _: u16) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_u32(self, _: u32) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_u64(self, _: u64) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_u128(self, _: u128) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_char(self, _: char) -> Result<(), Error> {
        self.scalar(32)
    }
    fn serialize_f32(self, v: f32) -> Result<(), Error> {
        if !v.is_finite() || v.abs() > 1.0e9 {
            return Err(Error);
        }
        self.scalar(32)
    }
    fn serialize_f64(self, v: f64) -> Result<(), Error> {
        if !v.is_finite() || v.abs() > 1.0e9 {
            return Err(Error);
        }
        self.scalar(32)
    }
    fn serialize_str(self, v: &str) -> Result<(), Error> {
        if v.len() > 128 * 1024 {
            return Err(Error);
        }
        self.scalar(v.len().checked_mul(6).ok_or(Error)?)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> {
        self.scalar(v.len())
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.scalar(4)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<(), Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.scalar(4)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Error> {
        self.scalar(4)
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, v: &'static str) -> Result<(), Error> {
        self.serialize_str(v)
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, v: &T) -> Result<(), Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _: &'static str, _: u32, name: &'static str, v: &T) -> Result<(), Error> {
        self.serialize_str(name)?;
        v.serialize(self)
    }
    fn serialize_seq(self, n: Option<usize>) -> Result<Compound<'a>, Error> {
        self.compound(n)
    }
    fn serialize_tuple(self, n: usize) -> Result<Compound<'a>, Error> {
        self.compound(Some(n))
    }
    fn serialize_tuple_struct(self, _: &'static str, n: usize) -> Result<Compound<'a>, Error> {
        self.compound(Some(n))
    }
    fn serialize_tuple_variant(self, _: &'static str, _: u32, name: &'static str, n: usize) -> Result<Compound<'a>, Error> {
        self.serialize_str(name)?;
        self.compound(Some(n))
    }
    fn serialize_map(self, n: Option<usize>) -> Result<Compound<'a>, Error> {
        self.compound(n)
    }
    fn serialize_struct(self, _: &'static str, n: usize) -> Result<Compound<'a>, Error> {
        self.compound(Some(n))
    }
    fn serialize_struct_variant(self, _: &'static str, _: u32, name: &'static str, n: usize) -> Result<Compound<'a>, Error> {
        self.serialize_str(name)?;
        self.compound(Some(n))
    }
}
impl ser::SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), Error> {
        self.0.serialize_str(key)?;
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
impl ser::SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, v: &T) -> Result<(), Error> {
        self.0.serialize_str(key)?;
        v.serialize(&mut *self.0)
    }
    fn end(self) -> Result<(), Error> {
        Ok(())
    }
}
