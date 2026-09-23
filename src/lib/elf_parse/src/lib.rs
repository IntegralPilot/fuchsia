// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Parses ELF files per the ELF specification in ulib/musl/include/elf.h
use bitflags::bitflags;

use num_derive::FromPrimitive;
use num_traits::cast::FromPrimitive;
use static_assertions::assert_eq_size;
use std::{fmt, mem};
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Possible errors that can occur during ELF parsing.
#[derive(Error, Debug)]
pub enum ElfParseError {
    #[error("Failed to read ELF from VMO: {}", _0)]
    ReadError(zx::Status),
    #[error("Parse error: {}", _0)]
    ParseError(&'static str),
    #[error("Invalid ELF file header: {}", _0)]
    InvalidFileHeader(&'static str),
    #[error("Invalid ELF program header: {}", _0)]
    InvalidProgramHeader(&'static str),
    #[error("Multiple ELF program headers of type {} present", _0)]
    MultipleHeaders(SegmentType),
}

impl ElfParseError {
    /// Returns an appropriate zx::Status code for the given error.
    pub fn as_zx_status(&self) -> zx::Status {
        match self {
            ElfParseError::ReadError(s) => *s,
            // Not a great status to return for an invalid ELF but there's no great fit, and this
            // matches elf_load.
            ElfParseError::ParseError(_)
            | ElfParseError::InvalidFileHeader(_)
            | ElfParseError::InvalidProgramHeader(_) => zx::Status::NOT_FOUND,
            ElfParseError::MultipleHeaders(_) => zx::Status::NOT_FOUND,
        }
    }
}

trait Validate {
    fn validate(&self) -> Result<(), ElfParseError>;
}

/// ELF identity header.
#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Eq, PartialEq, Default, Clone, Copy)]
#[repr(C)]
pub struct ElfIdent {
    /// e_ident[EI_MAG0:EI_MAG3]
    pub magic: [u8; 4],
    /// e_ident[EI_CLASS]
    pub class: u8,
    /// e_ident[EI_DATA]
    pub data: u8,
    /// e_ident[EI_VERSION]
    pub version: u8,
    /// e_ident[EI_OSABI]
    pub osabi: u8,
    /// e_ident[EI_ABIVERSION]
    pub abiversion: u8,
    /// e_ident[EI_PAD]
    pub pad: [u8; 7],
}

#[allow(unused)]
const EI_NIDENT: usize = 16;
assert_eq_size!(ElfIdent, [u8; EI_NIDENT]);

/// ELF class, from EI_CLASS.
#[derive(Debug, FromPrimitive, Eq, PartialEq)]
#[repr(u8)]
pub enum ElfClass {
    /// ELFCLASSNONE
    Unknown = 0,
    /// ELFCLASS32
    Elf32 = 1,
    /// ELFCLASS64
    Elf64 = 2,
}

/// ELF data encoding, from EI_DATA.
#[derive(Debug, FromPrimitive, Eq, PartialEq)]
#[repr(u8)]
pub enum ElfDataEncoding {
    /// ELFDATANONE
    Unknown = 0,
    /// ELFDATA2LSB
    LittleEndian = 1,
    /// ELFDATA2MSB
    BigEndian = 2,
}

/// ELF version, from EI_VERSION.
#[derive(Debug, FromPrimitive, Eq, PartialEq)]
#[repr(u8)]
pub enum ElfVersion {
    /// EV_NONE
    Unknown = 0,
    /// EV_CURRENT
    Current = 1,
}

impl ElfIdent {
    pub fn from_vmo(vmo: &zx::Vmo) -> Result<ElfIdent, ElfParseError> {
        // Read and parse the ELF file header from the VMO.
        let field = vmo.read_to_object(0).map_err(|s| ElfParseError::ReadError(s))?;
        Ok(field)
    }

    pub fn class(&self) -> Result<ElfClass, u8> {
        ElfClass::from_u8(self.class).ok_or(self.class)
    }

    pub fn is_arch32(&self) -> bool {
        self.class() == Ok(ElfClass::Elf32)
    }

    pub fn data(&self) -> Result<ElfDataEncoding, u8> {
        ElfDataEncoding::from_u8(self.data).ok_or(self.data)
    }

    pub fn version(&self) -> Result<ElfVersion, u8> {
        ElfVersion::from_u8(self.version).ok_or(self.version)
    }
}

impl std::fmt::Debug for ElfIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("ElfIdent");
        s.field("magic", &self.magic);

        match self.class() {
            Ok(c) => s.field("class", &c),
            Err(unknown) => s.field("class", &unknown),
        };
        match self.data() {
            Ok(d) => s.field("data", &d),
            Err(unknown) => s.field("data", &unknown),
        };
        match self.version() {
            Ok(v) => s.field("version", &v),
            Err(unknown) => s.field("version", &unknown),
        };

        s.field("osabi", &self.osabi).field("abiversion", &self.abiversion).finish()
    }
}

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Eq, PartialEq, Default, Clone, Copy)]
#[repr(C)]
pub struct Elf64FileHeader {
    pub ident: ElfIdent,
    pub elf_type: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: usize,
    pub phoff: usize,
    pub shoff: usize,
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Eq, PartialEq, Default, Clone, Copy)]
#[repr(C)]
pub struct Elf32FileHeader {
    pub ident: ElfIdent,
    pub elf_type: u16,
    pub machine: u16,
    pub version: u32,
    pub entry: u32, // These three are the only differently sized entries.
    pub phoff: u32, // <--
    pub shoff: u32, // <--
    pub flags: u32,
    pub ehsize: u16,
    pub phentsize: u16,
    pub phnum: u16,
    pub shentsize: u16,
    pub shnum: u16,
    pub shstrndx: u16,
}

