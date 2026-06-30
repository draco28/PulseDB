//! Vendored, **decode-only** reproduction of the `bincode 1.3` `DefaultOptions`
//! wire format (VS-4.0.3 / work-1.01).
//!
//! # Why this exists
//!
//! The serializer migration (VS-4.0.3 / 1.04) must read every pre-4.0 on-disk
//! serde-blob value, which was encoded with **bincode 1.3 default config**, and
//! re-encode it to postcard. But the `bincode` *crate* must leave the dependency
//! tree entirely so the `RUSTSEC-2025-0141` `deny.toml` ignore can drop (1.06).
//! This module is the single read-old chokepoint: a minimal [`serde::Deserializer`]
//! that decodes the bincode-1.3 layout **without** any dependency on the `bincode`
//! crate. It carries **no call into the bincode crate** — including in its tests
//! (frozen byte constants below), so it survives the 1.06 crate drop and proves the
//! "no maintained-crate dependency" property. (AC-5 greps this file for the
//! `bincode` path-call token and requires zero matches — keep prose call-free.)
//!
//! Scope: **decode-only**. There is no `Serializer` and no vendored encoder —
//! migration only ever *writes* postcard (design §4).
//!
//! # bincode 1.3 `DefaultOptions` wire format (the rules this module reproduces)
//!
//! The bincode crate's serialize/deserialize entry points use `DefaultOptions`:
//!
//! - **Fixed-width little-endian integers** (NOT varints): `u8/u16/u32/u64`,
//!   `i8/i16/i32/i64`, `f32/f64` are raw LE bytes of their natural width.
//! - **`u64` length prefix** (8 bytes LE) precedes every `Vec`/`String`/`map`/
//!   `bytes`/`seq` whose length is serialized.
//! - **Enum variant index = `u32`** (4 bytes LE). The `#[repr(u8)]` on the tag
//!   enums (`ExperienceTypeTag` etc.) is **irrelevant on the wire** — serde
//!   encodes the variant *position index*, and bincode writes it as `u32`.
//! - **`Option` = a single tag byte** (`0` = `None`, `1` = `Some` + payload).
//! - **`usize` → `u64`** (8 bytes; e.g. `EmbeddingDimension::Custom(usize)`).
//! - **`char` → `u32`**; `bool` → 1 byte (`0`/`1`).
//! - **Reject trailing bytes**: a decode that leaves unconsumed input is an error
//!   (matches bincode `DefaultOptions`, which rejects trailing input).
//!
//! ## The #1 silent-corruption gotcha — UUID = 24 bytes on disk
//!
//! Every ID newtype (`CollectiveId`, `ExperienceId`, …) wraps `uuid::Uuid`, and
//! `uuid` is pulled with `features = ["serde"]`. Under a **non-human-readable**
//! serializer, `Uuid` serializes via `serialize_bytes(self.as_bytes())`, which
//! bincode encodes as a **`u64` length-prefix (= 16) + 16 raw bytes = 24 bytes**,
//! NOT 16. So [`LegacyDeserializer::deserialize_bytes`] reads a `u64` length, then
//! that many bytes. A bare 16-byte read would silently misalign every later field.
//!
//! # Oracle provenance (how the frozen test constants were produced)
//!
//! See the `tests` module: the `*_GOLDEN` byte constants were produced **once** by
//! a throwaway generator (`examples/oracle_gen.rs`, deleted before commit) that
//! called the bincode crate's `serialize` entry point (= `DefaultOptions` fixint
//! LE — explicitly **NOT** the crate's `config::legacy()` varint, which differs on
//! the wire) against the *real production types*. The exact regen procedure is
//! documented beside the constants. One fixture (`COLLECTIVE_GOLDEN`) is
//! hand-verified byte-by-byte in a comment. The constants are `pub(crate)` so
//! VS-4.0.3/1.04's `{v3, bincode}` migration fixture reuses them rather than
//! re-freezing (and so 1.04 never re-invokes the bincode serializer, which would
//! fail 1.06's `src/` grep).

// This module is purely ADDITIVE in work-1.01: it delivers the vendored decoder
// and its golden tests, but the production read path still uses the bincode crate's
// deserialize until VS-4.0.3/1.04 cuts over to `decode`. So `decode`, `LegacyBincodeError`, and
// the internal deserializer are exercised only by the `#[cfg(test)]` golden tests
// here until 1.04 wires the real caller. Allow dead_code at the module scope so the
// `-D warnings` clippy gate (AC-6) passes; 1.04 will consume the public surface.
#![allow(dead_code)]

use std::fmt;

use serde::de::{
    self, DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};

// ============================================================================
// Error type — recoverable, typed, never panics
// ============================================================================

