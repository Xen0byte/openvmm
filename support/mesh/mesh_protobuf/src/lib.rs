// Copyright (C) Microsoft Corporation. All rights reserved.

//! Low-level functionality to serializing and deserializing mesh messages.
//!
//! Most code won't use this directly but will instead use the
//! [`Protobuf`](derive@Protobuf) derive macro.
//!
//! The encoding used is a superset of
//! [protobuf](https://developers.google.com/protocol-buffers/docs/encoding),
//! allowing protobuf clients and mesh to interoperate for a subset of types.
//! This includes an optional extension to allow external resources such as file
//! descriptors to be referenced by messages.
//!
//! This is used instead of serde in order to serialize objects by value. The
//! serde API takes values by reference, which makes it difficult to support
//! values, such as ports, handles, and file descriptors, whose ownership is to
//! be transferred to the target node.

#![warn(missing_docs)]
// UNSAFETY: Serialization and deserialization of structs directly.
#![allow(unsafe_code)]

extern crate self as mesh_protobuf;

pub mod buffer;
mod encode_with;
pub mod encoding;
pub mod inplace;
pub mod message;
pub mod oneof;
#[cfg(feature = "prost")]
pub mod prost;
pub mod protobuf;
pub mod protofile;
pub mod table;
mod time;
pub mod transparent;

pub use encode_with::EncodeAs;
pub use mesh_derive::Protobuf;
pub use time::Timestamp;

use self::table::decode::DecoderEntry;
use self::table::encode::EncoderEntry;
use inplace::InplaceOption;
use protofile::DescribeMessage;
use protofile::MessageDescription;
use protofile::TypeUrl;
use std::cell::RefCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::num::Wrapping;

/// Associates the default encoder/decoder type for converting an object to/from
/// protobuf format.
pub trait DefaultEncoding {
    /// The encoding to use for the serialization.
    ///
    /// This type may or may not implement and of the four traits
    /// ([`MessageEncode`], [`MessageDecode`], [`FieldEncode`], [`FieldDecode`],
    /// since a type may only be serializable and not deserializable, for
    /// example.
    type Encoding;
}

/// Trait for types that can be encoded and decoded as a protobuf message.
pub trait Protobuf: DefaultEncoding<Encoding = <Self as Protobuf>::Encoding> + Sized {
    /// The default encoding for `Self`.
    type Encoding: MessageEncode<Self, NoResources>
        + for<'a> MessageDecode<'a, Self, NoResources>
        + FieldEncode<Self, NoResources>
        + for<'a> FieldDecode<'a, Self, NoResources>;
}

impl<T> Protobuf for T
where
    T: DefaultEncoding,
    T::Encoding: MessageEncode<T, NoResources>
        + for<'a> MessageDecode<'a, T, NoResources>
        + FieldEncode<T, NoResources>
        + for<'a> FieldDecode<'a, T, NoResources>,
{
    type Encoding = <T as DefaultEncoding>::Encoding;
}

/// Trait for types implementing [`Protobuf`] and having an associated protobuf
/// message description.
pub trait DescribedProtobuf: Protobuf {
    /// The message description.
    const DESCRIPTION: MessageDescription<'static>;
    /// The type URL for this message.
    const TYPE_URL: TypeUrl<'static> = Self::DESCRIPTION.type_url();
}

impl<T: DefaultEncoding + Protobuf> DescribedProtobuf for T
where
    <T as DefaultEncoding>::Encoding: DescribeMessage<T>,
{
    const DESCRIPTION: MessageDescription<'static> =
        <<T as DefaultEncoding>::Encoding as DescribeMessage<T>>::DESCRIPTION;
}

/// The `MessageEncode` trait provides a message encoder for type `T`.
///
/// `R` is the external resource type, which allows encoding objects with
/// non-protobuf resources such as file descriptors. Most implementors of this
/// trait will be generic over all `R`.
pub trait MessageEncode<T, R>: Sized {
    /// Writes `item` as a message.
    fn write_message(item: T, writer: protobuf::MessageWriter<'_, '_, R>);