#[derive(FromPrimitive, Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum ElfType {
    /// ET_NONE
    Unknown = 0,
    /// ET_REL
    Relocatable = 1,
    /// ET_EXEC
    Executable = 2,
    /// ET_DYN
    SharedObject = 3,
    /// ET_CORE
    Core = 4,
}

#[derive(FromPrimitive, Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ElfArchitecture {
    /// EM_NONE
    Unknown = 0,
    /// EM_386
    I386 = 3,
    /// EM_ARM
    ARM = 40,
    /// EM_X86_64
    X86_64 = 62,
    /// EM_AARCH64
    AARCH64 = 183,
    /// EM_RISCV
    RISCV = 243,
}

pub const ELF_MAGIC: [u8; 4] = *b"\x7fELF";

#[cfg(target_endian = "little")]
pub const NATIVE_ENCODING: ElfDataEncoding = ElfDataEncoding::LittleEndian;
#[cfg(target_endian = "big")]
pub const NATIVE_ENCODING: ElfDataEncoding = ElfDataEncoding::BigEndian;

#[cfg(target_arch = "x86_64")]
pub const CURRENT_ARCH: ElfArchitecture = ElfArchitecture::X86_64;
#[cfg(target_arch = "x86_64")]
pub const ARCH32_ARCH: ElfArchitecture = ElfArchitecture::Unknown;

#[cfg(target_arch = "aarch64")]
pub const CURRENT_ARCH: ElfArchitecture = ElfArchitecture::AARCH64;
#[cfg(target_arch = "aarch64")]
pub const ARCH32_ARCH: ElfArchitecture = ElfArchitecture::ARM;

#[cfg(target_arch = "riscv64")]
pub const CURRENT_ARCH: ElfArchitecture = ElfArchitecture::RISCV;
#[cfg(target_arch = "riscv64")]
pub const ARCH32_ARCH: ElfArchitecture = ElfArchitecture::Unknown;

impl Elf64FileHeader {
    pub fn elf_type(&self) -> Result<ElfType, u16> {
        ElfType::from_u16(self.elf_type).ok_or(self.elf_type)
    }

    pub fn machine(&self) -> Result<ElfArchitecture, u16> {
        ElfArchitecture::from_u16(self.machine).ok_or(self.machine)
    }

    pub fn from_vmo(vmo: &zx::Vmo) -> Result<Box<Elf64FileHeader>, ElfParseError> {
        // Read and parse the ELF file header from the VMO.
        let mut header = Box::<Elf64FileHeader>::default();
        vmo.read(header.as_mut_bytes(), 0).map_err(|s| ElfParseError::ReadError(s))?;
        header.validate()?;
        Ok(header)
    }
}

impl std::fmt::Debug for Elf64FileHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Elf64FileHeader");
        s.field("ident", &self.ident);

        match self.elf_type() {
            Ok(t) => s.field("elf_type", &t),
            Err(unknown) => s.field("elf_type", &unknown),
        };
        match self.machine() {
            Ok(m) => s.field("machine", &m),
            Err(unknown) => s.field("machine", &unknown),
        };

        s.field("version", &self.version)
            .field("entry", &format_args!("{:#x}", self.entry))
            .field("phoff", &format_args!("{:#x}", self.phoff))
            .field("shoff", &format_args!("{:#x}", self.shoff))
            .field("flags", &format_args!("{:#x}", self.flags))
            .field("ehsize", &self.ehsize)
            .field("phentsize", &self.phentsize)
            .field("phnum", &self.phnum)
            .field("shentsize", &self.shentsize)
            .field("shnum", &self.shnum)
            .field("shstrndx", &self.shstrndx)
            .finish()
    }
}

impl Elf32FileHeader {
    pub fn elf_type(&self) -> Result<ElfType, u16> {
        ElfType::from_u16(self.elf_type).ok_or(self.elf_type)
    }

    pub fn machine(&self) -> Result<ElfArchitecture, u16> {
        ElfArchitecture::from_u16(self.machine).ok_or(self.machine)
    }

    pub fn from_vmo(vmo: &zx::Vmo) -> Result<Box<Elf32FileHeader>, ElfParseError> {
        // Read and parse the ELF file header from the VMO.
        let mut header = Box::<Elf32FileHeader>::default();
        vmo.read(header.as_mut_bytes(), 0).map_err(|s| ElfParseError::ReadError(s))?;
        header.validate()?;
        Ok(header)
    }
}

