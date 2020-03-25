use std::borrow::Cow;
use std::io::{Read, Result as IoResult, Write};

use byteorder::{BigEndian, ByteOrder, LittleEndian, ReadBytesExt};
use byteorder::WriteBytesExt;

use crate::Endianness;
use crate::errors::PcapError;
use crate::pcapng::blocks::{EnhancedPacketBlock, InterfaceDescriptionBlock, InterfaceStatisticsBlock, NameResolutionBlock, SectionHeaderBlock, SimplePacketBlock, SystemdJournalExportBlock};
use crate::pcapng::{PacketBlock, UnknownBlock};

use derive_into_owned::IntoOwned;

//   0               1               2               3
//   0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7 0 1 2 3 4 5 6 7
//  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//  |                          Block Type                           |
//  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//  |                      Block Total Length                       |
//  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//  /                          Block Body                           /
//  /          /* variable length, aligned to 32 bits */            /
//  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
//  |                      Block Total Length                       |
//  +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
/// PcapNg Block
#[derive(Clone, Debug)]
pub struct Block<'a> {
    pub type_: BlockType,
    pub initial_len: u32,
    pub body: Cow<'a, [u8]>,
    pub trailer_len: u32,
    pub(crate) endianness: Endianness
}

impl<'a> Block<'a> {

    /// Create an "owned" `Block` from a reader
    pub(crate) fn from_reader<R:Read, B: ByteOrder>(reader: &mut R) -> Result<Block<'static>, PcapError> {

        let type_ = reader.read_u32::<B>()?.into();

