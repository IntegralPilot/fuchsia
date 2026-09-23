# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Common utilities to parse ELF files."""

import io
import os
import struct
from collections.abc import Callable

# Standard ELF constants.
ELFMAG = b"\x7fELF"
ELF64_HEADER_SIZE = 64
EI_CLASS = 4
ELFCLASS64 = 2
PT_NOTE = 0x00000004
SHT_SYMTAB = 2
SHT_STRTAB = 3


def _roundup4(size):
    """Round |size| to 4 byte alignment."""
    return (size + 3) // 4 * 4


class ElfInput(object):
    """Convenience class to extract data from ELF input files.

    Usage is the following:

      1) Call ElfInput.open_file() or ElfInput.open_memory() to create
         instance.

      2) Call is_elf64() and other methods to return information about
         the input file.

      3) Call close() method when done.
    """

    def __init__(
        self,
        input: io.IOBase,
        input_size: int,
        input_name: str | None = None,
        log_func: Callable[[str], None] | None = None,
    ):
        self._input = input
        self._input_size = input_size
        self._input_name = input_name if input_name else "input"
        self._log_func = log_func
        self._elf_bits = 64

    def close(self):
        self._input.close()

    @property
    def size(self):
        """Return input ELF size in bytes."""
        return self._input_size

    @staticmethod
    def open_file(path: str, **kwargs):
        """Return  ElfInput instance for a file path."""
        file_size = os.path.getsize(path)
        return ElfInput(open(path, "rb"), file_size, path, **kwargs)

    @staticmethod
    def open_memory(data: bytes, *kwargs):
        """Return ElfInput instance for a memory buffer."""
        return ElfInput(io.BytesIo(data), len(data), "input", **kwargs)

    def is_elf64(self) -> bool:
        """Return true if instance is an ELF64 file."""
        header = self._try_read(ELF64_HEADER_SIZE)
        if len(header) != ELF64_HEADER_SIZE:
            return False
        if header[: len(ELFMAG)] != ELFMAG:
            return False  # Not an ELF file.
        if header[EI_CLASS] != ELFCLASS64:
            return False
        return True

    def ensure_elf64(self):
        """Ensure the input is ELF64, on success return self, or None on failure."""
        if self.is_elf64():
            return self
        else:
            self._log("NOT AN ELF64 FILE: " + self._input_name)
            return None

    def has_sections(self) -> bool:
        """Return true if an ELF file has sections."""
        if self._elf_bits == 64:
            return self._has_elf64_sections()
        return False

    def has_debug_info(self) -> bool:
        """Return True if this file contains debug sections."""
        if self._elf_bits == 64:
            return self._has_elf64_debug_info()
        return False

    def is_stripped(self) -> bool:
        """Return True if this file is stripped, i.e. has no local symbols."""
        if self._elf_bits == 64:
            return self._is_elf64_stripped()
        # Consider that invalid inputs are always stripped.
        return True

    def get_build_id(self) -> str:
        """Return GNU Build-ID value from ELF64 file, or an empty string if not available."""
        if self._elf_bits == 64:
            return self._get_elf64_build_id()
        return ""

    # Implementation details

    def _log(self, msg):
        if self._log_func:
            self._log_func(msg)

    def _try_read(self, size: int) -> bytes | None:
        return self._input.read(size)

    def _read(self, size: int) -> bytes:
        result = self._try_read(size)
        assert (
            result and len(result) == size
        ), f"Could not read {size} bytes from {self._input_name}"
        return result

    def _read_at(self, pos: int, size: int) -> bytes:
        self._input.seek(pos)
        return self._read(size)

    def _unpack_from(self, format: str, offset: int):
        self._input.seek(offset)
        data = self._read(struct.calcsize(format))
        return struct.unpack(format, data)

    def _has_elf64_sections(self) -> bool:
        """Return true if an ELF64 file has sections."""
        e_shoff, e_shentsize, e_shnum = self._get_elf64_sections_info()
        return e_shoff > 0 and e_shentsize == 64 and e_shnum > 0

    def _get_elf64_sections_info(self) -> tuple[int, int, int]:
        """Return true if an ELF64 file has sections."""
        e_shoff = self._unpack_from("Q", 0x28)[0]
        e_shentsize, e_shnum = self._unpack_from("HH", 0x3A)
        return e_shoff, e_shentsize, e_shnum

    def _has_elf64_debug_info(self) -> bool:
        """Return True if this file contains debug sections."""
        # The only reliable way to detect debug sections is to look at
        # their names, that begins with `.debug`. So first locate the
        # string table, then provide a section_filter that returns True
        # if the section is a debug one.
        string_table_func = self._get_elf64_string_table_func()

        def filter_debug_section(sh_pos, sh_size):
            sh_name_index = self._unpack_from("I", sh_pos)[0]
            sh_name = string_table_func(sh_name_index)
            self._log(
                "FILTER DEBUG SECTION %s %s [%s]"
                % (self._input_name, sh_name_index, sh_name)
            )
            return sh_name.startswith(".debug")

        return any(self._filter_elf64_sections(filter_debug_section))

    def _is_elf64_stripped(self) -> bool:
        """Return True if this file is stripped, i.e. has no local symbols."""
        string_table_func = self._get_elf64_string_table_func()

        def filter_symtab_section(sh_pos, sh_size):
            sh_type = self._unpack_from("I", sh_pos + 4)[0]
            sh_name_index = self._unpack_from("I", sh_pos)[0]
            sh_name = string_table_func(sh_name_index)
            self._log(
                "FILTER SYMTAB SECTION %s %s [%s] TYPE %s"
                % (self._input_name, sh_name_index, sh_name, sh_type)
            )
            return sh_type == SHT_SYMTAB

        return not any(self._filter_elf64_sections(filter_symtab_section))

    def _get_elf64_string_table_func(self):
        """Return a callable function that can convert a name_index into a
        string using the ELF64 string table from the input, if any."""
        # Index of string table in section table.
        e_shstrndx = self._unpack_from("H", 0x3E)[0]
        if e_shstrndx == 0:
            self._log("No string table in input")
            return self._no_string_table_func

        self._log(f"e_shstrndx = {e_shstrndx}")
        e_shoff, e_shentsize, e_shnum = self._get_elf64_sections_info()
        if e_shstrndx >= e_shnum:
            self._log(f"Invalid string table index {e_shstrndx} >= {e_shnum}")
            return self._no_string_table_func

        pos = e_shoff + e_shstrndx * e_shentsize
        if pos + e_shentsize > self.size:
            self._log(f"Malformed string table section entry")
            return self._no_string_table_func

        sh_type = self._unpack_from("I", pos + 4)[0]
        assert sh_type == SHT_STRTAB

        sh_offset, sh_size = self._unpack_from("QQ", pos + 0x18)

        def string_table_func(name_index):
            result = ""
            pos = name_index
            while pos < sh_size:
                b = self._read_at(sh_offset + pos, 1)[0]
                if b == b"\0":
                    break
                result += chr(b)
                pos += 1
            return result

        return string_table_func

    @staticmethod
    def _no_string_table_func(name_index):
        # Special function used when there is no string table.
        return "<MISSING_STRING_TABLE>"

    def _filter_elf64_sections(self, section_filter):
        """Generate list of section information in an ELF64 file.

        Args:
            section_filter: A callable that takes three parameters: an entry
                offset, and an entry size, and which returns a value that will
                be added to the result of the current call.
        """
        e_shoff, e_shentsize, e_shnum = self._get_elf64_sections_info()
        if e_shnum == 0:
            return

        assert e_shentsize == 0x40, (
            "Invalid section header entry size %s" % e_shentsize
        )

        pos = e_shoff
        limit = min(pos + e_shentsize * e_shnum, self.size)
        while pos + e_shentsize <= limit:
            yield section_filter(pos, pos + e_shentsize)
            pos += e_shentsize

    def _list_elf_notes(self, pos: int, size: int):
        """Generate (name, desc, type) notes from an ELF note section.

        Args:
            pos: Position of PT_NOTE section in input.
            size: Size of PT_NOTE section in bytes.
        Yields:
            (name, desc, type) tuples, where 'name' is a string, 'desc' a byte array
            and 'type' is an integer.
        """
        limit = pos + size
        while pos + 12 <= limit:
            namesz, descsz, type = self._unpack_from("III", pos)
            name_pos = pos + 12
            desc_pos = name_pos + _roundup4(namesz)
            next_pos = desc_pos + _roundup4(descsz)
            if next_pos > limit:
                break  # malformed input.

            if namesz > 0:
                name = self._read_at(name_pos, namesz - 1).decode()
                desc = self._read_at(desc_pos, descsz)
                yield (name, desc, type)

            pos = next_pos

    def _list_elf64_program_headers(self):
        """Generate (type, offset, size) tuples listing program headers in an ELF64 file."""
        e_phoff = self._unpack_from("Q", 0x20)[0]
        e_phentsize, e_phnum = self._unpack_from("HH", 0x36)
        assert e_phentsize == 0x38, (
            "Invalid program header entry size %s" % e_phentsize
        )

        self._log(
            "FOUND PROGRAM HEADERS AT %s size %s"
            % (e_phoff, e_phentsize * e_phnum)
        )
        pos = e_phoff
        limit = min(pos + e_phentsize * e_phnum, self.size)
        while pos + e_phentsize <= limit:
            p_type = self._unpack_from("I", pos)[0]
            p_offset = self._unpack_from("Q", pos + 0x08)[0]
            p_filesz = self._unpack_from("Q", pos + 0x020)[0]
            yield (p_type, p_offset, p_filesz)
            pos += e_phentsize

    def _get_elf64_build_id(self) -> str:
        """Return GNU Build-ID value from ELF64 file, or an empty string if not available."""
        for p_type, p_offset, p_filesz in self._list_elf64_program_headers():
            self._log(
                "  PROGRAM HEADER type 0x%x offset %s size %s"
                % (p_type, p_offset, p_filesz)
            )
            if p_type == PT_NOTE:
                self._log("FOUND PT_NOTE at %s size %s" % (p_offset, p_filesz))
                for name, desc, type in self._list_elf_notes(
                    p_offset, p_filesz
                ):
                    if name == "GNU" and type == 3:
                        return "".join("%02x" % x for x in desc)
        return ""
