// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_DIE_REF_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_DIE_REF_H_

#include <lib/syslog/cpp/macros.h>
#include <stdint.h>

#include <optional>
#include <ostream>

namespace zxdb {

class DwarfDieRef {
 public:
  enum class Section : uint8_t {
    kMain,  // .debug_info
    kType,  // .debug_types
  };

  constexpr DwarfDieRef() = default;

  static constexpr DwarfDieRef Main(uint64_t off) { return DwarfDieRef(off, Section::kMain); }
  static constexpr DwarfDieRef Type(uint64_t off) { return DwarfDieRef(off, Section::kType); }

  // Returns a DwarfDieRef for a DIE inside a type unit of the given DWARF version.
  // DWARF 4 places type units in .debug_types (Section::kType), while DWARF 5
  // places type units directly in .debug_info (Section::kMain).
  static constexpr DwarfDieRef ForTypeUnit(int dwarf_version, uint64_t off) {
    if (dwarf_version < 5) {
      return Type(off);
    }
    return Main(off);
  }

  constexpr bool is_valid() const { return offset_.has_value(); }
  constexpr explicit operator bool() const { return is_valid(); }

  constexpr uint64_t offset() const {
    FX_DCHECK(is_valid());
    return *offset_;
  }
  constexpr Section section() const { return section_; }

  constexpr auto operator<=>(const DwarfDieRef& other) const = default;

  constexpr DwarfDieRef operator+(uint64_t addend) const {
    if (!is_valid()) {
      return DwarfDieRef();
    }
    return DwarfDieRef(*offset_ + addend, section_);
  }

 private:
  constexpr explicit DwarfDieRef(uint64_t off, Section sec = Section::kMain)
      : section_(sec), offset_(off) {}

  Section section_ = Section::kMain;
  std::optional<uint64_t> offset_;
};

inline std::ostream& operator<<(std::ostream& out, const DwarfDieRef& ref) {
  if (!ref.is_valid()) {
    return out << "<invalid>";
  }
  out << "offset 0x" << std::hex << ref.offset();
  if (ref.section() == DwarfDieRef::Section::kType) {
    out << " (.debug_types)";
  }
  return out;
}

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_SYMBOLS_DWARF_DIE_REF_H_
