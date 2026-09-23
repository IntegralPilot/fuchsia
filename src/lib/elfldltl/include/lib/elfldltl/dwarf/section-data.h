// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_SECTION_DATA_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_SECTION_DATA_H_

#include <cstdint>
#include <optional>
#include <span>
#include <utility>

#include "../layout.h"

namespace elfldltl::dwarf {

// DWARF is generally independent of ELF.  However, decoding it requires
// knowing its byte order; and in a few places, knowing the environment's idea
// of address size (although in most cases DWARF self-identifies its own idea
// of the address size).  These are exactly the two things represented by the
// elfldltl::Elf<...> template types, so some template APIs here use an Elf
// class template parameter to indicate those aspects.  When DWARF is encoded
// in ELF files, the Elf parameter matches what DWARF decoding needs to know.

// Many kinds of DWARF data come in partly self-identifying units.  An object
// file section holding a certain kind of DWARF data often has an arbitrary
// sequence of data units of a particular kind (each kind in its own section),
// collected by the linker.  Across all the kinds of section data, the formats
// for data units have one thing in common: the initial length field.  This
// small header (usually 4 bytes, theoretically sometimes 12) indicates both
// the total length of the data unit and the size of offsets used in the DWARF
// format within that unit: 32-bit DWARF offsets, or 64-bit DWARF offsets.
// Note that this 32-bit / 64-bit distinction is unrelated to machine register
// size, address size, pointer ABI, etc.  The "32-bit DWARF" and "64-bit DWARF"
// formats refer purely to what size is used for DWARF "offset" values, which
// are usually offsets from the beginning of a whole section (so a single DWARF
// section has to exceed 4GiB to warrant using the 64-bit format in units that
// hold offsets into that section).

// This value for the initial length of a data unit indicates that it uses
// the 64-bit DWARF format, and the 64-bit length follows.
static constexpr uint32_t kDwarf64Length = 0xffffffff;

// Other values starting here are reserved, meanings not yet specified.  If
// these values are encountered, we don't know how to determine the length of
// the data unit.
static constexpr uint32_t kDwarf32Limit = 0xfffffff0;

// elfldltl::dwarf::SectionData represents a DWARF data unit in memory.  It
// identifies the DWARF format (offset size) used inside the unit, and
// holds a std::span<const std::byte> of the unit's contents.  SectionData
// objects are small and trivially copyable like std::span.
class SectionData {
 public:
  // This indicates the DWARF format used in a single data unit.  This
  // implies the offset size used in its data, as well as the form and size
  // of its initial length header.
  enum class Format : uint8_t {
    kDwarf32,  // 32-bit DWARF offsets.
    kDwarf64,  // 64-bit DWARF offsets.
  };

  constexpr SectionData() = default;

  constexpr SectionData(const SectionData&) = default;

  constexpr SectionData(std::span<const std::byte> contents, Format format)
      : contents_{contents}, format_{format} {}

  constexpr SectionData& operator=(const SectionData&) = default;

  // The total size of this data unit, including its initial length header.
  // This is the amount of the data that is consumed by Read (see below).
  constexpr size_t size_bytes() const { return initial_length_size() + contents_.size_bytes(); }

  // The contents of this data unit, not including its initial length header.
  constexpr std::span<const std::byte> contents() const { return contents_; }

  // The format of this data unit.
  constexpr Format format() const { return format_; }

  // The size of this data unit's initial length header.
  constexpr uint8_t initial_length_size() const { return InitialLengthSize(format_); }

  // The size of any data unit's initial length header given the format.
  static constexpr uint8_t InitialLengthSize(Format format) {
    return sizeof(uint32_t) + (format == Format::kDwarf64 ? sizeof(uint64_t) : 0);
  }

  // The size in bytes of DWARF offsets (4 or 8) used in this data unit.
  constexpr uint8_t offset_size() const { return OffsetSize(format_); }

  // The size of DWARF offsets used in any data unit given the format.
  static constexpr uint8_t OffsetSize(Format format) {
    return format == Format::kDwarf64 ? sizeof(uint64_t) : sizeof(uint32_t);
  }

  // Read an offset in this data unit's format from the byte buffer.
  // Returns std::nullopt if the buffer is too small.  The Elf template
  // parameter indicates the byte order used in the data unit.
  template <ElfApi Elf = Elf<>>
  constexpr std::optional<uint64_t> read_offset(size_t pos = 0) {
    return ReadOffset<Elf>(format_, contents_.subspan(pos));
  }

