// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bstr::{BStr, BString, ByteSlice};
use ebpf::{EbpfInstruction, EbpfMapType, MapFlags, MapSchema};
use num_derive::FromPrimitive;
use starnix_ext::map_ext::EntryExt;
use std::collections::HashMap;
use std::io::Read;
use std::{fs, io, mem};
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Eq, PartialEq, Default, Clone)]
#[repr(C)]
pub struct ElfIdent {
    pub magic: [u8; 4],
    pub class: u8,
    pub data: u8,
    pub version: u8,
    pub osabi: u8,
    pub abiversion: u8,
    pub pad: [u8; 7],
}

#[derive(KnownLayout, FromBytes, IntoBytes, Immutable, Eq, PartialEq, Default, Clone)]
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

const EM_BPF: u16 = 247;

#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Eq, PartialEq, Default, Clone, Debug)]
#[repr(C)]
pub struct Elf64SectionHeader {
    pub name: u32,
    pub type_: u32,
    pub flags: u64,
    pub addr: usize,
    pub offset: usize,
    pub size: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
}

#[derive(Debug, FromPrimitive, Eq, PartialEq, Copy, Clone)]
#[repr(u8)]
pub enum Elf64SectionType {
    Null = 0,
    Progbits = 1,
    Symtab = 2,
    Strtab = 3,
    Rela = 4,
    // As Progbits, but requires a array map to store the data
    NoBits = 8,
}

#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Debug, Eq, PartialEq, Copy, Clone)]
#[repr(C)]
pub struct Elf64Symbol {
    name: u32,
    info: u8,
    other: u8,
    shndx: u16,
    value: usize,
    size: u64,
}

#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Debug, Eq, PartialEq, Copy, Clone)]
#[repr(C)]
struct Elf64_Rel {
    offset: usize,
    info: u64,
}

/// Must match `struct bpf_map_def` in `bpf_helpers.h`
#[derive(KnownLayout, FromBytes, Immutable, IntoBytes, Debug, Eq, PartialEq, Copy, Clone)]
#[repr(C)]
struct bpf_map_def {
    map_type: EbpfMapType,
    key_size: u32,
    value_size: u32,
    max_entries: u32,
    flags: u32,
}

#[derive(Debug, PartialEq)]
pub struct MapDefinition {
    // The name is missing for array maps that are defined in the bss section.
    pub name: Option<BString>,
    pub schema: MapSchema,
}

impl MapDefinition {
    pub fn name(&self) -> String {
        self.name
            .as_ref()
            .map(|s| s.to_str_lossy().to_string())
            .unwrap_or_else(|| "<unnamed>".to_owned())
    }
}

#[derive(Debug)]
pub struct ProgramDefinition {
    pub code: Vec<EbpfInstruction>,
    pub maps: Vec<MapDefinition>,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("{}", _0)]
    IoError(#[from] io::Error),
    #[error("Invalid ELF file: {}", _0)]
    ElfParse(&'static str),
    #[error("Can't find section {:?}", _0)]
    ElfMissingSection(BString),
    #[error("InvalidStringIndex {}", _0)]
    ElfInvalidStringIndex(usize),
    #[error("InvalidSymbolIndex {}", _0)]
    ElfInvalidSymIndex(usize),
    #[error("Can't find function named: {}", _0)]
    InvalidProgramName(String),
    #[error("File is not compiled for eBPF")]
    InvalidArch,
}

#[derive(Debug)]
struct ElfFileContents {
    data: Vec<u8>,
}

impl ElfFileContents {
    fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    fn header(&self) -> Result<&Elf64FileHeader, Error> {
        let (header, _) = Elf64FileHeader::ref_from_prefix(&self.data)
            .map_err(|_| Error::ElfParse("Failed to load header"))?;
        Ok(header)
    }

    fn sections(&self) -> Result<&[Elf64SectionHeader], Error> {
        let header = self.header()?;
        let sections_start = header.shoff as usize;
        let sections_end = header.shoff + header.shnum as usize * header.shentsize as usize;
        let (sections, _) = <[Elf64SectionHeader]>::ref_from_prefix(
            &self
                .data
                .get(sections_start..sections_end)
                .ok_or_else(|| Error::ElfParse("Invalid sections header"))?,
        )
        .map_err(|_| Error::ElfParse("Failed to load ELF sections"))?;
        Ok(sections)
    }

    fn find_section<F>(&self, pred: F) -> Result<Option<&Elf64SectionHeader>, Error>
    where
        F: Fn(&Elf64SectionHeader) -> bool,
    {
        let sections = self.sections()?;
        Ok(sections.iter().find(|s| pred(s)))
    }

    fn get_section_data(&self, section: &Elf64SectionHeader) -> Result<&[u8], Error> {
        let section_start = section.offset as usize;
        let section_end = section_start + section.size as usize;
        self.data
            .get(section_start..section_end)
            .ok_or_else(|| Error::ElfParse("Invalid ELF section location"))
    }
}

#[derive(Debug)]
struct StringsSection {
    data: Vec<u8>,
}

impl StringsSection {
    fn get(&self, index: u32) -> Result<Option<&BStr>, Error> {
        let index = index as usize;
        if index >= self.data.len() {
            return Err(Error::ElfInvalidStringIndex(index));
        }
        if index == 0 {
            return Ok(None);
        }
        let end = index + self.data[index..].iter().position(|c| *c == 0).unwrap();
        Ok(Some(<&BStr>::from(&self.data[index..end])))
    }
}

#[derive(Debug)]
struct SymbolInfo<'a> {
    name: Option<&'a BStr>,
    section: &'a Elf64SectionHeader,
    data: &'a [u8],
    offset: usize,
}