    /// Computes the size of `item` as a message.
    ///
    /// Encoding will panic if the `write_message` call writes a different
    /// number of bytes than computed by this call.
    ///
    /// Takes a mut reference to allow mutating/stabilizing the value so that
    /// the subsequent call to `write_message` acts on the same value as this
    /// call.
    fn compute_message_size(item: &mut T, sizer: protobuf::MessageSizer<'_>);
}

/// The `MessageEncode` trait provides a message decoder for type `T`.
///
/// `R` is the external resource type, which allows decoding objects with
/// non-protobuf resources such as file descriptors. Most implementors of this
/// trait will be generic over all `R`.
pub trait MessageDecode<'a, T, R>: Sized {
    /// Reads a message into `item`.
    fn read_message(
        item: &mut InplaceOption<'_, T>,
        reader: protobuf::MessageReader<'a, '_, R>,
    ) -> Result<()>;
}

/// The `FieldEncode` trait provides a field encoder for type `T`.
///
/// `R` is the external resource type, which allows encoding objects with
/// non-protobuf resources such as file descriptors. Most implementors of this
/// trait will be generic over all `R`.
pub trait FieldEncode<T, R>: Sized {
    /// Writes `item` as a field.
    fn write_field(item: T, writer: protobuf::FieldWriter<'_, '_, R>);

    /// Computes the size of `item` as a field.
    ///
    /// Encoding will panic if the `write_field` call writes a different number
    /// of bytes than computed by this call.
    ///
    /// Takes a mut reference to allow mutating/stabilizing the value so that
    /// the subsequence call to `write_field` acts on the same value as this
    /// call.
    fn compute_field_size(item: &mut T, sizer: protobuf::FieldSizer<'_>);

    /// Returns the encoder for writing multiple instances of this field in a
    /// packed list, or `None` if there is no packed encoding for this type.
    fn packed<'a>() -> Option<&'a dyn PackedEncode<T>>
    where
        T: 'a,
    {
        None
    }

    /// Returns whether this field should be wrapped in a message when encoded
    /// nested in a sequence (such as a repeated field).
    ///
    /// This is necessary to avoid ambiguity between the repeated inner and
    /// outer values.
    fn wrap_in_sequence() -> bool {
        false
    }

    /// Writes this field as part of a sequence, wrapping it in a message if
    /// necessary.
    fn write_field_in_sequence(item: T, writer: &mut protobuf::SequenceWriter<'_, '_, R>) {
        if Self::wrap_in_sequence() {
            WrappedField::<Self>::write_field(item, writer.field())
        } else {
            Self::write_field(item, writer.field())
        }
    }

    /// Computes the size of this field as part of a sequence, including the
    /// size of a wrapping message.
    fn compute_field_size_in_sequence(item: &mut T, sizer: &mut protobuf::SequenceSizer<'_>) {
        if Self::wrap_in_sequence() {
            WrappedField::<Self>::compute_field_size(item, sizer.field())
        } else {
            Self::compute_field_size(item, sizer.field())
        }
    }

    /// The table encoder entry for this type, used in types from
    /// [`table::encode`].
    ///
    /// This should not be overridden by implementations.
    const ENTRY: EncoderEntry<T, R> = EncoderEntry::custom::<Self>();
}

/// Encoder methods for writing packed fields.
pub trait PackedEncode<T> {
    /// Writes a slice of data in packed format.
    fn write_packed(&self, data: &[T], writer: protobuf::PackedWriter<'_, '_>);

    /// Computes the size of the data in packed format.
    fn compute_packed_size(&self, data: &[T], sizer: protobuf::PackedSizer<'_>);

    /// If `true`, when this type is encoded as part of a sequence, it cannot be
    /// encoded with a normal repeated encoding and must be packed. This is used
    /// to determine if a nested repeated sequence needs to be wrapped in a
    /// message to avoid ambiguity.
    fn must_pack(&self) -> bool;
}

/// The `FieldEncode` trait provides a field decoder for type `T`.
///
/// `R` is the external resource type, which allows decoding objects with
/// non-protobuf resources such as file descriptors. Most implementors of this
/// trait will be generic over all `R`.
pub trait FieldDecode<'a, T, R>: Sized {
    /// Reads a field into `item`.
    fn read_field(
        item: &mut InplaceOption<'_, T>,
        reader: protobuf::FieldReader<'a, '_, R>,
    ) -> Result<()>;

