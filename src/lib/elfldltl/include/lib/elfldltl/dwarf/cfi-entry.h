// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_CFI_ENTRY_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_CFI_ENTRY_H_

#include <cstdint>
#include <span>
#include <string_view>

#include "../diagnostics.h"
#include "../layout.h"
#include "encoding.h"
#include "section-data.h"

namespace elfldltl::dwarf {

// This appears in the CIE_id / CIE_pointer location in the header encoding
// to distinguish a CIE from an FDE.  This encoding of CIE_id / CIE_pointer
// is the primary difference between DWARF .debug_frame and GNU .eh_frame:
//
//  * In .debug_frame format, CIE_id is all ones to indicate a CIE and
//    otherwise it's CIE_pointer, an absolute offset into the .debug_frame
//    section.
//
//  * In .eh_frame format, CIE_id is zero to indicate a CIE and otherwise it's
//    a signed offset *backwards* relative to the location of that CIE_pointer
//    itself: CIE_position = FDE_position - CIE_pointer.
//
inline constexpr uint64_t kCfiCieId = -1;
inline constexpr uint64_t kEhFrameCieId = 0;

// This describes the information from decoding a CIE header in .debug_frame
// (or .eh_frame) format.  Usually many FDEs will refer to the same CIE, so
// each CIE should be decoded once and cached.
struct CfiCie {
  // This value is used in the fde_augmentation_data_size member when there
  // is a nonempty augmentation string that doesn't include "z".
  static constexpr size_t kUnknownAugmentation = -1;

  // Version of .debug_frame format.
  uint8_t version = 0;

  // Sizes of addresses and segment selectors in this CIE's FDEs.
  uint8_t address_size = 0;
  uint8_t segment_selector_size = 0;

  // Factors for the "scaling factor" DW_CFA_*_sf operations.
  uint32_t code_alignment_factor = 0;
  int32_t data_alignment_factor = 0;

  // DWARF register number representing the caller's PC.  If the
  // signal_frame member (below) is true, this is an exact PC that was
  // interrupted by some sort of exception frame.  If false (common case),
  // this is the PC in the caller that the current frame will return to.
  uint32_t return_address_register = 0;

  // These are the DWARF CFI instructions that modify the initial state
  // specified by the ABI (i.e. as if DW_CFA_same_value for all call-saved
  // and fixed registers, DW_CFA_undefined for all call-clobbered
  // registers, and an appropriate definition of the CFA for the entry
  // point to a function on this machine).
  std::span<const std::byte> initial_instructions;

  // The augmentation string is DWARF CFI's main extension mechanism.
  // It's always a NUL terminated UTF-8 string, though the NUL is not
  // included in the std::string_view::size().  Letters in the string
  // indicate nonstandard extensions, called "augmentations", which might
  // indicate that additional "augmentation data" follows.  The de facto
  // standard is to start with the "z" augmentation, which says:
  //  * The first piece of CIE augmentation data is a ULEB128 size of all
  //    the CIE augmentation data (including that ULEB128)
  //  * FDEs also start with a ULEB128 that gives the size of their own
  //    FDE augmentation data.  Other augmentation letters can indicate
  //    what is encoded after that ULEB128.
  std::string_view augmentation;

  // This is the augmentation data, not including its own size ULEB128.
  // If there were any unrecognized augmentation letters and no "z", then
  // no augmentation data can be decoded and this will be empty.
  std::span<const std::byte> augmentation_data;

  // If there is no "z" augmentation, then this indicates the size of known
  // augmentation data included in FDEs, if any.  If it's kUnknownAugmentation
  // then there was no "z" but some unrecognized augmentation letters, so it's
  // impossible to know how much augmentation data there is in each FDE.  Note
  // this also means that the CIE might include unrecognized augmentation data
  // that appears at the start of initial_instructions.
  size_t fde_augmentation_data_size = 0;

  // "P" augmentation can include a personality routine directly in the CIE.
  uint64_t personality = 0;

  // This is true when "z" augmentation indicates that each FDE's augmentation
  // data starts with a ULEB128 giving its own size.
  bool fde_augmentation_sized = false;

