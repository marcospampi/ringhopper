//! Functionality for parsing tags as whole as well as tag files.

use byteorder::ByteOrder;

use crate::primitive::*;
use crate::accessor::*;
use crate::parse::*;
use crate::error::*;
use crate::crc32::CRC32;

use std::any::Any;

/// Used for defining information for saving structs into tag files.
pub trait PrimaryTagStruct: TagDataAccessor + TagData {
    /// Get the tag group of the tag struct.
    fn group() -> TagGroup where Self: Sized;

    /// Get the version of the tag struct's tag file.
    fn version() -> u16 where Self: Sized;
}

/// Methods automatically implemented for all [`PrimaryTagStruct`] types that implement [`Any`].
pub trait PrimaryTagStructDyn: PrimaryTagStruct + Any {
    /// Get this as a [`TagDataAccessor`] to allow for accessing tag data without needing the underlying structure.
    fn as_accessor(&self) -> &dyn TagDataAccessor;

    /// Get this as a mutable [`TagDataAccessor`] to allow for accessing tag data without needing the underlying structure.
    fn as_mut_accessor(&mut self) -> &mut dyn TagDataAccessor;

    /// Get this as an [`Any`] reference to downcast.
    fn as_any(&self) -> &dyn Any;

    /// Get this as a mutable [`Any`] reference to downcast.
    fn as_mut_any(&mut self) -> &mut dyn Any;

    /// Convert this to a tag file.
    ///
    /// See [`TagFile::to_tag_file`] for information.
    fn to_tag_file(&self) -> RinghopperResult<Vec<u8>>;

    /// Get the tag group of this tag.
    fn group(&self) -> TagGroup;
}

impl dyn PrimaryTagStructDyn {
    /// Get a reference to the tag as a concrete type.
    ///
    /// Returns `None` if it does not match the expected type.
    ///
    /// Convenience function for `.as_any().downcast_ref::<T>()` with some extra compile-time checks.
    pub fn get_ref<T: PrimaryTagStructDyn>(&self) -> Option<&T> {
        self.as_any().downcast_ref::<T>()
    }

    /// Get a mutable reference to the tag as a concrete type.
    ///
    /// Returns `None` if it does not match the expected type.
    ///
    /// Convenience function for `.as_mut_any().downcast_mut::<T>()` with some extra compile-time checks.
    pub fn get_mut<T: PrimaryTagStructDyn>(&mut self) -> Option<&mut T> {
        self.as_mut_any().downcast_mut::<T>()
    }
}

impl<T: PrimaryTagStruct + Any> PrimaryTagStructDyn for T {
    fn as_accessor(&self) -> &dyn TagDataAccessor {
        self
    }
    fn as_mut_accessor(&mut self) -> &mut dyn TagDataAccessor {
        self
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_mut_any(&mut self) -> &mut dyn Any {
        self
    }
    fn to_tag_file(&self) -> RinghopperResult<Vec<u8>> {
        TagFile::to_tag_file(self)
    }
    fn group(&self) -> TagGroup {
        <T as PrimaryTagStruct>::group()
    }
}

/// If CRC32 is set to this, then disable CRC32 checks.
pub const IGNORED_CRC32: u32 = 0xFFFFFFFF;

/// Required to be set for the tag header.
pub const BLAM_FOURCC: u32 = 0x626C616D;

/// Structure for identifying and validating a tag file, stored at the beginning of all tag files.
#[derive(Copy, Clone, PartialEq, Default)]
#[repr(C)]
pub struct TagFileHeader {
    /// Some ID value; not set on most tags, and always unused.
    pub id: ID,

    /// Some string value; not set on most tags, and always unused.
    pub string: String32,

    /// Tag group of the tag.
    pub group: TagGroup,

    /// CRC32 of all data following the header; set to [`IGNORED_CRC32`] to disable checks.
    pub crc32: u32,

    /// Size of the header. Equal to 0x40.
    pub header_size: u32,

    /// Unused.
    pub padding: Padding<[u8; 8]>,

    /// Version of the tag struct.
    pub version: u16,

    /// Always set to 255.
    pub u16_255: u16,

    /// `blam` FourCC for identifying that it is a tag file; must be equal to [`BLAM_FOURCC`].
    pub blam_fourcc: u32,
}

impl TagDataSimplePrimitive for TagFileHeader {
    fn size() -> usize {
        const _: () = assert!(std::mem::size_of::<TagFileHeader>() == 0x40);
        std::mem::size_of::<TagFileHeader>()
    }
    fn write<B: ByteOrder>(&self, data: &mut [u8], at: usize, struct_end: usize) -> RinghopperResult<()> {
        if self.id.is_null() {
            0u32.write::<B>(data, at + 0x00, struct_end)?;
        }
        else {
            self.id.write::<B>(data, at + 0x00, struct_end)?;
        }
        self.string.write::<B>(data, at + 0x04, struct_end)?;
        self.group.write::<B>(data, at + 0x24, struct_end)?;
        self.crc32.write::<B>(data, at + 0x28, struct_end)?;
        self.header_size.write::<B>(data, at + 0x2C, struct_end)?;
        self.padding.write::<B>(data, at + 0x30, struct_end)?;
        self.version.write::<B>(data, at + 0x38, struct_end)?;
        self.u16_255.write::<B>(data, at + 0x3A, struct_end)?;
        self.blam_fourcc.write::<B>(data, at + 0x3C, struct_end)?;
        Ok(())
    }
    fn read<B: ByteOrder>(data: &[u8], at: usize, struct_end: usize) -> RinghopperResult<Self> {
        Ok(Self {
            id: ID::read::<B>(data, at + 0x00, struct_end).unwrap_or(ID::null()),
            string: String32::read::<B>(data, at + 0x04, struct_end)?,
            group: TagGroup::read::<B>(data, at + 0x24, struct_end)?,
            crc32: u32::read::<B>(data, at + 0x28, struct_end)?,
            header_size: u32::read::<B>(data, at + 0x2C, struct_end)?,
            padding: Padding::<[u8; 8]>::read::<B>(data, at + 0x30, struct_end)?,
            version: u16::read::<B>(data, at + 0x38, struct_end)?,
            u16_255: u16::read::<B>(data, at + 0x3A, struct_end)?,
            blam_fourcc: u32::read::<B>(data, at + 0x3C, struct_end)?,
        })
    }
}

impl TagFileHeader {
    /// Return `Ok(())` if the header is valid.
    ///
    /// Return `Err(Error::TagParseFailure)` if the header is not valid.
    pub fn validate(&self) -> RinghopperResult<()> {
        if self.blam_fourcc == BLAM_FOURCC && self.u16_255 == 0x00FF && self.header_size as usize == <Self as TagDataSimplePrimitive>::size() {
            Ok(())
        }
        else {
            Err(Error::TagParseFailure)
        }
    }