impl std::fmt::Debug for Elf32FileHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Elf32FileHeader");
        s.field("ident", &self.ident);

        match self.elf_type() {
            Ok(t) => s.field("elf_type", &t),
            Err(unknown) => s.field("elf_type", &unknown),
        };
        match self.machine() {
            Ok(m) => s.field("machine", &m),
            Err(unknown) => s.field("machine", &unknown),
        };

        s.field("version", &self.version)
            .field("entry", &format_args!("{:#x}", self.entry))
            .field("phoff", &format_args!("{:#x}", self.phoff))
            .field("shoff", &format_args!("{:#x}", self.shoff))
            .field("flags", &format_args!("{:#x}", self.flags))
            .field("ehsize", &self.ehsize)
            .field("phentsize", &self.phentsize)
            .field("phnum", &self.phnum)
            .field("shentsize", &self.shentsize)
            .field("shnum", &self.shnum)
            .field("shstrndx", &self.shstrndx)
            .finish()
    }
}

impl Validate for Elf64FileHeader {
    fn validate(&self) -> Result<(), ElfParseError> {
        if self.ident.magic != ELF_MAGIC {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF magic"));
        }
        if self.ident.class() != Ok(ElfClass::Elf64) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF class"));
        }
        if self.ident.data() != Ok(NATIVE_ENCODING) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF data encoding"));
        }
        if self.ident.version() != Ok(ElfVersion::Current) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF version"));
        }
        if self.phentsize as usize != mem::size_of::<Elf64ProgramHeader>() {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF program header size"));
        }
        if self.phnum == u16::MAX {
            return Err(ElfParseError::InvalidFileHeader(
                "2^16 or more ELF program headers is unsupported",
            ));
        }
        if self.machine() != Ok(CURRENT_ARCH) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF architecture"));
        }
        if self.elf_type() != Ok(ElfType::SharedObject)
            && self.elf_type() != Ok(ElfType::Executable)
        {
            return Err(ElfParseError::InvalidFileHeader(
                "Invalid or unsupported ELF type, only ET_DYN is supported",
            ));
        }
        Ok(())
    }
}

impl Validate for Elf32FileHeader {
    fn validate(&self) -> Result<(), ElfParseError> {
        if self.ident.magic != ELF_MAGIC {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF magic"));
        }
        // If the current arch doesn't support 32-bit ELF, we will fail here
        if ARCH32_ARCH == ElfArchitecture::Unknown || !self.ident.is_arch32() {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF class"));
        }
        if self.ident.data() != Ok(NATIVE_ENCODING) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF data encoding"));
        }
        if self.ident.version() != Ok(ElfVersion::Current) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF version"));
        }
        if self.phentsize as usize != mem::size_of::<Elf32ProgramHeader>() {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF program header size"));
        }
        if self.phnum == u16::MAX {
            return Err(ElfParseError::InvalidFileHeader(
                "2^16 or more ELF program headers is unsupported",
            ));
        }
        if self.machine() != Ok(ARCH32_ARCH) {
            return Err(ElfParseError::InvalidFileHeader("Invalid ELF architecture"));
        }
        if self.elf_type() != Ok(ElfType::SharedObject)
            && self.elf_type() != Ok(ElfType::Executable)
        {
            return Err(ElfParseError::InvalidFileHeader(
                "Invalid or unsupported ELF type, only ET_DYN is supported",
            ));
        }
        Ok(())
    }
}

impl Into<Box<Elf64FileHeader>> for Box<Elf32FileHeader> {
    fn into(self) -> Box<Elf64FileHeader> {
        // The Validate attribute will fail on this header.
        Box::new(Elf64FileHeader {
            ident: self.ident as ElfIdent,
            elf_type: self.elf_type as u16,
            machine: self.machine as u16,
            version: self.version as u32,
            entry: self.entry as usize,
            phoff: self.phoff as usize,
            shoff: self.shoff as usize,
            flags: self.flags as u32,
            ehsize: self.ehsize as u16,
            phentsize: self.phentsize as u16,
            phnum: self.phnum as u16,
            shentsize: self.shentsize as u16,
            shnum: self.shnum as u16,
            shstrndx: self.shstrndx as u16,
        })
    }
}

#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Eq, PartialEq, Default, Clone, Copy)]
#[repr(C)]
pub struct Elf64ProgramHeader {
    pub segment_type: u32,
    pub flags: u32,
    pub offset: usize,
    pub vaddr: usize,
    pub paddr: usize,
    pub filesz: u64,
    pub memsz: u64,
    pub align: u64,
}

#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Eq, PartialEq, Default, Clone, Copy)]
#[repr(C)]
pub struct Elf32ProgramHeader {
    pub segment_type: u32,
    pub offset: u32,
    pub vaddr: u32,
    pub paddr: u32,
    pub filesz: u32,
    pub memsz: u32,
    pub flags: u32,
    pub align: u32,
}