#[derive(Debug)]
pub struct ElfFile {
    contents: ElfFileContents,
    strings: StringsSection,
    symbols_header: Elf64SectionHeader,
}

impl ElfFile {
    pub fn new(path: &str) -> Result<Self, Error> {
        let mut data = Vec::new();
        let mut file = fs::File::open(path)?;
        file.read_to_end(&mut data)?;
        let contents = ElfFileContents::new(data);

        let strings = contents
            .find_section(|s| s.type_ == Elf64SectionType::Strtab as u32)?
            .ok_or_else(|| Error::ElfParse("Symbols section not found"))?;
        let strings = contents.get_section_data(strings)?.to_vec();
        let strings = StringsSection { data: strings };

        let symbols_header = contents
            .find_section(|s| s.type_ == Elf64SectionType::Symtab as u32)?
            .ok_or_else(|| Error::ElfParse("Symbols section not found"))?
            .clone();

        Ok(Self { contents, strings, symbols_header })
    }

    fn get_section(&self, name: &BStr) -> Result<&[u8], Error> {
        let header = self
            .contents
            .find_section(|s| self.strings.get(s.name).unwrap_or(None) == Some(name))?
            .ok_or_else(|| Error::ElfMissingSection(name.to_owned()))?;
        self.contents.get_section_data(header)
    }

    fn symbols(&self) -> Result<&[Elf64Symbol], Error> {
        <[Elf64Symbol]>::ref_from_bytes(self.contents.get_section_data(&self.symbols_header)?)
            .map_err(|_| Error::ElfParse("Invalid ELF symbols table"))
    }

    fn get_symbol_info(&self, sym: &Elf64Symbol) -> Result<SymbolInfo<'_>, Error> {
        let sections = self.contents.sections()?;
        let section = sections
            .get(sym.shndx as usize)
            .ok_or_else(|| Error::ElfParse("Invalid section index"))?;
        let section_data = self.contents.get_section_data(section)?;
        let offset = sym.value;
        let end = sym.value + sym.size as usize;
        let data = section_data
            .get(offset..end)
            .ok_or_else(|| Error::ElfParse("Invalid symbol location"))?;
        let name = self.strings.get(sym.name)?;
        Ok(SymbolInfo { name, section, data, offset })
    }

    fn symbol_by_name(
        &self,
        name: &BStr,
        section_name: &BStr,
    ) -> Result<Option<SymbolInfo<'_>>, Error> {
        for sym in self
            .symbols()?
            .iter()
            .filter(|s| self.strings.get(s.name).unwrap_or(None) == Some(name))
        {
            let info = self.get_symbol_info(sym)?;
            if self.strings.get(info.section.name)? == Some(section_name) {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    fn symbol_by_index(&self, index: usize) -> Result<SymbolInfo<'_>, Error> {
        let sym = self.symbols()?.get(index).ok_or_else(|| Error::ElfInvalidSymIndex(index))?;
        self.get_symbol_info(sym)
    }
}

pub fn load_ebpf_program(
    path: &str,
    section_name: &str,
    program_name: &str,
) -> Result<ProgramDefinition, Error> {
    let elf_file = ElfFile::new(path)?;
    load_ebpf_program_from_file(&elf_file, section_name, program_name)
}