    /// Instantiates `item` with its default value, if there is one.
    ///
    /// If an implementation returns `Ok(())`, then it must have set an item.
    /// Callers of this method may panic otherwise.
    fn default_field(item: &mut InplaceOption<'_, T>) -> Result<()>;

    /// Unless `packed()::must_pack()` is true, the sequence decoder must detect
    /// the encoding (packed or not) and call the appropriate method.
    fn packed<'p, C: CopyExtend<T>>() -> Option<&'p dyn PackedDecode<'a, T, C>>
    where
        T: 'p,
    {
        None
    }

    /// Returns whether this field is wrapped in a message when encoded nested
    /// in a sequence (such as a repeated field).
    fn wrap_in_sequence() -> bool {
        false
    }

    /// Reads this field that was encoded as part of a sequence, unwrapping it
    /// from a message if necessary.
    fn read_field_in_sequence(
        item: &mut InplaceOption<'_, T>,
        reader: protobuf::FieldReader<'a, '_, R>,
    ) -> Result<()> {
        if Self::wrap_in_sequence() {
            WrappedField::<Self>::read_field(item, reader)
        } else {
            Self::read_field(item, reader)
        }
    }

    /// The table decoder entry for this type, used in types from
    /// [`table::decode`].
    ///
    /// This should not be overridden by implementations.
    const ENTRY: DecoderEntry<'a, T, R> = DecoderEntry::custom::<Self>();
}

/// Methods for decoding a packed field.
pub trait PackedDecode<'a, T, C> {
    /// Reads from the packed format into `data`.
    fn read_packed(&self, data: &mut C, reader: &mut protobuf::PackedReader<'a>) -> Result<()>;

    /// If `true`, when this type is decoded as part of a sequence, it must be
    /// done with `read_packed` and not the field methods.
    fn must_pack(&self) -> bool;
}

/// Trait for collections that can be extended by a slice of `T: Copy`.
pub trait CopyExtend<T> {
    /// Pushes `item` onto the collection.
    fn push(&mut self, item: T)
    where
        T: Copy;

    /// Extends the collection by `items`.
    fn extend_from_slice(&mut self, items: &[T])
    where
        T: Copy;
}

impl<T> CopyExtend<T> for Vec<T> {
    fn push(&mut self, item: T)
    where
        T: Copy,
    {
        self.push(item);
    }

    fn extend_from_slice(&mut self, items: &[T])
    where
        T: Copy,
    {
        self.extend_from_slice(items);
    }
}

/// Encoder for a wrapper message used when a repeated field is directly nested
/// inside another repeated field.
struct WrappedField<E>(pub E);

impl<T, R, E: FieldEncode<T, R>> FieldEncode<T, R> for WrappedField<E> {
    fn write_field(item: T, writer: protobuf::FieldWriter<'_, '_, R>) {
        writer.message(|mut writer| E::write_field(item, writer.field(1)));
    }

    fn compute_field_size(item: &mut T, sizer: protobuf::FieldSizer<'_>) {
        sizer.message(|mut sizer| E::compute_field_size(item, sizer.field(1)));
    }
}

impl<'a, T, R, E: FieldDecode<'a, T, R>> FieldDecode<'a, T, R> for WrappedField<E> {
    fn read_field(
        item: &mut InplaceOption<'_, T>,
        reader: protobuf::FieldReader<'a, '_, R>,
    ) -> Result<()> {
        for field in reader.message()? {
            let (number, reader) = field?;
            if number == 1 {
                E::read_field(item, reader)?;
            }
        }
        if item.is_none() {
            E::default_field(item)?;
        }
        Ok(())
    }

    fn default_field(item: &mut InplaceOption<'_, T>) -> Result<()> {
        E::default_field(item)
    }
}

/// Encodes a message with its default encoding.
pub fn encode<T: DefaultEncoding>(message: T) -> Vec<u8>
where
    T::Encoding: MessageEncode<T, NoResources>,
{
    protobuf::Encoder::new(message).encode().0
}

/// Decodes a message with its default encoding.
pub fn decode<'a, T: DefaultEncoding>(data: &'a [u8]) -> Result<T>
where
    T::Encoding: MessageDecode<'a, T, NoResources>,
{
    inplace_none!(message: T);
    protobuf::decode_with::<T::Encoding, _, _>(&mut message, data, &mut [])?;
    Ok(message.take().expect("should be constructed"))
}

/// Merges message fields into an existing message.
pub fn merge<'a, T: DefaultEncoding>(value: T, data: &'a [u8]) -> Result<T>
where
    T::Encoding: MessageDecode<'a, T, NoResources>,
{
    inplace_some!(value);
    protobuf::decode_with::<T::Encoding, _, _>(&mut value, data, &mut [])?;
    Ok(value.take().expect("should be constructed"))
}

/// Marker trait indicating that an encoded value of `T` can be decoded as a
/// `Self`.
pub trait Downcast<T> {}