#[derive(FromPrimitive, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Elf64DynTag {
    Null = 0,
    Needed = 1,
    Pltrelsz = 2,
    Pltgot = 3,
    Hash = 4,
    Strtab = 5,
    Symtab = 6,
    Rela = 7,
    Relasz = 8,
    Relaent = 9,
    Strsz = 10,
    Syment = 11,
    Init = 12,
    Fini = 13,
    Soname = 14,
    Rpath = 15,
    Symbolic = 16,
    Rel = 17,
    Relsz = 18,
    Relent = 19,
    Pltrel = 20,
    Debug = 21,
    Textrel = 22,
    Jmprel = 23,
    BindNow = 24,
    InitArray = 25,
    FiniArray = 26,
    InitArraysz = 27,
    FiniArraysz = 28,
    Runpath = 29,
    Flags = 30,
    PreinitArray = 32,
    PreinitArraysz = 33,
    Loos = 0x6000000D,
    Hios = 0x6ffff000,
    Loproc = 0x70000000,
    Hiproc = 0x7fffffff,
}

#[derive(
    IntoBytes, Immutable, Copy, Clone, KnownLayout, FromBytes, Default, Debug, Eq, PartialEq,
)]
#[repr(C)]
pub struct Elf64Dyn {
    pub tag: u64,
    pub value: u64,
}

impl Elf64Dyn {
    pub fn tag(&self) -> Result<Elf64DynTag, u64> {
        Elf64DynTag::from_u64(self.tag).ok_or(self.tag)
    }
}

#[derive(
    IntoBytes, Immutable, Copy, Clone, KnownLayout, FromBytes, Default, Debug, Eq, PartialEq,
)]
#[repr(C)]
pub struct Elf32Dyn {
    pub tag: u32,
    pub value: u32,
}

impl Elf32Dyn {
    pub fn tag(&self) -> Result<Elf64DynTag, u32> {
        // The dyn tags are all int32 regardless of the format.
        Elf64DynTag::from_u32(self.tag).ok_or(self.tag)
    }
}

impl Into<Elf64Dyn> for Elf32Dyn {
    fn into(self) -> Elf64Dyn {
        Elf64Dyn { tag: self.tag as u64, value: self.value as u64 }
    }
}

pub type Elf64Addr = u64;
pub type Elf64Half = u16;
pub type Elf64Word = u32;
pub type Elf64Xword = u64;

pub type Elf32Addr = u32;
pub type Elf32Half = u16;
pub type Elf32Word = u32;
pub type Elf32SWord = i32;
pub type Elf32Lword = u64;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
pub struct elf64_sym {
    pub st_name: Elf64Word,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: Elf64Half,
    pub st_value: Elf64Addr,
    pub st_size: Elf64Xword,
}
pub type Elf64Sym = elf64_sym;

#[repr(C)]
#[derive(Debug, Default, Copy, Clone, IntoBytes, KnownLayout, FromBytes, Immutable)]
pub struct elf32_sym {
    pub st_name: Elf32Word,
    pub st_value: Elf32Addr,
    pub st_size: Elf32Word,
    pub st_info: u8,
    pub st_other: u8,
    pub st_shndx: Elf32Half,
}
pub type Elf32Sym = elf32_sym;

#[derive(FromPrimitive, Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SegmentType {
    /// PT_NULL
    Unused = 0,
    /// PT_LOAD
    Load = 1,
    /// PT_DYNAMIC
    Dynamic = 2,
    /// PT_INTERP
    Interp = 3,
    /// PT_GNU_STACK
    GnuStack = 0x6474e551,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct SegmentFlags: u32 {
        const EXECUTE = 0b0001;
        const WRITE   = 0b0010;
        const READ    = 0b0100;
    }
}

impl fmt::Display for SegmentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SegmentType::Unused => write!(f, "PT_NULL"),
            SegmentType::Load => write!(f, "PT_LOAD"),
            SegmentType::Dynamic => write!(f, "PT_DYNAMIC"),
            SegmentType::Interp => write!(f, "PT_INTERP"),
            SegmentType::GnuStack => write!(f, "PT_GNU_STACK"),
        }
    }
}

impl Elf64ProgramHeader {
    pub fn segment_type(&self) -> Result<SegmentType, u32> {
        SegmentType::from_u32(self.segment_type).ok_or(self.segment_type)
    }

    pub fn flags(&self) -> SegmentFlags {
        // Ignore bits that don't correspond to one of the flags included in SegmentFlags
        SegmentFlags::from_bits_truncate(self.flags)
    }
}

impl std::fmt::Debug for Elf64ProgramHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Elf64ProgramHeader");

        match self.segment_type() {
            Ok(t) => s.field("segment_type", &t),
            Err(unknown) => s.field("segment_type", &unknown),
        };

        s.field("flags", &self.flags())
            .field("offset", &format_args!("{:#x}", self.offset))
            .field("vaddr", &format_args!("{:#x}", self.vaddr))
            .field("paddr", &format_args!("{:#x}", self.paddr))
            .field("filesz", &format_args!("{:#x}", self.filesz))
            .field("memsz", &format_args!("{:#x}", self.memsz))
            .field("align", &self.align)
            .finish()
    }
}

impl Elf32ProgramHeader {
    pub fn segment_type(&self) -> Result<SegmentType, u32> {
        SegmentType::from_u32(self.segment_type).ok_or(self.segment_type)
    }