pub fn load_ebpf_program_from_file(
    elf_file: &ElfFile,
    section_name: &str,
    program_name: &str,
) -> Result<ProgramDefinition, Error> {
    if elf_file.contents.header()?.machine != EM_BPF {
        return Err(Error::InvalidArch);
    }

    let prog_sym = elf_file
        .symbol_by_name(program_name.into(), BStr::new(section_name))?
        .ok_or_else(|| Error::InvalidProgramName(program_name.to_owned()))?;
    let mut code = <[EbpfInstruction]>::ref_from_bytes(prog_sym.data)
        .map_err(|_| Error::ElfParse("Failed to load program instructions"))?
        .to_vec();

    // Walk through the relocation table to update all map references
    // in the program while building the maps list.
    let mut maps = vec![];
    let mut map_indices = HashMap::new();

    let rel_table_section_name = format!(".rel{}", section_name);
    let rel_table = match elf_file.get_section(BStr::new(&rel_table_section_name)) {
        Ok(r) => Some(r),
        Err(Error::ElfMissingSection(_)) => None,
        Err(e) => return Err(e),
    };
    if let Some(rel_table) = rel_table {
        let rel_entries = <[Elf64_Rel]>::ref_from_bytes(rel_table)
            .map_err(|_| Error::ElfParse("Failed to parse .rel section"))?;
        for rel in rel_entries.iter().filter(|rel| {
            rel.offset >= prog_sym.offset && rel.offset < prog_sym.offset + prog_sym.data.len()
        }) {
            let offset = rel.offset - prog_sym.offset;
            if offset % mem::size_of::<EbpfInstruction>() != 0 {
                return Err(Error::ElfParse("Invalid relocation offset"));
            }
            let pc = offset / mem::size_of::<EbpfInstruction>();
            let sym_index = (rel.info >> 32) as usize;
            let sym = elf_file.symbol_by_index(sym_index)?;

            // Determine whether the map is found the map section or bss
            let is_from_map_section = match sym {
                SymbolInfo { name: Some(_), section: Elf64SectionHeader { type_, .. }, .. }
                    if *type_ == Elf64SectionType::Progbits as u32 =>
                {
                    code[pc].set_src_reg(ebpf::BPF_PSEUDO_MAP_IDX);
                    true
                }
                SymbolInfo { name: None, section: Elf64SectionHeader { type_, .. }, .. }
                    if *type_ == Elf64SectionType::NoBits as u32 =>
                {
                    let offset = code[pc].imm();
                    code[pc].set_src_reg(ebpf::BPF_PSEUDO_MAP_IDX_VALUE);
                    code[pc + 1].set_imm(offset);
                    false
                }
                _ => return Err(Error::ElfParse("Invalid map symbol")),
            };

            // Insert map index to the code. The actual map address is inserted
            // later, when the program is linked.
            let map_index = map_indices.entry(sym_index).or_insert_with_fallible(|| {
                let schema = if is_from_map_section {
                    let Ok((def, _)) = bpf_map_def::ref_from_prefix(sym.data) else {
                        return Err(Error::ElfParse("Failed to load map definition"));
                    };
                    MapSchema {
                        map_type: def.map_type,
                        key_size: def.key_size,
                        value_size: def.value_size,
                        max_entries: def.max_entries,
                        flags: MapFlags::from_bits_truncate(def.flags),
                    }
                } else {
                    MapSchema {
                        map_type: linux_uapi::bpf_map_type_BPF_MAP_TYPE_ARRAY,
                        key_size: 4,
                        value_size: sym.section.size as u32,
                        max_entries: 1,
                        flags: MapFlags::empty(),
                    }
                };
                maps.push(MapDefinition { name: sym.name.map(|x| x.to_owned()), schema });
                Ok(maps.len() - 1)
            })?;

            code[pc].set_imm(*map_index as i32);
        }
    }

    Ok(ProgramDefinition { code, maps })
}

#[cfg(test)]
mod test {
    use ebpf_api::{AttachType, ProgramType};

    use super::*;

    #[test]
    fn test_load_ebpf_program() {
        let ProgramDefinition { code, maps } =
            load_ebpf_program("/pkg/data/loader_test_prog.o", ".text", "test_prog")
                .expect("Failed to load program");

        // Verify that all maps were loaded.
        let mut names = maps.iter().map(|m| m.name.clone()).collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            &names,
            &[None, BStr::new("array").to_owned().into(), BStr::new("hashmap").to_owned().into()]
        );

        // Check that the program passes the verifier.
        let maps_schema = maps.iter().map(|m| m.schema).collect();
        let calling_context = ProgramType::SocketFilter
            .create_calling_context(AttachType::Unspecified, maps_schema)
            .unwrap();

        ebpf::verify_program(code, calling_context, &mut ebpf::NullVerifierLogger)
            .expect("Failed to verify loaded program");
    }
}