  // "R" augmentation can indicate a different encoding for the FDE
  // initial_location (start PC) and address_range (length) values.
  // Otherwise (in standard .debug_frame) they are absolute addresses of
  // the width the address_size field says.
  uint8_t fde_encoding = EncodedPtr::kPtr;

  // "L" augmentation can indicate an LSDA pointer is present in the FDE.
  // In standard .debug_frame, there is no LSDA pointer present.
  uint8_t lsda_encoding = EncodedPtr::kOmit;

  // "S" augmentation (.cfi_signal_frame) indicates that FDEs using this
  // CIE represent a set of exact registers where the "caller PC" is the
  // precise instruction of interest, rather than being a return address
  // of a function call instruction that precedes it.
  bool signal_frame = false;
};

// This describes the information from decoding an FDE header in
// .debug_frame (or .eh_frame) format according to its CIE (see above).
struct CfiFde {
  // Normalized CIE_pointer field: vaddr or .debug_frame offset of the CIE.
  // The decoded CfiCie is necessary to decode an FDE, so a reference to the
  // CfiCie used for decoding should usually travel alongside the CfiFde
  // rather than looking it up afresh via this pointer.
  uint64_t cie_pointer = 0;

  // This FDE covers a range of PCs starting at initial_location and covering
  // address_range bytes: [initial_location, initial_location + address_range).
  uint64_t initial_location = 0;
  uint64_t address_range = 0;

  // This all the FDE's augmentation data, whose exact interpretation is
  // determined by the CIE's augmentation string.
  std::span<const std::byte> augmentation_data;

  // This is set only if the CIE uses "L" augmentation to indicate the FDE
  // has an LSDA pointer (set by the .cfi_lsda assembly directive).
  uint64_t lsda = 0;

  // These are the DWARF CFI instructions for the FDE.  They modify the
  // initial state the CIE yields, and advance the PC from initial_location
  // to define additional rows.
  std::span<const std::byte> instructions;
};

struct CfiEntry {
 public:
  // Read a single CFI entry (either CIE or FDE) from the byte stream.
  // Failures will be sent to the diagnostics object with any error_args
  // appended to the basic details in the FormatError call.  The Elf template
  // parameter indicates the byte order.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  static constexpr std::optional<CfiEntry> Read(  //
      Diagnostics& diag, std::span<const std::byte> bytes, ErrorArgs&&... error_args) {
    // The error_args are passed by reference since they may be used again.
    std::optional data = SectionData::Read<Elf>(diag, bytes, error_args...);
    if (!data) {
      return std::nullopt;
    }
    std::optional cie_pointer = data->template read_offset<Elf>();

    if (!cie_pointer) [[unlikely]] {
      // Now the error_args can be moved.
      diag.FormatError("DWARF CFI entry of ", data->size_bytes(), " bytes is too small",
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }

    CfiEntry entry;
    entry.cie_pointer_ = *cie_pointer;
    entry.data_ = *data;
    if (entry.data_.format() == SectionData::Format::kDwarf32 &&
        entry.cie_pointer_ == kCfiCieId32) {
      // Extend to the 64-bit CIE_id.  In .eh_frame format, -1 is not the
      // CIE_id but -1 is always in invalid value since the relative CIE
      // position must be more than one byte away from the FDE to have space
      // for the CIE header; the CIE_id is 0 so no extension is needed.
      entry.cie_pointer_ = kCfiCieId;
    }
    return entry;
  }

  // After Read yields an entry from .eh_frame, normalize it to standard DWARF
  // CFI format as used in .debug_frame.  The argument must be the virtual
  // address that corresponds to the beginning of the buffer passed to Read.
  constexpr void NormalizeEhFrame(uint64_t vaddr) {
    if (cie_pointer_ == kEhFrameCieId) {
      // Normalize CIE_id value.
      cie_pointer_ = kCfiCieId;
    } else {
      // The CIE_pointer is an offset backwards from its own location.  The
      // vaddr passed in is the start of the FDE, which has its initial length
      // header before the CIE_pointer.
      vaddr += data_.initial_length_size();
      if (data_.format() == SectionData::Format::kDwarf32) {
        // Sign-extend from 32 bits, though it's stored as uint64_t and was
        // zero-extended from uint32_t by SectionData::read_offset().
        const uint32_t renarrowed = static_cast<uint32_t>(cie_pointer_);
        const int32_t offset_back = std::bit_cast<int32_t>(renarrowed);
        const int64_t sign_extended = static_cast<int64_t>(offset_back);
        cie_pointer_ = std::bit_cast<uint64_t>(sign_extended);
      }
      // The stored cie_pointer_ is signed 64 bits, though stored as uint64_t.
      cie_pointer_ = vaddr - std::bit_cast<int64_t>(cie_pointer_);
    }
  }

  // Like Read, but with NormalizeEhFrame applied to the results.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  static constexpr std::optional<CfiEntry> ReadEhFrame(  //
      Diagnostics& diag, std::span<const std::byte> bytes, Elf::size_type vaddr,
      ErrorArgs&&... error_args) {
    std::optional<CfiEntry> result = Read<Elf>(diag, bytes, std::forward<ErrorArgs>(error_args)...);
    if (result) {
      result->NormalizeEhFrame(vaddr);
    }
    return result;
  }