/// Error returned by the vendored legacy-bincode decode path.
///
/// Decode is **fail-safe**: it never panics and never reads past the end of the
/// input buffer. A truncated buffer yields [`LegacyBincodeError::Eof`]; leftover
/// input after a complete value yields [`LegacyBincodeError::TrailingBytes`].
///
/// The error is **recoverable** by design: the decay-config read path tries
/// `StoredDecayConfig` then falls back to `StoredDecayConfigV1` on a decode error
/// (`redb.rs`), so 1.04 mirrors that try-current-then-legacy pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LegacyBincodeError {
    /// The input ended before a full value could be decoded.
    Eof,
    /// A complete value was decoded but unconsumed bytes remain.
    TrailingBytes,
    /// An enum variant index, bool, or char byte was not a valid value.
    InvalidValue(String),
    /// A length prefix exceeded the remaining input (avoids huge allocations).
    LengthOverflow,
    /// A `str`/`char` field contained invalid UTF-8.
    InvalidUtf8,
    /// A serde-driven error (custom message, or an unsupported type was hit).
    Message(String),
}

impl fmt::Display for LegacyBincodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Eof => write!(f, "unexpected end of input while decoding legacy bincode"),
            Self::TrailingBytes => write!(f, "trailing bytes after decoding legacy bincode value"),
            Self::InvalidValue(m) => write!(f, "invalid legacy bincode value: {m}"),
            Self::LengthOverflow => write!(f, "legacy bincode length prefix exceeds input"),
            Self::InvalidUtf8 => write!(f, "invalid UTF-8 in legacy bincode value"),
            Self::Message(m) => write!(f, "legacy bincode decode error: {m}"),
        }
    }
}

impl std::error::Error for LegacyBincodeError {}

impl de::Error for LegacyBincodeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Message(msg.to_string())
    }
}

type Result<T> = std::result::Result<T, LegacyBincodeError>;

// ============================================================================
// Public decode entry point
// ============================================================================

/// Decodes a `bincode 1.3 DefaultOptions`-encoded value into `T`.
///
/// Returns a typed, recoverable [`LegacyBincodeError`] on any malformed input
/// (truncation, trailing bytes, invalid enum index, bad UTF-8) — never panics.
pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let mut de = LegacyDeserializer::new(bytes);
    let value = T::deserialize(&mut de)?;
    if de.input.is_empty() {
        Ok(value)
    } else {
        Err(LegacyBincodeError::TrailingBytes)
    }
}

// ============================================================================
// The Deserializer
// ============================================================================

struct LegacyDeserializer<'de> {
    input: &'de [u8],
}

impl<'de> LegacyDeserializer<'de> {
    fn new(input: &'de [u8]) -> Self {
        Self { input }
    }

    fn take(&mut self, n: usize) -> Result<&'de [u8]> {
        if self.input.len() < n {
            return Err(LegacyBincodeError::Eof);
        }
        let (head, tail) = self.input.split_at(n);
        self.input = tail;
        Ok(head)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    /// Reads a `u64` length prefix and validates it fits in the remaining input.
    ///
    /// Validating up-front avoids a huge speculative allocation when a corrupt
    /// length claims more bytes than exist (e.g. a `Vec::with_capacity` blowup).
    fn read_len(&mut self) -> Result<usize> {
        let len = self.read_u64()?;
        let len = usize::try_from(len).map_err(|_| LegacyBincodeError::LengthOverflow)?;
        if len > self.input.len() {
            return Err(LegacyBincodeError::LengthOverflow);
        }
        Ok(len)
    }
}