        //Special case for the section header because we don't know the endianness yet
        if type_ == BlockType::SectionHeader {
            let mut initial_len = reader.read_u32::<BigEndian>()?;
            let magic = reader.read_u32::<BigEndian>()?;

            let endianness = match magic {
                0x1A2B3C4D => Endianness::Big,
                0x4D3C2B1A => Endianness::Little,
                _ => return Err(PcapError::InvalidField("SectionHeaderBlock: invalid magic number"))
            };

            if endianness == Endianness::Little {
                initial_len = initial_len.swap_bytes();
            }

            if (initial_len % 4) != 0 {
                return Err(PcapError::InvalidField("Block: (initial_len % 4) != 0"));
            }

            if initial_len < 12 {
                return Err(PcapError::InvalidField("Block: initial_len < 12"))
            }

            let body_len = initial_len - 12;
            let mut body = vec![0_u8; body_len as usize];
            // Rewrite the magic in the body
            (&mut body[..]).write_u32::<BigEndian>(magic)?;
            reader.read_exact(&mut body[4..])?;

            let trailer_len = match endianness {
                Endianness::Big => reader.read_u32::<BigEndian>()?,
                Endianness::Little => reader.read_u32::<LittleEndian>()?
            };

            if initial_len != trailer_len {
                return Err(PcapError::InvalidField("Block initial_length != trailer_length"))
            }

            Ok(
                Block {
                    type_,
                    initial_len,
                    body: Cow::Owned(body),
                    trailer_len,
                    endianness
                }
            )
        }
        else {

            //Common case
            let initial_len = reader.read_u32::<B>()?;
            if (initial_len % 4) != 0 {
                return Err(PcapError::InvalidField("Block: (initial_len % 4) != 0"));
            }

            if initial_len < 12 {
                return Err(PcapError::InvalidField("Block: initial_len < 12"))
            }

            let body_len = initial_len - 12;
            let mut body = vec![0_u8; body_len as usize];
            reader.read_exact(&mut body[..])?;

            let trailer_len = reader.read_u32::<B>()?;
            if initial_len != trailer_len {
                return Err(PcapError::InvalidField("Block initial_length != trailer_length"))
            }

            Ok(
                Block {
                    type_,
                    initial_len,
                    body: Cow::Owned(body),
                    trailer_len,
                    endianness: Endianness::new::<B>()
                }
            )
        }
    }

    /// Create an "borrowed" `Block` from a slice
    pub(crate) fn from_slice<B: ByteOrder>(mut slice: &'a[u8]) -> Result<(&[u8], Self), PcapError> {

        if slice.len() < 12 {
            return Err(PcapError::IncompleteBuffer(12 - slice.len()));
        }

        let type_ = slice.read_u32::<B>()?.into();

        //Special case for the section header because we don't know the endianness yet
        if type_ == BlockType::SectionHeader {
            let mut initial_len = slice.read_u32::<BigEndian>()?;

            let mut tmp_slice = slice;

            let magic = tmp_slice.read_u32::<BigEndian>()?;

            let endianness = match magic {
                0x1A2B3C4D => Endianness::Big,
                0x4D3C2B1A => Endianness::Little,
                _ => return Err(PcapError::InvalidField("SectionHeaderBlock: invalid magic number"))
            };

            if endianness == Endianness::Little {
                initial_len = initial_len.swap_bytes();
            }

            if (initial_len % 4) != 0 {
                return Err(PcapError::InvalidField("Block: (initial_len % 4) != 0"));
            }

            if initial_len < 12 {
                return Err(PcapError::InvalidField("Block: initial_len < 12"))
            }

            //Check if there is enough data for the body and the trailer_len
            if slice.len() < initial_len as usize - 8 {
                return Err(PcapError::IncompleteBuffer(initial_len as usize - 8 - slice.len()));
            }

            let body_len = initial_len - 12;
            let body = &slice[..body_len as usize];

            let mut rem = &slice[body_len as usize ..];

            let trailer_len = match endianness {
                Endianness::Big => rem.read_u32::<BigEndian>()?,
                Endianness::Little => rem.read_u32::<LittleEndian>()?
            };

            if initial_len != trailer_len {
                return Err(PcapError::InvalidField("Block initial_length != trailer_length"))
            }


            let block = Block {
                type_,
                initial_len,
                body: Cow::Borrowed(body),
                trailer_len,
                endianness
            };

            return Ok((rem, block))
        }
        else {

            //Common case
            let initial_len = slice.read_u32::<B>()?;

            if (initial_len % 4) != 0 {
                return Err(PcapError::InvalidField("Block: (initial_len % 4) != 0"));
            }

            if initial_len < 12 {
                return Err(PcapError::InvalidField("Block: initial_len < 12"))
            }

            //Check if there is enough data for the body and the trailer_len
            if slice.len() < initial_len as usize - 8 {
                return Err(PcapError::IncompleteBuffer(initial_len as usize - 8 - slice.len()));
            }

            let body_len = initial_len - 12;
            let body = &slice[..body_len as usize];

            let mut rem = &slice[body_len as usize ..];

            let trailer_len = rem.read_u32::<B>()?;

            if initial_len != trailer_len {
                return Err(PcapError::InvalidField("Block initial_length != trailer_length"))
            }

            let block = Block {
                type_,
                initial_len,
                body: Cow::Borrowed(body),
                trailer_len,
                endianness: Endianness::new::<B>()
            };

            Ok((rem, block))
        }
    }

    pub fn parsed(&self) -> Result<ParsedBlock, PcapError> {

        match self.endianness {
            Endianness::Big => ParsedBlock::from_slice::<BigEndian>(self.type_, &self.body).map(|r| r.1),
            Endianness::Little => ParsedBlock::from_slice::<LittleEndian>(self.type_, &self.body).map(|r| r.1)
        }
    }

    pub fn write_to<B:ByteOrder, W: Write>(&self, writer: &mut W) -> IoResult<usize> {

        writer.write_u32::<B>(self.type_.into())?;
        writer.write_u32::<B>(self.initial_len)?;
        writer.write(&self.body[..])?;
        writer.write_u32::<B>(self.trailer_len)?;

        Ok(12 + self.body.len())
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BlockType {
    SectionHeader,
    InterfaceDescription,
    Packet,
    SimplePacket,
    NameResolution,
    InterfaceStatistics,
    EnhancedPacket,
    SystemdJournalExport,
    Unknown(u32)
}

impl From<u32> for BlockType {
    fn from(src: u32) -> Self {
        match src {
            0x0A0D0D0A => BlockType::SectionHeader,
            0x00000001 => BlockType::InterfaceDescription,
            0x00000002 => BlockType::Packet,
            0x00000003 => BlockType::SimplePacket,
            0x00000004 => BlockType::NameResolution,
            0x00000005 => BlockType::InterfaceStatistics,
            0x00000006 => BlockType::EnhancedPacket,
            0x00000009 => BlockType::SystemdJournalExport,
            _ => BlockType::Unknown(src),
        }
    }
}

impl Into<u32> for BlockType {
    fn into(self) -> u32 {
        match self {
            BlockType::SectionHeader => 0x0A0D0D0A,
            BlockType::InterfaceDescription => 0x00000001,
            BlockType::Packet => 0x00000002,
            BlockType::SimplePacket => 0x00000003,
            BlockType::NameResolution => 0x00000004,
            BlockType::InterfaceStatistics => 0x00000005,
            BlockType::EnhancedPacket => 0x00000006,
            BlockType::SystemdJournalExport => 0x00000009,
            BlockType::Unknown(c) => c,
        }
    }
}

/// PcapNg parsed blocks
#[derive(Clone, Debug, IntoOwned, Eq, PartialEq)]
pub enum ParsedBlock<'a> {
    SectionHeader(SectionHeaderBlock<'a>),
    InterfaceDescription(InterfaceDescriptionBlock<'a>),
    Packet(PacketBlock<'a>),
    SimplePacket(SimplePacketBlock<'a>),
    NameResolution(NameResolutionBlock<'a>),
    InterfaceStatistics(InterfaceStatisticsBlock<'a>),
    EnhancedPacket(EnhancedPacketBlock<'a>),
    SystemdJournalExport(SystemdJournalExportBlock<'a>),
    Unknown(UnknownBlock<'a>)
}

impl<'a> ParsedBlock<'a> {

    /// Create a `ParsedBlock` from a slice
    pub fn from_slice<B: ByteOrder>(type_: BlockType, slice: &'a[u8]) -> Result<(&'a [u8], Self), PcapError> {

        match type_ {

            BlockType::SectionHeader => {
                let (rem, block) = SectionHeaderBlock::from_slice::<BigEndian>(slice)?;
                Ok((rem, ParsedBlock::SectionHeader(block)))
            },
            BlockType::InterfaceDescription => {
                let (rem, block) = InterfaceDescriptionBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::InterfaceDescription(block)))
            },
            BlockType::Packet => {
                let (rem, block) = PacketBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::Packet(block)))
            },
            BlockType::SimplePacket => {
                let (rem, block) = SimplePacketBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::SimplePacket(block)))
            },
            BlockType::NameResolution => {
                let (rem, block) = NameResolutionBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::NameResolution(block)))
            },
            BlockType::InterfaceStatistics => {
                let (rem, block) = InterfaceStatisticsBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::InterfaceStatistics(block)))
            },
            BlockType::EnhancedPacket => {
                let (rem, block) = EnhancedPacketBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::EnhancedPacket(block)))
            },
            BlockType::SystemdJournalExport => {
                let (rem, block) = SystemdJournalExportBlock::from_slice::<B>(slice)?;
                Ok((rem, ParsedBlock::SystemdJournalExport(block)))
            }
            _ => Ok((&[], ParsedBlock::Unknown(UnknownBlock::new(type_, slice.len() as u32 + 12, slice))))
        }
    }

    pub fn into_enhanced_packet(self) -> Option<EnhancedPacketBlock<'a>> {
        match self {
            ParsedBlock::EnhancedPacket(a) => Some(a),
            _ => None
        }
    }

    pub fn into_interface_description(self) -> Option<InterfaceDescriptionBlock<'a>> {
        match self {
            ParsedBlock::InterfaceDescription(a) => Some(a),
            _ => None
        }
    }

    pub fn into_interface_statistics(self) -> Option<InterfaceStatisticsBlock<'a>> {
        match self {
            ParsedBlock::InterfaceStatistics(a) => Some(a),
            _ => None
        }
    }

    pub fn into_name_resolution(self) -> Option<NameResolutionBlock<'a>> {
        match self {
            ParsedBlock::NameResolution(a) => Some(a),
            _ => None
        }
    }

    pub fn into_packet(self) -> Option<PacketBlock<'a>> {
        match self {
            ParsedBlock::Packet(a) => Some(a),
            _ => None
        }
    }

    pub fn into_section_header(self) -> Option<SectionHeaderBlock<'a>> {
        match self {
            ParsedBlock::SectionHeader(a) => Some(a),
            _ => None
        }
    }

    pub fn into_simple_packet(self) -> Option<SimplePacketBlock<'a>> {
        match self {
            ParsedBlock::SimplePacket(a) => Some(a),
            _ => None
        }
    }

    pub fn into_systemd_journal_export(self) -> Option<SystemdJournalExportBlock<'a>> {
        match self {
            ParsedBlock::SystemdJournalExport(a) => Some(a),
            _ => None
        }
    }
}

pub(crate) trait PcapNgBlock<'a> {

    const BLOCK_TYPE: BlockType;

    fn from_slice<B: ByteOrder>(slice: &'a [u8]) -> Result<(&[u8], Self), PcapError> where Self: std::marker::Sized;
    fn write_to<B: ByteOrder, W: Write>(&self, writer: &mut W) -> IoResult<usize>;

    fn write_block_to<B: ByteOrder, W: Write>(&self, writer: &mut W) -> IoResult<usize> {

        let len = self.write_to::<B, _>(&mut std::io::sink()).unwrap() + 12;

        writer.write_u32::<B>( Self::BLOCK_TYPE.into())?;
        writer.write_u32::<B>(len as u32)?;
        self.write_to::<B, _>(writer)?;
        writer.write_u32::<B>(len as u32)?;

        Ok(len)
    }

    fn into_parsed(self) -> ParsedBlock<'a>;
}