/// Marker trait indicating that an encoded value of `Self` can be decoded as a
/// `T`.
pub trait Upcast<T> {}

impl<T, U: Downcast<T>> Upcast<U> for T {}

/// An empty resources type, used when an encoding does not require any external
/// resources (such as files or mesh channels).
pub enum NoResources {}

/// A serialized message, consisting of binary data and a list
/// of resources.
#[derive(Debug)]
pub struct SerializedMessage<R = NoResources> {
    /// The message data.
    pub data: Vec<u8>,
    /// The message resources.
    pub resources: Vec<R>,
}

impl<R> Default for SerializedMessage<R> {
    fn default() -> Self {
        Self {
            data: Default::default(),
            resources: Default::default(),
        }
    }
}

impl<R> SerializedMessage<R> {
    /// Serializes a message.
    pub fn from_message<T: DefaultEncoding>(t: T) -> Self
    where
        T::Encoding: MessageEncode<T, R>,
    {
        let (data, resources) = protobuf::Encoder::new(t).encode();
        Self { data, resources }
    }

    /// Deserializes a message.
    pub fn into_message<T: DefaultEncoding>(self) -> Result<T>
    where
        T::Encoding: for<'a> MessageDecode<'a, T, R>,
    {
        let (data, mut resources) = self.prep_decode();
        inplace_none!(message: T);
        protobuf::decode_with::<T::Encoding, _, _>(&mut message, &data, &mut resources)?;
        Ok(message.take().expect("should be constructed"))
    }

    fn prep_decode(self) -> (Vec<u8>, Vec<Option<R>>) {
        let data = self.data;
        let resources = self.resources.into_iter().map(Some).collect();
        (data, resources)
    }
}

/// A decoding error.
#[derive(Debug)]
pub struct Error(Box<ErrorInner>);

#[derive(Debug)]
struct ErrorInner {
    types: Vec<&'static str>,
    err: Box<dyn std::error::Error + Send + Sync>,
}

/// The cause of a decoding error.
#[derive(Debug, thiserror::Error)]
enum DecodeError {
    #[error("expected a message")]
    ExpectedMessage,
    #[error("expected a resource")]
    ExpectedResource,
    #[error("expected a varint")]
    ExpectedVarInt,
    #[error("expected a fixed64")]
    ExpectedFixed64,
    #[error("expected a fixed32")]
    ExpectedFixed32,
    #[error("expected a byte array")]
    ExpectedByteArray,
    #[error("field cannot exist")]
    Unexpected,

    #[error("eof parsing a varint")]
    EofVarInt,
    #[error("eof parsing a fixed64")]
    EofFixed64,
    #[error("eof parsing a fixed32")]
    EofFixed32,
    #[error("eof parsing a byte array")]
    EofByteArray,

    #[error("varint too big")]
    VarIntTooBig,

    #[error("missing resource")]
    MissingResource,
    #[error("invalid resource range")]
    InvalidResourceRange,

    #[error("unknown wire type {0}")]
    UnknownWireType(u32),

    #[error("invalid UTF-32 character")]
    InvalidUtf32,
    #[error("wrong buffer size for u128")]
    BadU128,
    #[error("invalid UTF-8 string")]
    InvalidUtf8(#[source] std::str::Utf8Error),
    #[error("missing required field")]
    MissingRequiredField,
    #[error("wrong packed array length")]
    BadPackedArrayLength,
    #[error("wrong array length")]
    BadArrayLength,

    #[error("duration out of range")]
    DurationRange,
}

impl Error {
    /// Creates a new error.
    pub fn new(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(Box::new(ErrorInner {
            types: Vec::new(),
            err: error.into(),
        }))
    }