    pub fn flags(&self) -> SegmentFlags {
        // Ignore bits that don't correspond to one of the flags included in SegmentFlags
        SegmentFlags::from_bits_truncate(self.flags)
    }
}

impl std::fmt::Debug for Elf32ProgramHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("Elf32ProgramHeader");

        match self.segment_type() {
            Ok(t) => s.field("segment_type", &t),
            Err(unknown) => s.field("segment_type", &unknown),
        };

        s.field("flags", &self.flags())
            .field("offset", &format_args!("{:#x}", self.offset))
            .field("vaddr", &format_args!("{:#x}", self.vaddr))
            .field("paddr", &format_args!("{:#x}", self.paddr))
            .field("filesz", &format_args!("{:#x}", self.filesz))
            .field("memsz", &format_args!("{:#x}", self.memsz))
            .field("align", &self.align)
            .finish()
    }
}

impl Into<Elf64ProgramHeader> for Elf32ProgramHeader {
    fn into(self) -> Elf64ProgramHeader {
        Elf64ProgramHeader {
            segment_type: self.segment_type,
            flags: self.flags,
            offset: self.offset as usize,
            vaddr: self.vaddr as usize,
            paddr: self.paddr as usize,
            filesz: self.filesz as u64,
            memsz: self.memsz as u64,
            align: self.align as u64,
        }
    }
}

impl Validate for [Elf64ProgramHeader] {
    fn validate(&self) -> Result<(), ElfParseError> {
        let page_size = zx::system_get_page_size() as usize;
        let mut vaddr_high: usize = 0;
        for hdr in self {
            match hdr.segment_type() {
                Ok(SegmentType::Load) => {
                    if hdr.filesz > hdr.memsz {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "filesz > memsz in a PT_LOAD segment",
                        ));
                    }

                    // Virtual addresses for PT_LOAD segments should not overlap.
                    if hdr.vaddr < vaddr_high {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Overlap in virtual addresses",
                        ));
                    }
                    vaddr_high = hdr.vaddr.checked_add(hdr.memsz as usize).ok_or({
                        ElfParseError::InvalidProgramHeader(
                            "load segment overflow the 64-bit memory space",
                        )
                    })?;

                    // Segment alignment should be a multiple of the system page size.
                    if hdr.align % page_size as u64 != 0 {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Alignment must be multiple of the system page size",
                        ));
                    }

                    // Virtual addresses should be at the same page offset as their offset in the
                    // file.
                    if hdr.align != 0
                        && (hdr.vaddr % hdr.align as usize) != (hdr.offset % hdr.align as usize)
                    {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Virtual address and offset in file are not at same offset in page",
                        ));
                    }
                }
                Ok(SegmentType::GnuStack) => {
                    if hdr.flags().contains(SegmentFlags::EXECUTE) {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Fuchsia does not support executable stacks",
                        ));
                    }
                }
                // No specific validation to perform for these.
                Ok(SegmentType::Unused) | Ok(SegmentType::Interp) | Ok(SegmentType::Dynamic) => {}
                // Ignore segment types that we don't care about.
                Err(_) => {}
            }
        }
        Ok(())
    }
}

// Ensure we stay in the 32-bit space.
impl Validate for [Elf32ProgramHeader] {
    fn validate(&self) -> Result<(), ElfParseError> {
        let page_size = zx::system_get_page_size() as u32;
        let mut vaddr_high: u32 = 0;
        for hdr in self {
            match hdr.segment_type() {
                Ok(SegmentType::Load) => {
                    if hdr.filesz > hdr.memsz {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "filesz > memsz in a PT_LOAD segment",
                        ));
                    }

                    // Virtual addresses for PT_LOAD segments should not overlap.
                    if hdr.vaddr < vaddr_high {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Overlap in virtual addresses",
                        ));
                    }
                    vaddr_high = hdr.vaddr.checked_add(hdr.memsz).ok_or({
                        ElfParseError::InvalidProgramHeader(
                            "load segment overflow the 32-bit memory space",
                        )
                    })?;

                    // Segment alignment should be a multiple of the system page size.
                    if hdr.align % page_size != 0 {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Alignment must be multiple of the system page size",
                        ));
                    }

                    // Virtual addresses should be at the same page offset as their offset in the
                    // file.
                    if hdr.align != 0 && (hdr.vaddr % hdr.align) != (hdr.offset % hdr.align) {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Virtual address and offset in file are not at same offset in page",
                        ));
                    }
                }
                Ok(SegmentType::GnuStack) => {
                    if hdr.flags().contains(SegmentFlags::EXECUTE) {
                        return Err(ElfParseError::InvalidProgramHeader(
                            "Fuchsia does not support executable stacks",
                        ));
                    }
                }
                // No specific validation to perform for these.
                Ok(SegmentType::Unused) | Ok(SegmentType::Interp) | Ok(SegmentType::Dynamic) => {}
                // Ignore segment types that we don't care about.
                Err(_) => {}
            }
        }
        Ok(())
    }
}