// serde's data model dispatches through these methods. bincode is NOT
// self-describing, so `deserialize_any` is impossible — every method reads its
// fixed layout directly.
impl<'de> de::Deserializer<'de> for &mut LegacyDeserializer<'de> {
    type Error = LegacyBincodeError;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        Err(LegacyBincodeError::Message(
            "legacy bincode is not self-describing; deserialize_any is unsupported".into(),
        ))
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.read_u8()? {
            0 => visitor.visit_bool(false),
            1 => visitor.visit_bool(true),
            n => Err(LegacyBincodeError::InvalidValue(format!("bool byte {n}"))),
        }
    }

    fn deserialize_i8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_i8(self.read_u8()? as i8)
    }

    fn deserialize_i16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_i16(self.read_u16()? as i16)
    }

    fn deserialize_i32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_i32(self.read_u32()? as i32)
    }

    fn deserialize_i64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_i64(self.read_u64()? as i64)
    }

    fn deserialize_u8<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_u8(self.read_u8()?)
    }

    fn deserialize_u16<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_u16(self.read_u16()?)
    }

    fn deserialize_u32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_u32(self.read_u32()?)
    }

    fn deserialize_u64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_u64(self.read_u64()?)
    }

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_f32(f32::from_bits(self.read_u32()?))
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_f64(f64::from_bits(self.read_u64()?))
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let code = self.read_u32()?;
        let c = char::from_u32(code)
            .ok_or_else(|| LegacyBincodeError::InvalidValue(format!("char code {code}")))?;
        visitor.visit_char(c)
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let len = self.read_len()?;
        let bytes = self.take(len)?;
        let s = std::str::from_utf8(bytes).map_err(|_| LegacyBincodeError::InvalidUtf8)?;
        visitor.visit_borrowed_str(s)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_str(visitor)
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        // The 24-byte-UUID path: a `u64` length prefix then that many raw bytes.
        let len = self.read_len()?;
        let bytes = self.take(len)?;
        visitor.visit_borrowed_bytes(bytes)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.read_u8()? {
            0 => visitor.visit_none(),
            1 => visitor.visit_some(self),
            n => Err(LegacyBincodeError::InvalidValue(format!("option tag {n}"))),
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value> {
        // A newtype struct (e.g. `CollectiveId(Uuid)`) is transparent on the wire.
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let len = self.read_len()?;
        visitor.visit_seq(CountedAccess::new(self, len))
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        // Tuples have a statically-known length — no length prefix on the wire.
        visitor.visit_seq(CountedAccess::new(self, len))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_seq(CountedAccess::new(self, len))
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        let len = self.read_len()?;
        visitor.visit_map(CountedAccess::new(self, len))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        // A struct is a fixed-length, unprefixed sequence of its (non-skipped)
        // fields, in declaration order. serde's derive does not visit `#[serde(skip)]`
        // fields, so they consume no wire bytes — matching bincode exactly.
        visitor.visit_seq(CountedAccess::new(self, fields.len()))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        // Variant index is a `u32` (load-bearing — NOT the `#[repr(u8)]` width).
        let variant_index = self.read_u32()?;
        visitor.visit_enum(EnumDeserializer {
            de: self,
            variant_index,
        })
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        // bincode never serializes field/variant names — identifiers are unreachable.
        Err(LegacyBincodeError::Message(
            "legacy bincode does not encode identifiers".into(),
        ))
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value> {
        // We cannot skip an unknown field in a non-self-describing format.
        Err(LegacyBincodeError::Message(
            "cannot skip unknown field in non-self-describing legacy bincode".into(),
        ))
    }

    fn is_human_readable(&self) -> bool {
        false
    }
}

// ============================================================================
// Seq / map / struct element access
// ============================================================================

/// Drives a fixed number of element decodes for seq/tuple/struct/map bodies.
struct CountedAccess<'a, 'de: 'a> {
    de: &'a mut LegacyDeserializer<'de>,
    remaining: usize,
}

impl<'a, 'de> CountedAccess<'a, 'de> {
    fn new(de: &'a mut LegacyDeserializer<'de>, len: usize) -> Self {
        Self { de, remaining: len }
    }
}

impl<'de, 'a> SeqAccess<'de> for CountedAccess<'a, 'de> {
    type Error = LegacyBincodeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

impl<'de, 'a> MapAccess<'de> for CountedAccess<'a, 'de> {
    type Error = LegacyBincodeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn next_value_seed<Vv: DeserializeSeed<'de>>(&mut self, seed: Vv) -> Result<Vv::Value> {
        seed.deserialize(&mut *self.de)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining)
    }
}

// ============================================================================
// Enum access
// ============================================================================

struct EnumDeserializer<'a, 'de: 'a> {
    de: &'a mut LegacyDeserializer<'de>,
    variant_index: u32,
}

impl<'de, 'a> EnumAccess<'de> for EnumDeserializer<'a, 'de> {
    type Error = LegacyBincodeError;
    type Variant = VariantDeserializer<'a, 'de>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant)> {
        // serde identifies the variant by its index; feed the u32 through a u32
        // deserializer so the derived `Visitor::visit_u32` selects the variant.
        let variant = seed.deserialize(self.variant_index.into_deserializer())?;
        Ok((variant, VariantDeserializer { de: self.de }))
    }
}

struct VariantDeserializer<'a, 'de: 'a> {
    de: &'a mut LegacyDeserializer<'de>,
}

impl<'de, 'a> VariantAccess<'de> for VariantDeserializer<'a, 'de> {
    type Error = LegacyBincodeError;

    fn unit_variant(self) -> Result<()> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        seed.deserialize(&mut *self.de)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        visitor.visit_seq(CountedAccess::new(self.de, len))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_seq(CountedAccess::new(self.de, fields.len()))
    }
}

#[cfg(test)]
mod tests;