  // Read an offset in the indicated DWARF format from the byte buffer.
  // A 32-bit offset is zero-extended to uint64_t.
  template <ElfApi Elf = Elf<>>
  static constexpr std::optional<uint64_t> ReadOffset(  //
      Format format, std::span<const std::byte> bytes) {
    switch (format) {
      case Format::kDwarf32:
        return ReadFromBytes<typename Elf::Word>(bytes);
      case Format::kDwarf64:
        return ReadFromBytes<typename Elf::Xword>(bytes);
    }
    __builtin_trap();  // Should be unreachable.
  }

  // This identifies the size and format of a data unit in the byte stream.
  // If there is a fundamental format error, it's reported via the
  // Diagnostics object with the error_args appended after the generic
  // message and std::nullopt is returned.  Otherwise, size_bytes()
  // indicates how much of the byte stream was consumed by this unit, and
  // contents() holds the data unit bytes after the initial length: where
  // the header for the particular kind of data starts.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  static constexpr std::optional<SectionData> Read(  //
      Diagnostics& diag, std::span<const std::byte> bytes, ErrorArgs&&... error_args);

  // This combines Read with peeling off the size_bytes() consumed from the
  // byte stream, returning the new tail:
  // ```
  // auto [data, bytes] = SectionData::Consume(diag, bytes);
  // if (data) {
  //   Decode(data->contents);
  // }
  // ```
  // On failure (when `data == std::nullopt`), `bytes` is returned unchanged.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  static constexpr std::pair<std::optional<SectionData>, std::span<const std::byte>> Consume(
      Diagnostics& diag, std::span<const std::byte> bytes, ErrorArgs&&... error_args) {
    auto read = Read<Elf>(diag, bytes, std::forward<ErrorArgs>(error_args)...);
    return {read, read ? bytes.subspan(read->size_bytes()) : bytes};
  }

 private:
  template <typename T>
  static constexpr std::optional<T> ReadFromBytes(std::span<const std::byte> bytes) {
    if (bytes.size_bytes() < sizeof(T)) [[unlikely]] {
      return std::nullopt;
    }
    T value;
    memcpy(&value, bytes.data(), sizeof(value));
    return value;
  }

  std::span<const std::byte> contents_;
  Format format_ = Format::kDwarf32;
};

template <ElfApi Elf, class Diagnostics, typename... ErrorArgs>
constexpr std::optional<SectionData> SectionData::Read(  //
    Diagnostics& diag, std::span<const std::byte> bytes, ErrorArgs&&... error_args) {
  using Word = Elf::Word;
  using Xword = Elf::Xword;

  const size_t input_size = bytes.size_bytes();
  auto consume = [&](auto& value) -> bool {
    if (bytes.size_bytes() < sizeof(value)) [[unlikely]] {
      diag.FormatError("data size ", input_size, " too small for DWARF header",
                       std::forward<ErrorArgs>(error_args)...);
      return false;
    }
    memcpy(&value, bytes.data(), sizeof(value));
    bytes = bytes.subspan(sizeof(value));
    return true;
  };

  Word initial_length;
  if (!consume(initial_length)) [[unlikely]] {
    return std::nullopt;
  }

  Format format = Format::kDwarf32;
  size_t data_size = initial_length;
  if (initial_length >= kDwarf32Limit) {
    if (initial_length != kDwarf64Length) [[unlikely]] {
      diag.FormatError("Reserved initial-length value ", initial_length, " used in DWARF header",
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }
    Xword size64;
    if (!consume(size64)) [[unlikely]] {
      return std::nullopt;
    }
    format = Format::kDwarf64;
    data_size = static_cast<size_t>(size64);
    if (data_size != size64) [[unlikely]] {
      diag.FormatError("64-bit initial length ", size64, " in DWARF header too big",
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }
  }

  if (bytes.size_bytes() < data_size) [[unlikely]] {
    diag.FormatError("data size ", input_size, " < ", InitialLengthSize(format) + data_size,
                     " required by DWARF header", std::forward<ErrorArgs>(error_args)...);
    return std::nullopt;
  }

  return SectionData{bytes.first(data_size), format};
}

}  // namespace elfldltl::dwarf

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_SECTION_DATA_H_