pub struct Elf64Headers {
    file_header: Box<Elf64FileHeader>,
    // Use a Box<[_]> instead of a Vec<> to communicate/enforce that the slice is not mutated after
    // construction.
    program_headers: Option<Box<[Elf64ProgramHeader]>>,
    // Section headers are not parsed currently since they aren't needed for the current use case,
    // but could be added if needed.
}

impl Elf64Headers {
    pub fn from_vmo(vmo: &zx::Vmo) -> Result<Elf64Headers, ElfParseError> {
        // Read and parse the ELF file header from the VMO.
        let file_header = Elf64FileHeader::from_vmo(vmo)?;

        // Read and parse the ELF program headers from the VMO. Also support the degenerate case
        // where there are no program headers, which is valid ELF but probably not useful outside
        // tests.
        let mut program_headers = None;
        if file_header.phnum > 0 {
            let mut phdrs = vec![Elf64ProgramHeader::default(); file_header.phnum as usize];
            vmo.read(phdrs.as_mut_bytes(), file_header.phoff as u64)
                .map_err(|s| ElfParseError::ReadError(s))?;
            phdrs.validate()?;
            program_headers = Some(phdrs.into_boxed_slice());
        }

        Ok(Elf64Headers { file_header, program_headers })
    }

    pub fn from_vmo_with_arch32(vmo: &zx::Vmo) -> Result<Elf64Headers, ElfParseError> {
        // Check the class, then load the right one.
        let ident = ElfIdent::from_vmo(vmo)?;
        if !ident.is_arch32() {
            // This is not arch32, so we can just proceed.
            return Elf64Headers::from_vmo(vmo);
        }
        let arch32_headers = Elf32FileHeader::from_vmo(vmo)?;
        let file_header: Box<Elf64FileHeader> = arch32_headers.into();

        // Read and parse the ELF program headers from the VMO. Also support the degenerate case
        // where there are no program headers, which is valid ELF but probably not useful outside
        // tests.
        let mut program_headers = None;
        if file_header.phnum > 0 {
            let mut phdrs = vec![Elf32ProgramHeader::default(); file_header.phnum as usize];
            vmo.read(phdrs.as_mut_bytes(), file_header.phoff as u64)
                .map_err(|s| ElfParseError::ReadError(s))?;
            // We can't just use the 64-bit validation because we need to keep
            // the addresses within the u32 space (if not <3Gb).
            phdrs.validate()?;
            let phdrs64: Vec<Elf64ProgramHeader> = phdrs.iter().map(|&ph| ph.into()).collect();
            // TODO(drewry) we should be able to get rid of this extra validation.
            phdrs64.validate()?;
            program_headers = Some(phdrs64.into_boxed_slice());
        }

        Ok(Elf64Headers { file_header, program_headers })
    }

    pub fn file_header(&self) -> &Elf64FileHeader {
        &*self.file_header
    }

    pub fn program_headers(&self) -> &[Elf64ProgramHeader] {
        match &self.program_headers {
            Some(boxed_slice) => &*boxed_slice,
            None => &[],
        }
    }

    /// Returns an iterator that yields all program headers of the given type.
    pub fn program_headers_with_type(
        &self,
        stype: SegmentType,
    ) -> impl Iterator<Item = &Elf64ProgramHeader> {
        self.program_headers().iter().filter(move |x| match x.segment_type() {
            Ok(t) => t == stype,
            _ => false,
        })
    }

    /// Returns 0 or 1 headers of the given type, or Err([ElfParseError::MultipleHeaders]) if more
    /// than 1 such header is present.
    pub fn program_header_with_type(
        &self,
        stype: SegmentType,
    ) -> Result<Option<&Elf64ProgramHeader>, ElfParseError> {
        let mut headers = self.program_headers_with_type(stype);
        let header = headers.next();
        if headers.next().is_some() {
            return Err(ElfParseError::MultipleHeaders(stype));
        }
        return Ok(header);
    }

    /// Creates an instance of Elf64Headers from in-memory representations of the ELF headers.

    pub fn new_for_test(
        file_header: &Elf64FileHeader,
        program_headers: Option<&[Elf64ProgramHeader]>,
    ) -> Self {
        Self {
            file_header: Box::new(*file_header),
            program_headers: program_headers.map(|headers| headers.into()),
        }
    }
}

pub struct Elf64DynSection {
    dyn_entries: Box<[Elf64Dyn]>,
}
impl Elf64DynSection {
    pub fn from_vmo(vmo: &zx::Vmo) -> Result<Elf64DynSection, ElfParseError> {
        let headers = Elf64Headers::from_vmo(vmo)?;
        const ENTRY_SIZE: usize = std::mem::size_of::<Elf64Dyn>();
        if let Some(dynamic_header) = headers.program_header_with_type(SegmentType::Dynamic)? {
            let dyn_entries_size = dynamic_header.filesz as usize / ENTRY_SIZE;
            let mut entries = vec![Elf64Dyn::default(); dyn_entries_size];
            vmo.read(entries.as_mut_bytes(), dynamic_header.offset as u64)
                .map_err(|s| ElfParseError::ReadError(s))?;
            let dyn_entries = entries.into_boxed_slice();
            return Ok(Elf64DynSection { dyn_entries });
        }
        Err(ElfParseError::ParseError("Dynamic header not found"))
    }