    /// Returns a new error with an additional type context added.
    pub fn typed<T>(mut self) -> Self {
        self.0.types.push(std::any::type_name::<T>());
        self
    }
}

impl From<DecodeError> for Error {
    fn from(kind: DecodeError) -> Self {
        Self(Box::new(ErrorInner {
            types: Vec::new(),
            err: kind.into(),
        }))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(&ty) = self.0.types.last() {
            write!(f, "decoding failed in {}", ty)?;
            for &ty in self.0.types.iter().rev().skip(1) {
                write!(f, "/{}", ty)?;
            }
            Ok(())
        } else {
            write!(f, "decoding failed")
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.err.as_ref())
    }
}

/// Extension trait to add type context to [`Error`].
pub trait ResultExt {
    /// Add type `T`'s name to the error.
    fn typed<T>(self) -> Self;
}

impl<T> ResultExt for Result<T> {
    fn typed<U>(self) -> Self {
        self.map_err(Error::typed::<U>)
    }
}

/// A decoding result.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::encode;
    use super::SerializedMessage;
    use crate::decode;
    use crate::encoding::BorrowedCowField;
    use crate::encoding::OwningCowField;
    use crate::encoding::VecField;
    use crate::DecodeError;
    use crate::FieldDecode;
    use crate::FieldEncode;
    use crate::NoResources;
    use mesh_derive::Protobuf;
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::collections::HashMap;
    use std::convert::Infallible;
    use std::error::Error;
    use std::num::NonZeroU32;
    use std::time::Duration;

    #[track_caller]
    fn assert_roundtrips<T>(t: T)
    where
        T: crate::DefaultEncoding + Clone + Eq + std::fmt::Debug,
        T::Encoding:
            crate::MessageEncode<T, NoResources> + for<'a> crate::MessageDecode<'a, T, NoResources>,
    {
        println!("{t:?}");
        let v = encode(t.clone());
        println!("{v:x?}");
        let t2 = decode::<T>(&v).unwrap();
        assert_eq!(t, t2);
    }

    #[track_caller]
    fn assert_field_roundtrips<T>(t: T)
    where
        T: crate::DefaultEncoding + Clone + Eq + std::fmt::Debug,
        T::Encoding: FieldEncode<T, NoResources> + for<'a> FieldDecode<'a, T, NoResources>,
    {
        assert_roundtrips((t,));
    }

    #[test]
    fn test_field() {
        assert_field_roundtrips(5u32);
        assert_field_roundtrips(true);
        assert_field_roundtrips("hi".to_string());
        assert_field_roundtrips(5u128);
        assert_field_roundtrips(());
        assert_field_roundtrips((1, 2));
        assert_field_roundtrips(("foo".to_string(), "bar".to_string()));
        assert_field_roundtrips([1, 2, 3]);
        assert_field_roundtrips(["abc".to_string(), "def".to_string()]);
        assert_field_roundtrips(Some(5));
        assert_field_roundtrips(Option::<u32>::None);
        assert_field_roundtrips(vec![1, 2, 3]);
        assert_field_roundtrips(vec!["abc".to_string(), "def".to_string()]);
        assert_field_roundtrips(Some(Some(true)));
        assert_field_roundtrips(Some(Option::<bool>::None));
        assert_field_roundtrips(vec![None, Some(true), None]);
        assert_field_roundtrips(HashMap::from_iter([(5u32, 6u32), (4, 2)]));
        assert_field_roundtrips(BTreeMap::from_iter([
            ("hi".to_owned(), 6u32),
            ("hmm".to_owned(), 2),
        ]));
    }

    #[test]
    fn test_nonzero() {
        assert_field_roundtrips(NonZeroU32::new(1).unwrap());
        assert_eq!(encode((5u32,)), encode((NonZeroU32::new(5).unwrap(),)));
        assert_eq!(
            decode::<(NonZeroU32,)>(&encode((Some(0u32),)))
                .unwrap_err()
                .source()
                .unwrap()
                .to_string(),
            "value must be non-zero"
        )
    }