  // This does ReadEhFrame, but using a Memory object to read the FDE at vaddr.
  // The error_args do not need to include reporting the FDE vaddr passed here.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  static constexpr std::optional<CfiEntry> ReadEhFrameFromMemory(  //
      Diagnostics& diag, MemoryReader<typename Elf::size_type, std::byte> auto& memory,
      Elf::size_type vaddr, ErrorArgs&&... error_args) {
    auto bytes = memory.template ReadArray<std::byte>(vaddr);
    if (!bytes) [[unlikely]] {
      diag.FormatError("invalid FDE pointer ", FileAddress{vaddr},
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }
    return ReadEhFrame<Elf>(diag, *bytes, vaddr, " in FDE", FileAddress{vaddr},
                            std::forward<ErrorArgs>(error_args)...);
  }

  constexpr size_t size_bytes() const { return data_.size_bytes(); }

  // Returns true if this is a CIE, false if it's an FDE.
  constexpr bool IsCie() const { return cie_pointer_ == kCfiCieId; }

  // Returns std::nullopt for a CIE, or the CIE pointer for an FDE.
  constexpr std::optional<uint64_t> cie_pointer() const {
    if (IsCie()) {
      return std::nullopt;
    }
    return cie_pointer_;
  }

  // Read the CIE referenced by this FDE.  The Memory object is used to read
  // from virtual addresses in the same address space used in NormalizeEhFrame
  // on this object.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  constexpr std::optional<CfiEntry> ReadEhFrameCieFromMemory(  //
      Diagnostics& diag, MemoryReader<typename Elf::size_type, std::byte> auto& memory,
      ErrorArgs&&... error_args) const {
    using size_type = Elf::size_type;

    if (IsCie()) [[unlikely]] {
      diag.FormatError("DWARF CFI FDE is actually a CIE", std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }
    auto bytes = memory.template ReadArray<std::byte>(cie_pointer_);
    if (!bytes) [[unlikely]] {
      diag.FormatError("DWARF CFI FDE has invalid CIE pointer", FileAddress{cie_pointer_},
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    }
    // Pass error_args by reference since they may be used again below.
    std::optional<CfiEntry> result =
        ReadEhFrame<Elf>(diag, *bytes, static_cast<size_type>(cie_pointer_), error_args...);
    if (result) {
      if (!result->IsCie()) [[unlikely]] {
        diag.FormatError("DWARF CFI FDE's CIE pointer", FileAddress{cie_pointer_},
                         " points to another FDE", std::forward<ErrorArgs>(error_args)...);
        return std::nullopt;
      }
    }
    return result;
  }

  // Decode a CIE into the internal form described above.  The vaddr should be
  // that of the CIE itself if in .eh_frame format, where "PC-relative"
  // encodings might be used.  The error_args do not need to include reporting
  // the CIE vaddr passed here.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  std::optional<CfiCie> DecodeCie(Diagnostics& diag, uint64_t vaddr,
                                  ErrorArgs&&... error_args) const {
    auto truncated = [vaddr, &diag, &error_args...]() {
      diag.FormatError("DWARF CFI has truncated CIE", FileAddress{vaddr},
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    };

    // Skip the common header already read.
    std::span<const std::byte> bytes = data_.contents().subspan(data_.offset_size());

    if (bytes.size_bytes() < 2) [[unlikely]] {
      return truncated();
    }

    // The first thing is the version byte.
    CfiCie cie = {.version = static_cast<uint8_t>(bytes.front())};
    bytes = bytes.subspan(1);

    // The augmentation string is next and it's NUL-terminated.
    cie.augmentation = std::string_view{
        reinterpret_cast<const char*>(bytes.data()),
        bytes.size(),
    };
    if (size_t len = cie.augmentation.find('\0'); len != std::string_view::npos) {
      cie.augmentation = cie.augmentation.substr(0, len);
      bytes = bytes.subspan(len + 1);
    } else {
      return truncated();
    }

    switch (cie.version) {
      case 1:
      case 3:  // There is no version 2 of this format.
        // Older versions have implied address size.
        cie.address_size = sizeof(typename Elf::Addr);
        break;
      case 4:
        // DWARF 4 introduced explicit address size, and segment selectors.
        if (bytes.size_bytes() < 2) [[unlikely]] {
          return truncated();
        }
        cie.address_size = static_cast<uint8_t>(bytes[0]);
        cie.segment_selector_size = static_cast<uint8_t>(bytes[1]);
        bytes = bytes.subspan(2);
        break;
      default:
        diag.FormatError("DWARF CFI has unsupported CIE version ", cie.version, FileAddress{vaddr},
                         std::forward<ErrorArgs>(error_args)...);
        return std::nullopt;
    }

    // In the ancient G++ v2 format, there is a pointer right after the
    // augmentation string, before the standard fields.
    if (cie.augmentation.starts_with("eh")) {
      if (bytes.size_bytes() < cie.address_size) [[unlikely]] {
        return truncated();
      }
      bytes = bytes.subspan(cie.address_size);
    }

    // Next is the code_alignment_factor.
    if (auto factor = Uleb128::Read(bytes)) {
      cie.code_alignment_factor = static_cast<uint32_t>(factor->value);
      if (cie.code_alignment_factor != factor->value) [[unlikely]] {
        diag.FormatError("DWARF CFI CIE has unreasonable code_alignment_factor ", factor->value,
                         FileAddress{vaddr}, std::forward<ErrorArgs>(error_args)...);
        return std::nullopt;
      }
      bytes = bytes.subspan(factor->size_bytes);
    } else [[unlikely]] {
      return truncated();
    }

    // Next is the data_alignment_factor.
    if (auto factor = Sleb128::Read(bytes)) {
      cie.data_alignment_factor = static_cast<int32_t>(factor->value);
      if (cie.data_alignment_factor != factor->value) [[unlikely]] {
        diag.FormatError("DWARF CFI CIE has unreasonable data_alignment_factor ", factor->value,
                         FileAddress{vaddr}, std::forward<ErrorArgs>(error_args)...);
        return std::nullopt;
      }
      bytes = bytes.subspan(factor->size_bytes);
    } else [[unlikely]] {
      return truncated();
    }

    // Next is the return_address_register.  Before DWARF 3, this was one byte.
    if (cie.version < 3) {
      if (bytes.empty()) [[unlikely]] {
        return truncated();
      }
      cie.return_address_register = static_cast<uint8_t>(bytes.front());
      bytes = bytes.subspan(1);
    } else if (auto rar = Uleb128::Read(bytes)) {
      cie.return_address_register = static_cast<uint32_t>(rar->value);
      if (cie.return_address_register != rar->value) [[unlikely]] {
        diag.FormatError("DWARF CFI CIE has unreasonable return_address_register ", rar->value,
                         FileAddress{vaddr}, std::forward<ErrorArgs>(error_args)...);
        return std::nullopt;
      }
      bytes = bytes.subspan(rar->size_bytes);
    } else [[unlikely]] {
      return truncated();
    }

    // Grok the augmentation string, starting with "z".
    if (cie.augmentation.starts_with('z')) {
      cie.fde_augmentation_sized = true;
      auto data_size = Uleb128::Read(bytes);
      if (!data_size || bytes.size_bytes() - data_size->size_bytes < data_size->value)
          [[unlikely]] {
        return truncated();
      }
      bytes = bytes.subspan(data_size->size_bytes);
      cie.augmentation_data = bytes.first(data_size->value);
      cie.initial_instructions = bytes.subspan(data_size->value);
    } else {
      // This will be reduced later.
      cie.augmentation_data = bytes;
    }

    // Interpret other known augmentations after the "z".  This consumes from
    // bytes, which is also cie.augmentation_data when sized.
    for (const char c : cie.augmentation.substr(cie.fde_augmentation_sized ? 1 : 0)) {
      switch (c) {
        case 'L':
          // CIE augmentation gives LSDA encoding used in FDE augmentation.
          if (bytes.empty()) [[unlikely]] {
            return truncated();
          }
          cie.lsda_encoding = static_cast<uint8_t>(bytes.front());
          bytes = bytes.subspan(1);
          if (!cie.fde_augmentation_sized &&
              cie.fde_augmentation_data_size != CfiCie::kUnknownAugmentation) {
            cie.fde_augmentation_data_size +=
                EncodedPtr::EncodedSize(cie.lsda_encoding, cie.address_size);
          }
          break;

        case 'P': {
          // CIE augmentation gives the personality routine vaddr.
          if (bytes.empty()) [[unlikely]] {
            return truncated();
          }
          uint8_t encoding = static_cast<uint8_t>(bytes.front());
          bytes = bytes.subspan(1);
          if (auto personality = EncodedPtr::Read<Elf>(encoding, bytes)) {
            cie.personality = personality->ptr;
          } else [[unlikely]] {
            return truncated();
          }
          switch (EncodedPtr::Modifier(encoding)) {
            case EncodedPtr::kAbs:
              break;
            case EncodedPtr::kPcrel:
              // Add in the vaddr of CIE and the distance from there to here.
              cie.personality += vaddr;
              cie.personality += data_.initial_length_size();
              cie.personality += bytes.data() - data_.contents().data();
              break;
            default:
              diag.FormatError("DWARF CFI uses invalid personality routine encoding ", encoding,
                               FileAddress{vaddr}, std::forward<ErrorArgs>(error_args)...);
              return std::nullopt;
          }
          break;
        }

        case 'R':
          // FDE initial_instructions and address_range use a special encoding.
          if (bytes.empty()) [[unlikely]] {
            return truncated();
          }
          cie.fde_encoding = static_cast<uint8_t>(bytes.front());
          bytes = bytes.subspan(1);
          break;

        case 'S':
          cie.signal_frame = true;
          break;

        case 'B':
        case 'G':
          // These are Aarch64 extensions we don't support but they don't use
          // any augmentation data so it's safe to ignore them.
          break;

        default:
          // Any unknown letter might have augmentation data.
          if (!cie.fde_augmentation_sized) {
            cie.fde_augmentation_data_size = CfiCie::kUnknownAugmentation;
          }
          break;
      }
    }

    // Without "z", a (suspected) size of augmentation data is known only now.
    if (!cie.fde_augmentation_sized) {
      size_t size = cie.augmentation_data.size_bytes() - bytes.size_bytes();
      cie.augmentation_data = cie.augmentation_data.first(size);
      cie.initial_instructions = bytes;
    }

    return cie;
  }

  // Decode an FDE into the internal form described above, after the CIE it
  // points to has already been decoded.  The vaddr should be that of the FDE
  // itself if in .eh_frame format, where "PC-relative" encodings might be
  // used. The error_args do not need to include reporting the FDE vaddr passed
  // here.
  template <ElfApi Elf = Elf<>, class Diagnostics, typename... ErrorArgs>
  std::optional<CfiFde> DecodeFde(Diagnostics& diag, uint64_t vaddr, const CfiCie& cie,
                                  ErrorArgs&&... error_args) const {
    auto truncated = [vaddr, &diag, &error_args...](uint8_t encoding) {
      diag.FormatError("DWARF CFI has invalid encoding ", static_cast<unsigned int>(encoding),
                       " or truncated FDE", FileAddress{vaddr},
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    };

    // Normalize an encoded address to be absolute.
    auto normalize = [fde_vaddr = vaddr, &diag, &error_args...](  //
                         uint64_t encoding_vaddr, EncodedPtr& encoded) {
      if (EncodedPtr::Signed(encoded.encoding)) {
        encoded.ptr = std::bit_cast<uint64_t>(encoded.sptr);
      }
      switch (EncodedPtr::Modifier(encoded.encoding)) {
        case EncodedPtr::kAbs:
          break;
        case EncodedPtr::kPcrel:
          encoded.ptr += encoding_vaddr;
          break;
        default:
          [[unlikely]];
          diag.FormatError("DWARF CFI uses unsupported encoding ",
                           static_cast<unsigned int>(encoded.encoding), " in FDE",
                           FileAddress{fde_vaddr}, std::forward<ErrorArgs>(error_args)...);
          return false;
      }
      return true;
    };

    // Skip the common header already read.
    std::span<const std::byte> bytes = data_.contents().subspan(data_.offset_size());

    // The data_.contents() started after the initial length header, and then
    // the CIE_pointer was just skipped to reach the vaddr of the encoded PC.
    const uint64_t pc_vaddr = vaddr + data_.initial_length_size() + data_.offset_size();
    auto pc = EncodedPtr::Read<Elf>(cie.fde_encoding, bytes, cie.address_size);
    if (!pc) [[unlikely]] {
      return truncated(cie.fde_encoding);
    }
    bytes = bytes.subspan(pc->encoded_size);

    // One encoding byte indicates the encoding for both initial_location (pc)
    // and address_range (length), but the length ignores the relative and
    // indirect flags and only uses the same base encoding: usually the PC is
    // encoded as PC-relative and the count as absolute, of the same width.
    const uint8_t length_encoding = EncodedPtr::Type(cie.fde_encoding);
    auto length = EncodedPtr::Read<Elf>(length_encoding, bytes, cie.address_size);
    if (!length) [[unlikely]] {
      return truncated(cie.fde_encoding);
    }
    bytes = bytes.subspan(length->encoded_size);

    if (!normalize(pc_vaddr, *pc)) [[unlikely]] {
      return std::nullopt;
    }

    CfiFde fde = {
        .cie_pointer = cie_pointer_,
        .initial_location = pc->ptr,
        .address_range = length->ptr,
    };

    // Figure out the bounds of the augmentation data.
    if (cie.fde_augmentation_sized) {
      auto size = Uleb128::Read(bytes);
      if (!size) [[unlikely]] {
        return truncated(EncodedPtr::kUleb128);
      }
      bytes = bytes.subspan(size->size_bytes);
      if (bytes.size_bytes() < size->value) [[unlikely]] {
        return truncated(EncodedPtr::kUleb128);
      }
      fde.augmentation_data = bytes.first(size->value);
      bytes = bytes.subspan(size->value);
    } else if (cie.fde_augmentation_data_size == CfiCie::kUnknownAugmentation) [[unlikely]] {
      diag.FormatError("DWARF CFI has CIE", FileAddress{cie_pointer_},
                       " using unrecognized augmentation string \"", cie.augmentation, "\"",
                       std::forward<ErrorArgs>(error_args)...);
      return std::nullopt;
    } else {
      fde.augmentation_data = bytes.first(cie.fde_augmentation_data_size);
      bytes = bytes.subspan(cie.fde_augmentation_data_size);
    }

    // After that, the rest is the CFI instructions.
    fde.instructions = bytes;

    // The only part of the augmentation data we know about is the LSDA.  When
    // there is no LSDA, the empty EncodedPtr has a zero .ptr anyway.
    auto lsda = EncodedPtr::Read<Elf>(cie.lsda_encoding, fde.augmentation_data, cie.address_size);
    if (!lsda) [[unlikely]] {
      return truncated(cie.lsda_encoding);
    }
    fde.lsda = lsda->ptr;

    return fde;
  }

 private:
  static constexpr uint32_t kCfiCieId32 = -1;

  uint64_t cie_pointer_ = 0;
  SectionData data_;
};

}  // namespace elfldltl::dwarf

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_DWARF_CFI_ENTRY_H_