    pub fn from_vmo_with_arch32(vmo: &zx::Vmo) -> Result<Elf64DynSection, ElfParseError> {
        // Check the class, then load the right one.
        let ident = ElfIdent::from_vmo(vmo)?;
        if !ident.is_arch32() {
            // This is not arch32, so we can just proceed.
            return Elf64DynSection::from_vmo(vmo);
        }
        let headers = Elf64Headers::from_vmo_with_arch32(vmo)?;
        const ENTRY_SIZE: usize = std::mem::size_of::<Elf32Dyn>();
        if let Some(dynamic_header) = headers.program_header_with_type(SegmentType::Dynamic)? {
            let dyn_entries_size = dynamic_header.filesz as usize / ENTRY_SIZE;
            let mut entries = vec![Elf32Dyn::default(); dyn_entries_size];
            vmo.read(entries.as_mut_bytes(), dynamic_header.offset as u64)
                .map_err(|s| ElfParseError::ReadError(s))?;
            let entries64: Vec<Elf64Dyn> = entries.iter().map(|&dt| dt.into()).collect();
            let dyn_entries = entries64.into_boxed_slice();
            return Ok(Elf64DynSection { dyn_entries });
        }
        Err(ElfParseError::ParseError("Dynamic header not found"))
    }

    pub fn dynamic_entries(&self) -> &[Elf64Dyn] {
        &*self.dyn_entries
    }

