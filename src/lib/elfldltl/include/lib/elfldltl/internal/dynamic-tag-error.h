// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DYNAMIC_TAG_ERROR_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DYNAMIC_TAG_ERROR_H_

#include "../constants.h"
#include "const-string.h"

namespace elfldltl::internal {

template <ElfDynTag Tag>
struct DynamicTagType {};

consteval std::string DynamicTagName(ElfDynTag tag) {
  switch (tag) {
    case ElfDynTag::kNull:
      return "DT_NULL";
    case ElfDynTag::kRelr:
      return "DT_RELR";
    case ElfDynTag::kRelrSz:
      return "DT_RELRSZ";
    case ElfDynTag::kRel:
      return "DT_REL";
    case ElfDynTag::kRelSz:
      return "DT_RELSZ";
    case ElfDynTag::kRelCount:
      return "DT_RELCOUNT";
    case ElfDynTag::kRela:
      return "DT_RELA";
    case ElfDynTag::kRelaSz:
      return "DT_RELASZ";
    case ElfDynTag::kRelaCount:
      return "DT_RELACOUNT";
    case ElfDynTag::kJmpRel:
      return "DT_JMPREL";
    case ElfDynTag::kPltRelSz:
      return "DT_PLTRELSZ";
    case ElfDynTag::kPltRel:
      return "DT_PLTREL";
    case ElfDynTag::kStrTab:
      return "DT_STRTAB";
    case ElfDynTag::kStrSz:
      return "DT_STRSZ";
    case ElfDynTag::kInitArray:
      return "DT_INIT_ARRAY";
    case ElfDynTag::kInitArraySz:
      return "DT_INIT_ARRAYSZ";
    case ElfDynTag::kFiniArray:
      return "DT_FINI_ARRAY";
    case ElfDynTag::kFiniArraySz:
      return "DT_FINI_ARRAYSZ";
    case ElfDynTag::kPreinitArray:
      return "DT_PREINIT_ARRAY";
    case ElfDynTag::kPreinitArraySz:
      return "DT_PREINIT_ARRAYSZ";
    default:
      __builtin_abort();
  }
}

template <ElfDynTag AddressTag, ElfDynTag SizeBytesTag, ElfDynTag CountTag>
struct DynamicTagError {
  static constexpr ConstString kMissingAddress{
      [] { return DynamicTagName(SizeBytesTag) + " without " + DynamicTagName(AddressTag); }};

  static constexpr ConstString kMissingSize{
      [] { return DynamicTagName(AddressTag) + " without " + DynamicTagName(SizeBytesTag); }};

  static constexpr ConstString kMisalignedAddress{
      [] { return DynamicTagName(AddressTag) + " has misaligned address"; }};

  static constexpr ConstString kMisalignedSize{[] {
    return DynamicTagName(SizeBytesTag) + " not a multiple of " + DynamicTagName(AddressTag) +
           " entry size";
  }};

  static constexpr ConstString kRead{[] {
    return "invalid address in " + DynamicTagName(AddressTag) + " or invalid size in " +
           DynamicTagName(SizeBytesTag);
  }};

  static constexpr ConstString kInvalidCount{
      [] { return DynamicTagName(CountTag) + " too large for " + DynamicTagName(SizeBytesTag); }};
};

}  // namespace elfldltl::internal

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_DYNAMIC_TAG_ERROR_H_