    /// Return `Ok(())` if the group is correct.
    ///
    /// Returns `Err(Error::TagHeaderGroupTypeMismatch)` if the type is wrong, and `Err(Error::TagHeaderGroupVersionMismatch)` if the
    /// version is wrong but the type is correct.
    pub fn verify_group_matches<T: PrimaryTagStruct>(&self) -> RinghopperResult<()> {
        if self.group != T::group() {
            return Err(Error::TagHeaderGroupTypeMismatch)
        }
        if self.version != T::version() {
            return Err(Error::TagHeaderGroupVersionMismatch)
        }
        Ok(())
    }
}

/// Determine how strict parsing should be.
///
/// In all cases, out-of-bounds data cannot be parsed. However, allowing data with bad checksums to be parsed can be enabled, if desired.
#[derive(Copy, Clone, Default)]
pub enum ParseStrictness {
    /// Require the CRC32 to match; should be used by default.
    #[default]
    Strict,

    /// Allow the CRC32 to mismatch; should only be used if accessing potentially broken tags is necessary.
    Relaxed,
}

/// Methods for handling tag files.
pub struct TagFile {}

impl TagFile {
    /// Encode the tag data into tag file format.
    ///
    /// This returns a serialized byte array that can be loaded back with [`TagFile::read_tag_from_file_buffer`] or with other tools and libraries when stored as a file.
    ///
    /// Returns `Err` if the tag is unable to be represented in tag format, such as if 32-bit array limits are exceeded.
    pub fn to_tag_file<T: PrimaryTagStruct>(tag_data: &T) -> RinghopperResult<Vec<u8>> {
        let header_len = <TagFileHeader as TagData>::size();
        let tag_file_len = <T as TagData>::size();
        let capacity = header_len + tag_file_len;

        let mut data = Vec::new();
        data.resize(capacity, 0);
        tag_data.write_to_tag_file(&mut data, header_len, capacity)?;

        let mut crc32 = CRC32::new();
        crc32.update(&data[header_len..]);
        let new_header = TagFileHeader {
            blam_fourcc: BLAM_FOURCC,
            crc32: crc32.crc(),
            group: T::group(),
            header_size: header_len as u32,
            version: T::version(),
            u16_255: 0x00FF,
            ..Default::default()
        };
        new_header.write_to_tag_file(&mut data, 0, header_len).expect("writing the tag file header should always work");

        Ok(data)
    }

    fn validate_crc32(header: &TagFileHeader, data_after_header: &[u8], strictness: ParseStrictness) -> RinghopperResult<()> {
        let actual_crc32 = if header.crc32 == IGNORED_CRC32 {
            None
        }
        else {
            let mut crc32 = CRC32::new();
            crc32.update(data_after_header);
            Some(crc32.crc())
        };

        match strictness {
            ParseStrictness::Relaxed => (),
            ParseStrictness::Strict => {
                if let Some(c) = actual_crc32 {
                    if c != header.crc32 {
                        return Err(Error::ChecksumMismatch)
                    }
                }
            }
        }

        Ok(())
    }

    /// Returns the header and everything after the header without parsing the actual tag  data.
    ///
    /// Return `Err` if parsing the header fails.
    pub fn load_header_and_data(file: &[u8], strictness: ParseStrictness) -> RinghopperResult<(TagFileHeader, &[u8])> {
        let header = TagFileHeader::read_from_tag_file(file, 0, 0x40, &mut 0)?;
        header.validate()?;

        let data_after_header = &file[<TagFileHeader as TagData>::size()..];
        Self::validate_crc32(&header, data_after_header, strictness)?;

        Ok((header, data_after_header))
    }

    /// Read the tag file buffer.
    ///
    /// Returns `Err` if the tag data is invalid, corrupt, or does not correspond to `T`.
    pub fn read_tag_from_file_buffer<T: PrimaryTagStruct>(file: &[u8], strictness: ParseStrictness) -> RinghopperResult<T> {
        let (header, data_after_header) = Self::load_header_and_data(file, strictness)?;
        header.verify_group_matches::<T>()?;

        let mut cursor = T::size();
        let result = T::read_from_tag_file(data_after_header, 0, T::size(), &mut cursor)?;
        Ok(result)
    }
}

#[cfg(test)]
mod test;