    #[test]
    fn test_derive_struct() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            x: u32,
            y: u32,
            z: String,
            w: Option<bool>,
        }

        let foo = Foo {
            x: 5,
            y: 104824,
            z: "alphabet".to_owned(),
            w: None,
        };
        assert_roundtrips(foo);
    }

    #[test]
    fn test_nested_derive_struct() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            x: u32,
            y: u32,
            b: Option<Bar>,
        }

        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Bar {
            a: Option<bool>,
            b: u32,
        }

        let foo = Foo {
            x: 5,
            y: 104824,
            b: Some(Bar {
                a: Some(true),
                b: 5,
            }),
        };
        assert_roundtrips(foo);
    }

    #[test]
    fn test_derive_enum() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        enum Foo {
            A,
            B(u32, String),
            C { x: bool, y: u32 },
        }

        assert_roundtrips(Foo::A);
        assert_roundtrips(Foo::B(12, "hi".to_owned()));
        assert_roundtrips(Foo::C { x: true, y: 0 });
        assert_roundtrips(Foo::C { x: false, y: 0 });
    }

    #[test]
    fn test_vec() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            u32: Vec<u32>,
            u8: Vec<u8>,
            vec_no_pack: Vec<(u32,)>,
            vec_of_vec8: Vec<Vec<u8>>,
            vec_of_vec32: Vec<Vec<u32>>,
            vec_of_vec_no_pack: Vec<Vec<(u32,)>>,
        }

        let foo = Foo {
            u32: vec![1, 2, 3, 4, 5],
            u8: b"abcdefg".to_vec(),
            vec_no_pack: vec![(1,), (2,), (3,), (4,), (5,)],
            vec_of_vec8: vec![b"abc".to_vec(), b"def".to_vec()],
            vec_of_vec32: vec![vec![1, 2, 3], vec![4, 5, 6]],
            vec_of_vec_no_pack: vec![vec![(64,), (65,)], vec![(66,), (67,)]],
        };
        assert_roundtrips(foo);
    }

    struct NoPackU32;

    impl<R> FieldEncode<u32, R> for NoPackU32 {
        fn write_field(item: u32, writer: crate::protobuf::FieldWriter<'_, '_, R>) {
            writer.varint(item.into())
        }

        fn compute_field_size(item: &mut u32, sizer: crate::protobuf::FieldSizer<'_>) {
            sizer.varint((*item).into())
        }
    }

    impl<R> FieldDecode<'_, u32, R> for NoPackU32 {
        fn read_field(
            _item: &mut crate::inplace::InplaceOption<'_, u32>,
            _reader: crate::protobuf::FieldReader<'_, '_, R>,
        ) -> crate::Result<()> {
            unimplemented!()
        }

        fn default_field(_item: &mut crate::inplace::InplaceOption<'_, u32>) -> crate::Result<()> {
            unimplemented!()
        }
    }

    #[test]
    fn test_vec_alt() {
        {
            #[derive(Protobuf, Clone)]
            struct NoPack {
                #[mesh(encoding = "VecField<NoPackU32>")]
                v: Vec<u32>,
            }

            #[derive(Protobuf)]
            struct CanPack {
                v: Vec<u32>,
            }

            let no_pack = NoPack { v: vec![1, 2, 3] };
            let v = encode(no_pack.clone());
            println!("{v:x?}");
            let can_pack = decode::<CanPack>(&v).unwrap();
            assert_eq!(no_pack.v, can_pack.v);
        }

        {
            #[derive(Protobuf, Clone)]
            struct NoPackNest {
                #[mesh(encoding = "VecField<VecField<NoPackU32>>")]
                v: Vec<Vec<u32>>,
            }

            #[derive(Protobuf)]
            struct CanPackNest {
                v: Vec<Vec<u32>>,
            }

            let no_pack = NoPackNest {
                v: vec![vec![1, 2, 3], vec![4, 5, 6]],
            };
            let v = encode(no_pack.clone());
            println!("{v:x?}");
            let can_pack = decode::<CanPackNest>(&v).unwrap();
            assert_eq!(no_pack.v, can_pack.v);
        }
    }

    #[test]
    fn test_merge() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Bar(u32);

        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        enum Enum {
            A(u32),
            B(Option<u32>, Vec<u8>),
        }

        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            x: u32,
            y: u32,
            z: String,
            w: Option<bool>,
            v: Vec<u32>,
            v8: Vec<u8>,
            vb: Vec<Bar>,
            e: Enum,
        }

        let foo = Foo {
            x: 1,
            y: 2,
            z: "abc".to_string(),
            w: Some(true),
            v: vec![1, 2, 3],
            v8: b"xyz".to_vec(),
            vb: vec![Bar(1), Bar(2)],
            e: Enum::B(Some(1), b"abc".to_vec()),
        };
        assert_roundtrips(foo.clone());
        let foo2 = Foo {
            x: 3,
            y: 4,
            z: "def".to_string(),
            w: None,
            v: vec![4, 5, 6],
            v8: b"uvw".to_vec(),
            vb: vec![Bar(3), Bar(4), Bar(5)],
            e: Enum::B(None, b"def".to_vec()),
        };
        assert_roundtrips(foo2.clone());
        let foo3 = Foo {
            x: 3,
            y: 4,
            z: "def".to_string(),
            w: Some(true),
            v: vec![1, 2, 3, 4, 5, 6],
            v8: b"xyzuvw".to_vec(),
            vb: vec![Bar(1), Bar(2), Bar(3), Bar(4), Bar(5)],
            e: Enum::B(Some(1), b"abcdef".to_vec()),
        };
        assert_roundtrips(foo3.clone());
        let foo = super::merge(foo, &<SerializedMessage>::from_message(foo2).data).unwrap();
        assert_eq!(foo, foo3);
    }

    #[test]
    fn test_alternate_encoding() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            sint32: i32,
            #[mesh(encoding = "mesh_protobuf::encoding::VarintField")]
            int32: i32,
        }
        assert_roundtrips(Foo {
            int32: -1,
            sint32: -1,
        });
        assert_eq!(
            &encode(Foo {
                sint32: -1,
                int32: -1,
            }),
            &[8, 1, 16, 255, 255, 255, 255, 255, 255, 255, 255, 255, 1]
        );
    }

    #[test]
    fn test_array() {
        assert_field_roundtrips([1, 2, 3]);
        assert_field_roundtrips(["a".to_string(), "b".to_string(), "c".to_string()]);
        assert_field_roundtrips([vec![1, 2, 3], vec![4, 5, 6]]);
        assert_field_roundtrips([vec![1u8, 2]]);
        assert_field_roundtrips([[0_u8, 1], [2, 3]]);
        assert_field_roundtrips([Vec::<()>::new()]);
        assert_field_roundtrips([vec!["abc".to_string()]]);
    }

    #[test]
    fn test_nested() {
        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Nested<T> {
            pub n: u32,
            pub foo: T,
        }

        #[derive(Protobuf, Debug, Clone, PartialEq, Eq)]
        struct Foo {
            x: u32,
            y: u32,
            z: String,
            w: Option<bool>,
        }

        let t = Nested {
            n: 5,
            foo: Foo {
                x: 5,
                y: 104824,
                z: "alphabet".to_owned(),
                w: None,
            },
        };
        let t2: Nested<SerializedMessage> = SerializedMessage::from_message(t.clone())
            .into_message()
            .unwrap();
        let t3: Nested<Foo> = SerializedMessage::from_message(t2).into_message().unwrap();
        assert_eq!(t, t3);
    }

    #[test]
    fn test_lifetime() {
        #[derive(Protobuf)]
        struct Foo<'a>(&'a str);

        let s = String::from("foo");
        let v = encode(Foo(&s));
        let foo: Foo<'_> = decode(&v).unwrap();
        assert_eq!(foo.0, &s);
    }

    #[test]
    fn test_generic_lifetime() {
        #[derive(Protobuf)]
        struct Foo<T>(T);

        let s = String::from("foo");
        let v = encode(Foo(s.as_str()));
        let foo: Foo<&str> = decode(&v).unwrap();
        assert_eq!(foo.0, &s);
    }

    #[test]
    fn test_infallible() {
        assert!(matches!(
            decode::<Infallible>(&[])
                .unwrap_err()
                .source()
                .unwrap()
                .downcast_ref::<DecodeError>(),
            Some(DecodeError::Unexpected)
        ));
    }

    #[test]
    fn test_empty_message() {
        #[derive(Protobuf)]
        struct Message(u32);

        let v = encode(((Message(0),),));
        assert_eq!(&v, b"");

        let _message: ((Message,),) = decode(&[]).unwrap();
    }

    #[test]
    fn test_nested_empty_message() {
        #[derive(Debug, Clone, PartialEq, Eq, Protobuf)]
        struct Message(Outer, Inner);

        #[derive(Debug, Default, Clone, PartialEq, Eq, Protobuf)]
        struct Outer(Inner);

        #[derive(Debug, Default, Clone, PartialEq, Eq, Protobuf)]
        struct Inner(u32);

        assert_roundtrips(Message(Default::default(), Inner(1)));
    }

    #[test]
    fn test_transparent_message() {
        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        struct Inner(u32);

        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        #[mesh(transparent)]
        struct TupleStruct(Inner);

        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        #[mesh(transparent)]
        struct NamedStruct {
            x: Inner,
        }

        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        #[mesh(transparent)]
        struct GenericStruct<T>(T);

        assert_roundtrips(TupleStruct(Inner(5)));
        assert_eq!(encode(TupleStruct(Inner(5))), encode(Inner(5)));
        assert_eq!(encode(NamedStruct { x: Inner(5) }), encode(Inner(5)));
        assert_eq!(encode(GenericStruct(Inner(5))), encode(Inner(5)));
    }

    #[test]
    fn test_transparent_field() {
        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        #[mesh(transparent)]
        struct Inner(u32);

        #[derive(Protobuf, Copy, Clone, PartialEq, Eq, Debug)]
        struct Outer<T>(T);

        assert_roundtrips(Outer(Inner(5)));
        assert_eq!(encode(Outer(Inner(5))), encode(Outer(5u32)));
    }

    #[test]
    fn test_transparent_enum() {
        #[derive(Protobuf, Clone, PartialEq, Eq, Debug)]
        enum Foo {
            #[mesh(transparent)]
            Bar(u32),
            #[mesh(transparent)]
            Option(Option<u32>),
            #[mesh(transparent)]
            Vec(Vec<u32>),
            #[mesh(transparent)]
            VecNoPack(Vec<(u32,)>),
        }

        assert_roundtrips(Foo::Bar(0));
        assert_eq!(encode(Foo::Bar(0)), encode((Some(0),)));
        assert_roundtrips(Foo::Option(Some(5)));
        assert_roundtrips(Foo::Option(None));
        assert_roundtrips(Foo::Vec(vec![]));
        assert_roundtrips(Foo::Vec(vec![5]));
        assert_roundtrips(Foo::VecNoPack(vec![(5,)]));
    }

    #[test]
    fn test_cow() {
        #[derive(Protobuf)]
        struct OwnedString<'a>(#[mesh(encoding = "OwningCowField")] Cow<'a, str>);
        #[derive(Protobuf)]
        struct BorrowedString<'a>(#[mesh(encoding = "BorrowedCowField")] Cow<'a, str>);
        #[derive(Protobuf)]
        struct OwnedBytes<'a>(#[mesh(encoding = "OwningCowField")] Cow<'a, [u8]>);
        #[derive(Protobuf)]
        struct BorrowedBytes<'a>(#[mesh(encoding = "BorrowedCowField")] Cow<'a, [u8]>);

        let s_owning: OwnedString<'_>;
        let v_owning: OwnedBytes<'_>;

        {
            let b = encode(("abc",));
            let mut b2 = b.clone();
            b2.extend(encode(("def",)));

            let s_borrowed: BorrowedString<'_>;
            let v_borrowed: BorrowedBytes<'_>;
            let v_borrowed2: BorrowedBytes<'_>;
            {
                let (s,): (String,) = decode(&b2).unwrap();
                assert_eq!(&s, "def");
                let (v,): (Vec<u8>,) = decode(&b2).unwrap();
                assert_eq!(&v, b"abcdef");

                s_owning = decode(&b2).unwrap();
                let s_owning = s_owning.0;
                assert!(matches!(s_owning, Cow::Owned(_)));
                assert_eq!(s_owning.as_ref(), "def");

                s_borrowed = decode(&b2).unwrap();
                let s_borrowed = s_borrowed.0;
                assert!(matches!(s_borrowed, Cow::Borrowed(_)));
                assert_eq!(s_borrowed.as_ref(), "def");

                v_owning = decode(&b2).unwrap();
                let v_owning = v_owning.0;
                assert!(matches!(v_owning, Cow::Owned(_)));
                assert_eq!(v_owning.as_ref(), b"abcdef");

                v_borrowed = decode(&b).unwrap();
                let v_borrowed = v_borrowed.0;
                assert!(matches!(v_borrowed, Cow::Borrowed(_)));
                assert_eq!(v_borrowed.as_ref(), b"abc");

                // This one is owned because it has to append more data.
                v_borrowed2 = decode(&b2).unwrap();
                let v_borrowed2 = v_borrowed2.0;
                assert!(matches!(v_borrowed2, Cow::Owned(_)));
                assert_eq!(v_borrowed2.as_ref(), b"abcdef");
            }
        }
    }

    #[test]
    fn test_duration() {
        assert_roundtrips(Duration::ZERO);
        assert_roundtrips(Duration::from_secs(1));
        assert_roundtrips(Duration::from_secs(1) + Duration::from_nanos(10000));
        assert_roundtrips(Duration::from_secs(1) - Duration::from_nanos(10000));
        decode::<Duration>(&encode((-1i64 as u64, 0u32))).unwrap_err();
        assert_eq!(
            decode::<Duration>(&encode((1u64, 1u32))).unwrap(),
            Duration::from_secs(1) + Duration::from_nanos(1)
        );
    }

    #[test]
    fn test_failure_recovery() {
        let m = encode(("foo", 2, 3));
        decode::<(String, String, String)>(&m).unwrap_err();
    }
}