    pub fn dynamic_entry_with_tag(&self, tag: Elf64DynTag) -> Option<&Elf64Dyn> {
        self.dynamic_entries().iter().find(move |x| match x.tag() {
            Ok(t) => t == tag,
            _ => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Error;
    use assert_matches::assert_matches;
    use std::fs::File;

    // These are specially crafted files that just contain a valid ELF64 file header but
    // nothing else.
    static HEADER_DATA_X86_64: &'static [u8] =
        include_bytes!("../test-utils/elf_x86-64_file-header.bin");
    static HEADER_DATA_AARCH64: &'static [u8] =
        include_bytes!("../test-utils/elf_aarch64_file-header.bin");
    static HEADER_DATA_RISCV64: &'static [u8] =
        include_bytes!("../test-utils/elf_riscv64_file-header.bin");

    #[cfg(target_arch = "x86_64")]
    static HEADER_DATA: &'static [u8] = HEADER_DATA_X86_64;
    #[cfg(target_arch = "aarch64")]
    static HEADER_DATA: &'static [u8] = HEADER_DATA_AARCH64;
    #[cfg(target_arch = "riscv64")]
    static HEADER_DATA: &'static [u8] = HEADER_DATA_RISCV64;

    // Returns a vec of file headers for different architectures that do not match the current one.
    fn wrong_arch_file_headers() -> Vec<&'static [u8]> {
        if cfg!(target_arch = "x86_64") {
            vec![HEADER_DATA_AARCH64, HEADER_DATA_RISCV64]
        } else if cfg!(target_arch = "aarch64") {
            vec![HEADER_DATA_X86_64, HEADER_DATA_RISCV64]
        } else if cfg!(target_arch = "riscv64") {
            vec![HEADER_DATA_AARCH64, HEADER_DATA_X86_64]
        } else {
            panic!("Unrecognized arch")
        }
    }

    #[test]
    fn test_parse_file_header() -> Result<(), Error> {
        let vmo = zx::Vmo::create(HEADER_DATA.len() as u64)?;
        vmo.write(&HEADER_DATA, 0)?;

        let headers = Elf64Headers::from_vmo(&vmo)?;
        assert_eq!(
            headers.file_header(),
            &Elf64FileHeader {
                ident: ElfIdent {
                    magic: ELF_MAGIC,
                    class: ElfClass::Elf64 as u8,
                    data: ElfDataEncoding::LittleEndian as u8,
                    version: ElfVersion::Current as u8,
                    osabi: 0,
                    abiversion: 0,
                    pad: [0; 7],
                },
                elf_type: ElfType::SharedObject as u16,
                machine: CURRENT_ARCH as u16,
                version: 1,
                entry: 0x10000,
                phoff: 0,
                shoff: 0,
                flags: 0,
                ehsize: mem::size_of::<Elf64FileHeader>() as u16,
                phentsize: mem::size_of::<Elf64ProgramHeader>() as u16,
                phnum: 0,
                shentsize: 0,
                shnum: 0,
                shstrndx: 0,
            }
        );
        assert_eq!(headers.program_headers().len(), 0);
        Ok(())
    }

    #[test]
    fn test_parse_wrong_arch() -> Result<(), Error> {
        for header_data in wrong_arch_file_headers() {
            let vmo = zx::Vmo::create(header_data.len() as u64)?;
            vmo.write(&HEADER_DATA, 0)?;

            match Elf64Headers::from_vmo(&vmo) {
                Err(ElfParseError::InvalidFileHeader(msg)) => {
                    assert_eq!(msg, "Invalid ELF architecture");
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[test]
    fn test_parse_program_headers() -> Result<(), Error> {
        // Let's try to parse ourselves!
        // Ideally we'd use std::env::current_exe but that doesn't seem to be implemented (yet?)
        let file = File::open("/pkg/bin/elf_parse_lib_test")?;
        let vmo = fdio::get_vmo_copy_from_file(&file)?;

        let headers = Elf64Headers::from_vmo(&vmo)?;
        assert!(headers.program_headers().len() > 0);
        assert!(headers.program_header_with_type(SegmentType::Interp)?.is_some());
        assert!(headers.program_headers_with_type(SegmentType::Dynamic).count() == 1);
        assert!(headers.program_headers_with_type(SegmentType::Load).count() > 1);
        Ok(())
    }

    #[test]
    fn test_parse_dynamic_section() -> Result<(), Error> {
        let file = File::open("/pkg/bin/elf_parse_lib_test")?;
        let vmo = fdio::get_vmo_copy_from_file(&file)?;

        let dynamic_section = Elf64DynSection::from_vmo(&vmo)?;
        assert!(dynamic_section.dynamic_entries().len() > 0);
        Ok(())
    }

    #[test]
    fn test_parse_static_pie() -> Result<(), Error> {
        // Parse the statically linked PIE test binary.
        let file = File::open("/pkg/bin/static_pie_test_util")?;
        let vmo = fdio::get_vmo_copy_from_file(&file)?;

        // Should have no PT_INTERP header, but should have PT_DYNAMIC and 1+ PT_LOAD.
        let headers = Elf64Headers::from_vmo(&vmo)?;
        assert!(headers.program_headers().len() > 0);
        assert!(headers.program_header_with_type(SegmentType::Interp)?.is_none());
        assert!(headers.program_headers_with_type(SegmentType::Dynamic).count() == 1);
        assert!(headers.program_headers_with_type(SegmentType::Load).count() > 1);
        Ok(())
    }

    #[test]
    fn test_parse_program_header_bad_alignment() {
        let page_size = zx::system_get_page_size() as usize;
        let headers = [Elf64ProgramHeader {
            segment_type: SegmentType::Load as u32,
            flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
            align: page_size as u64 + 1024,
            offset: 0x1000,
            vaddr: 0x1000,
            paddr: 0x1000,
            filesz: 0x1000,
            memsz: 0x1000,
        }];
        assert_matches!(
            headers.validate(),
            Err(ElfParseError::InvalidProgramHeader(
                "Alignment must be multiple of the system page size"
            ))
        );
    }

    #[test]
    fn test_parse_program_header_bad_offset_and_vaddr() {
        let page_size = zx::system_get_page_size() as usize;
        let headers = [Elf64ProgramHeader {
            segment_type: SegmentType::Load as u32,
            flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
            align: page_size as u64,
            offset: 0x1001,
            vaddr: 0x1002,
            paddr: 0x1000,
            filesz: 0x1000,
            memsz: 0x1000,
        }];
        assert_matches!(
            headers.validate(),
            Err(ElfParseError::InvalidProgramHeader(
                "Virtual address and offset in file are not at same offset in page"
            ))
        );
    }

    #[test]
    fn test_parse_program_header_overflow() {
        let page_size = zx::system_get_page_size() as usize;
        let headers = [
            Elf64ProgramHeader {
                segment_type: SegmentType::Load as u32,
                flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
                align: page_size as u64,
                offset: 0x1000,
                // Near the end of the address space, will overflow when adding memsz
                vaddr: usize::MAX - 0xfff,
                paddr: usize::MAX - 0xfff,
                filesz: 0x2000,
                memsz: 0x2000,
            },
            Elf64ProgramHeader {
                segment_type: SegmentType::Load as u32,
                flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
                align: page_size as u64,
                offset: 0x3000,
                vaddr: 0x1000,
                paddr: 0x1000,
                filesz: 0x1000,
                memsz: 0x1000,
            },
        ];

        assert_matches!(
            headers.validate(),
            Err(ElfParseError::InvalidProgramHeader(
                "load segment overflow the 64-bit memory space"
            ))
        );
    }

    #[test]
    fn test_parse_program_header_overflow_32() {
        let page_size = zx::system_get_page_size() as u32;
        let headers = [
            Elf32ProgramHeader {
                segment_type: SegmentType::Load as u32,
                flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
                align: page_size,
                offset: 0x1000,
                // Near the end of the address space, will overflow when adding memsz
                vaddr: u32::MAX - 0xfff,
                paddr: u32::MAX - 0xfff,
                filesz: 0x2000,
                memsz: 0x2000,
            },
            Elf32ProgramHeader {
                segment_type: SegmentType::Load as u32,
                flags: (SegmentFlags::READ | SegmentFlags::EXECUTE).bits(),
                align: page_size,
                offset: 0x3000,
                vaddr: 0x1000,
                paddr: 0x1000,
                filesz: 0x1000,
                memsz: 0x1000,
            },
        ];

        assert_matches!(
            headers.validate(),
            Err(ElfParseError::InvalidProgramHeader(
                "load segment overflow the 32-bit memory space"
            ))
        );
    }
}
