// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_PHDR_ERROR_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_PHDR_ERROR_H_

#include "../constants.h"
#include "const-string.h"

namespace elfldltl::internal {

consteval std::string PhdrTypeName(ElfPhdrType type) {
  switch (type) {
    case ElfPhdrType::kNull:
      return "PT_NULL";
    case ElfPhdrType::kLoad:
      return "PT_LOAD";
    case ElfPhdrType::kDynamic:
      return "PT_DYNAMIC";
    case ElfPhdrType::kInterp:
      return "PT_INTERP";
    case ElfPhdrType::kNote:
      return "PT_NOTE";
    case ElfPhdrType::kTls:
      return "PT_TLS";
    case ElfPhdrType::kPhdr:
      return "PT_PHDR";
    case ElfPhdrType::kEhFrameHdr:
      return "PT_GNU_EH_FRAME";
    case ElfPhdrType::kStack:
      return "PT_GNU_STACK";
    case ElfPhdrType::kRelro:
      return "PT_GNU_RELRO";
  }
}

template <ElfPhdrType Type>
struct PhdrError {
  static constexpr ConstString kDuplicateHeader{
      [] { return "too many "s + PhdrTypeName(Type) + " headers; expected at most one"; }};

  static constexpr ConstString kUnknownFlags{[] {
    return PhdrTypeName(Type) + " header has unrecognized flags (other than PF_R, PF_W, PF_X)";
  }};

  static constexpr ConstString kBadAlignment{[] {
    return PhdrTypeName(Type) + " header has `p_align` that is not zero or a power of two";
  }};

  static constexpr ConstString kUnalignedVaddr{
      [] { return PhdrTypeName(Type) + " header has `p_vaddr % p_align != 0`"; }};

  static constexpr ConstString kOffsetNotEquivVaddr{[] {
    return PhdrTypeName(Type) + " header has incongruent `p_offset` and `p_vaddr` modulo `p_align`";
  }};

  static constexpr ConstString kFileszNotEqMemsz{
      [] { return PhdrTypeName(Type) + " header has `p_filesz != p_memsz`"; }};

  static constexpr ConstString kIncompatibleEntrySize{
      [] { return PhdrTypeName(Type) + " segment size is not a multiple of entry size"; }};

  static constexpr ConstString kIncompatibleEntryAlignment{[] {
    return PhdrTypeName(Type) + " segment alignment is not a multiple of entry alignment";
  }};
};

}  // namespace elfldltl::internal

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_PHDR_ERROR_H_
