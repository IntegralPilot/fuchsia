// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/cfi_module.h"

#include <elf.h>
#include <lib/fit/result.h>

#include <cinttypes>
#include <cstdint>
#include <limits>
#include <map>
#include <string>

#include <safemath/checked_math.h>

#include "src/lib/unwinder/cfi_parser.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/module.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

namespace {

// The CIE ID that distinguishes a CIE from an FDE. In the eh_frame section (version 1), the CIE ID
// is always zero. In the debug_frame section (version 4), the value is either a 4 byte or 8 byte
// value, depending on whether the section is encoded as 32 bit or 64 bit DWARF. Regardless of the
// size of the value, the value shall always be filled with 0xFF.
const constexpr uint32_t kDwarf32CieId = std::numeric_limits<uint32_t>::max();
const constexpr uint64_t kDwarf64CieId = std::numeric_limits<uint64_t>::max();

// Check and return the size of each entry in the table. It's doubled because each entry contains
// 2 addresses, i.e., the start_pc and the fde_offset.
[[nodiscard]] Error DecodeTableEntrySize(uint8_t table_enc, uint64_t& res) {
  if (table_enc == 0xFF) {  // DW_EH_PE_omit
    return Error("no binary search table");
  }
  if ((table_enc & 0xF0) != 0x30) {
    return Error("invalid table_enc");
  }
  switch (table_enc & 0x0F) {
    case 0x02:  // DW_EH_PE_udata2  A 2 bytes unsigned value.
    case 0x0A:  // DW_EH_PE_sdata2  A 2 bytes signed value.
      res = 4;
      return Success();
    case 0x03:  // DW_EH_PE_udata4  A 4 bytes unsigned value.
    case 0x0B:  // DW_EH_PE_sdata4  A 4 bytes signed value.
      res = 8;
      return Success();
    case 0x04:  // DW_EH_PE_udata8  An 8 bytes unsigned value.
    case 0x0C:  // DW_EH_PE_sdata8  An 8 bytes signed value.
      res = 16;
      return Success();
    default:
      return Error("unsupported table_enc: %#x", table_enc);
  }
}

// Decode the length and cie_ptr field in CIE/FDE. It's awkward because we want to support both
// .eh_frame format and .debug_frame format.
//
// When returned, |ptr| will point to the beginning of the body, and |end| will point to the end
// of the entry.
[[nodiscard]] Error DecodeCieFdeHdrAndAdvance(Memory* elf, UnwindTableSectionType section_type,
                                              uint64_t& ptr, uint64_t& end, uint64_t& cie_id) {
  uint32_t short_length;
  if (auto err = elf->ReadAndAdvance(ptr, short_length); err.has_err()) {
    return err;
  }
  if (short_length == 0) {
    return Error("not a valid CIE/FDE");
  }

  // The first 4 bytes of the length set to 0xFFFFFFFF indicates that this is 64 bit DWARF format
  // and we can read the cie_id directly below after reading the full 8 byte length.
  if (short_length != 0xFFFFFFFF) {
    end = ptr + short_length;
  } else {
    uint64_t length;
    if (auto err = elf->ReadAndAdvance(ptr, length); err.has_err()) {
      return err;
    }
    end = ptr + length;
  }
  // The cie_id is 8-bytes only in debug_frame and it's a 64-bit DWARF format.
  if (section_type == UnwindTableSectionType::kDebugFrame && short_length == 0xFFFFFFFF) {
    if (auto err = elf->ReadAndAdvance(ptr, cie_id); err.has_err()) {
      return err;
    }
  } else {
    uint32_t short_cie_id;
    if (auto err = elf->ReadAndAdvance(ptr, short_cie_id); err.has_err()) {
      return err;
    }

    cie_id = short_cie_id;
  }
  return Success();
}

// Returns true if the given |cie_id| value is correct for |section_type|.
[[nodiscard]] bool CheckCieId(UnwindTableSectionType section_type, uint64_t cie_id) {
  switch (section_type) {
    case UnwindTableSectionType::kEhFrame:
      return cie_id == 0;
    case UnwindTableSectionType::kDebugFrame:
      return cie_id == kDwarf32CieId || cie_id == kDwarf64CieId;
  }
}

}  // namespace

// static.
fit::result<Error, std::unique_ptr<CfiModule>> CfiModule::FromLoadedElfModule(
    const LoadedElfModule& loaded_elf_module) {
  // CfiModule's constructor is private to force the use of this method, which guarantees that only
  // valid objects are returned, or errors. Unfortunately that means we also cannot use
  // std::make_unique here.
  auto cfi_module = std::unique_ptr<CfiModule>(new CfiModule(loaded_elf_module));

  if (auto err = cfi_module->Load(); err.is_error()) {
    return err.take_error();
  }

  return fit::ok(std::move(cfi_module));
}

// Load the .eh_frame and/or .debug_frame.
//
// If |address_mode_| is kProcess, then .eh_frame will be loaded from the loaded segment in process
// memory, and .debug_frame will never be loaded. Otherwise, the module will read from the given ELF
// file on disk. Depending on the method of compilation for the particular TU of a given PC, the
// .eh_frame or .debug_frame section may be used. .debug_frame is preferred and therefore inspected
// first for a PC.
//
// See the Linux Standard Base Core Specification
// https://refspecs.linuxfoundation.org/LSB_5.0.0/LSB-Core-generic/LSB-Core-generic/ehframechpt.html
// and a reference implementation in LLVM
// https://github.com/llvm/llvm-project/blob/main/libunwind/src/DwarfParser.hpp
// https://github.com/llvm/llvm-project/blob/main/libunwind/src/EHHeaderParser.hpp
fit::result<Error> CfiModule::Load() {
  if (!binary_elf_) {
    return fit::error(Error("no elf memory"));
  }

  // ElfModule already handles the ELF header and program headers.
  // We just need them here to find the unwind sections.
  Elf64_Ehdr ehdr;
  if (auto err = binary_elf_->Read(loaded_elf_module_.load_address(), ehdr); err.has_err()) {
    return fit::error(err);
  }

  auto eh_frame_err = LoadEhFrame(ehdr);
  auto debug_frame_err = LoadDebugFrame(ehdr);

  if (eh_frame_err.has_err() && debug_frame_err.has_err()) {
    return fit::error(
        Error("Failed to load both eh_frame (err=\"%s\") and debug_frame (\"%s\") sections\n.",
              eh_frame_err.msg().c_str(), debug_frame_err.msg().c_str()));
  }

  return fit::ok();
}

Error CfiModule::LoadEhFrame(const Elf64_Ehdr& ehdr) {
  // ==============================================================================================
  // Load from the .eh_frame_hdr section.
  // This section may not always be present - it's purely an optimization when we get to use it.
  // When not present, we have to resort to linearly scanning the FDE list.
  // ==============================================================================================
  eh_frame_hdr_ptr_ = 0;
  auto phdr = loaded_elf_module_.GetSegmentByType(PT_GNU_EH_FRAME);
  if (phdr.is_error()) {
    return phdr.error_value();
  }

  if (loaded_elf_module_.mode() == Module::AddressMode::kProcess) {
    if (!safemath::CheckAdd(loaded_elf_module_.load_address(), phdr->p_vaddr)
             .AssignIfValid(&eh_frame_hdr_ptr_)) {
      return Error("Calculated eh_frame_hdr start overflows");
    }
  } else {
    if (!safemath::CheckAdd(loaded_elf_module_.load_address(), phdr->p_offset)
             .AssignIfValid(&eh_frame_hdr_ptr_)) {
      return Error("Calculated eh_frame_hdr start overflows");
    }
  }

  if (eh_frame_hdr_ptr_ > 0) {
    auto p = eh_frame_hdr_ptr_;
    uint8_t version;
    if (auto err = binary_elf_->ReadAndAdvance(p, version); err.has_err()) {
      return err;
    }
    if (version != 1) {
      return Error("unknown eh_frame_hdr version %d", version);
    }

    uint8_t eh_frame_ptr_enc;
    uint8_t fde_count_enc;
    uint64_t eh_frame_ptr;  // not used
    if (auto err = binary_elf_->ReadAndAdvance(p, eh_frame_ptr_enc); err.has_err()) {
      return err;
    }
    if (auto err = binary_elf_->ReadAndAdvance(p, fde_count_enc); err.has_err()) {
      return err;
    }
    if (auto err = binary_elf_->ReadAndAdvance(p, table_enc_); err.has_err()) {
      return err;
    }
    if (auto err = DecodeTableEntrySize(table_enc_, table_entry_size_); err.has_err()) {
      return err;
    }
    if (auto err = binary_elf_->ReadEncodedAndAdvance(p, eh_frame_ptr, eh_frame_ptr_enc,
                                                      eh_frame_hdr_ptr_);
        err.has_err()) {
      return err;
    }
    if (auto err =
            binary_elf_->ReadEncodedAndAdvance(p, fde_count_, fde_count_enc, eh_frame_hdr_ptr_);
        err.has_err()) {
      return err;
    }
    table_ptr_ = p;

    if (fde_count_ == 0) {
      return Error("empty binary search table");
    }
  }

  return Success();
}

Error CfiModule::LoadDebugFrame(const Elf64_Ehdr& ehdr) {
  // ==============================================================================================
  // Load from the .debug_frame section, if present.
  // ==============================================================================================
  debug_frame_ptr_ = 0;
  debug_frame_end_ = 0;

  // Section headers and .debug_frame section are not loaded.
  if (loaded_elf_module_.mode() == Module::AddressMode::kProcess) {
    return Error("debug_frame section not present when AddressMode == kProcess");
  }

  // if ehdr.e_shstrndx is 0, it means there's no section info, i.e., the binary is stripped.
  if (!ehdr.e_shstrndx) {
    return Error("no section info, is this a stripped binary?");
  }

  auto shdr = loaded_elf_module_.GetSectionByName(".debug_frame");
  if (shdr.is_error()) {
    return shdr.error_value();
  }

  if (!safemath::CheckAdd(loaded_elf_module_.load_address(), shdr->sh_offset)
           .AssignIfValid(&debug_frame_ptr_)) {
    return Error("Overflowed finding debug_frame start.");
  }

  if (!safemath::CheckAdd(debug_frame_ptr_, shdr->sh_size).AssignIfValid(&debug_frame_end_)) {
    return Error("Overflowed finding debug_frame end.");
  }

  if (auto err = BuildDebugFrameMap(); err.has_err()) {
    return err;
  }
  return Success();
}

fit::result<Error, bool> CfiModule::Step(Memory* stack, const Registers& current,
                                         Registers& next) const {
  DwarfCie cie;
  auto cfi_parser = PrepareToStep(current, cie);
  if (cfi_parser.is_error()) {
    return cfi_parser.take_error();
  }

  if (auto err = cfi_parser->Step(stack, cie.return_address_register, current, next);
      err.has_err()) {
    return fit::error(err);
  }

  return fit::ok(cie.is_signal_frame);
}

void CfiModule::AsyncStep(AsyncMemory* stack, const Registers& current,
                          fit::callback<void(Error, Registers)> cb) const {
  DwarfCie cie;
  auto cfi_parser = PrepareToStep(current, cie);
  if (cfi_parser.is_error()) {
    cb(cfi_parser.error_value(), Registers(current.arch()));
    return;
  }

  // Make sure |cfi_parser| lives as long as the callback.
  cfi_parser->AsyncStep(stack, cie.return_address_register, current,
                        [cfi_parser = std::move(cfi_parser), cb = std::move(cb)](
                            Error err, Registers regs) mutable { cb(err, std::move(regs)); });
}

fit::result<Error, std::unique_ptr<CfiParser>> CfiModule::PrepareToStep(const Registers& current,
                                                                        DwarfCie& cie) const {
  uint64_t pc;
  if (auto err = current.GetPC(pc); err.has_err()) {
    return fit::error(err);
  }
  if (!loaded_elf_module_.IsValidPC(pc)) {
    return fit::error(Error("pc %#" PRIx64 " is outside of the executable area", pc));
  }

  DwarfFde fde;

  // Search for .debug_frame first. This is preferred over .eh_frame sections in situations where we
  // might encounter both since debug_frame is likely to contain more complete information about all
  // callsites that might be optimized out of eh_frame sections for binary size or performance
  // reasons. Callers must provide an appropriate binary for the debug_frame section to be found,
  // namely from either an unstripped binary or a separated debug info file corresponding to the
  // program being unwound.
  if (auto debug_frame_err = SearchDebugFrame(pc, cie, fde); debug_frame_err.has_err()) {
    if (auto eh_frame_err = SearchEhFrame(pc, cie, fde); eh_frame_err.has_err()) {
      return fit::error(Error(debug_frame_err.msg() + "; " + eh_frame_err.msg()));
    }
  }

  auto cfi_parser =
      std::make_unique<CfiParser>(current.arch(), loaded_elf_module_.size(),
                                  cie.code_alignment_factor, cie.data_alignment_factor);

  // Parse instructions in CIE first.
  if (auto err = cfi_parser->ParseInstructions(binary_elf_, cie.instructions_begin,
                                               cie.instructions_end, -1);
      err.has_err()) {
    return fit::error(err);
  }

  cfi_parser->Snapshot();

  // Parse instructions in FDE until pc.
  if (auto err = cfi_parser->ParseInstructions(binary_elf_, fde.instructions_begin,
                                               fde.instructions_end, pc - fde.pc_begin);
      err.has_err()) {
    return fit::error(err);
  }

  return fit::ok(std::move(cfi_parser));
}

Error CfiModule::BinarySearchEhFrame(uint64_t pc, DwarfCie& cie, DwarfFde& fde) const {
  // Binary search for fde_ptr in the range [low, high).
  uint64_t low = 0;
  uint64_t high = fde_count_;
  while (low + 1 < high) {
    uint64_t mid = (low + high) / 2;
    uint64_t addr = table_ptr_ + mid * table_entry_size_;
    uint64_t mid_pc;
    if (auto err = binary_elf_->ReadEncoded(addr, mid_pc, table_enc_, eh_frame_hdr_ptr_);
        err.has_err()) {
      return err;
    }
    if (pc < mid_pc) {
      high = mid;
    } else {
      low = mid;
    }
  }

  uint64_t addr = table_ptr_ + low * table_entry_size_ + table_entry_size_ / 2;

  uint64_t fde_ptr;
  if (auto err = binary_elf_->ReadEncoded(addr, fde_ptr, table_enc_, eh_frame_hdr_ptr_);
      err.has_err()) {
    return err;
  }

  if (auto err = DecodeFde(UnwindTableSectionType::kEhFrame, binary_elf_, fde_ptr, cie, fde);
      err.has_err()) {
    return err;
  }
  return Success();
}

Error CfiModule::LinearSearchEhFrame(uint64_t pc, DwarfCie& cie, DwarfFde& fde) const {
  // A length of 0 indicates the terminator FDE.
  uint64_t addr = eh_frame_begin_;

  while (true) {
    if (auto err = DecodeFde(UnwindTableSectionType::kEhFrame, binary_elf_, addr, cie, fde);
        err.has_err()) {
      return err;
    }

    if (fde.instructions_end == 0) {
      // Got the terminator FDE without finding an FDE containing pc, now we give up.
      return Error("Failed to find an FDE containing pc!");
    } else if (pc >= fde.pc_begin && pc < fde.pc_end) {
      break;
    }
  }

  return Success();
}

Error CfiModule::SearchEhFrame(uint64_t pc, DwarfCie& cie, DwarfFde& fde) const {
  if (eh_frame_hdr_ptr_ != 0) {
    // We always expect the eh_frame_hdr section to be complete, so there's no fallback mechanism
    // for errors.
    if (auto err = BinarySearchEhFrame(pc, cie, fde); err.has_err()) {
      return err;
    }
  } else if (eh_frame_begin_ != 0) {
    auto err = LinearSearchEhFrame(pc, cie, fde);
    if (err.ok()) {
      return Success();
    }

  } else {
    return Error("Do not have eh_frame_hdr or eh_frame pointers");
  }

  if (pc < fde.pc_begin || pc >= fde.pc_end) {
    return Error("cannot find FDE for pc %#" PRIx64, pc);
  }
  return Success();
}

Error CfiModule::SearchDebugFrame(uint64_t pc, DwarfCie& cie, DwarfFde& fde) const {
  if (!debug_frame_ptr_) {
    return Error("no .debug_frame section");
  } else if (debug_frame_map_.empty()) {
    return Error("debug frame map not loaded");
  }

  auto debug_frame_map_it = debug_frame_map_.upper_bound(pc);
  if (debug_frame_map_it == debug_frame_map_.begin()) {
    return Error("cannot find FDE for pc %#" PRIx64 " in .debug_frame", pc);
  }
  debug_frame_map_it--;
  uint64_t fde_ptr = debug_frame_map_it->second;

  if (auto err = DecodeFde(UnwindTableSectionType::kDebugFrame, debug_info_elf_, fde_ptr, cie, fde);
      err.has_err()) {
    return err;
  }
  if (pc < fde.pc_begin || pc >= fde.pc_end) {
    return Error("cannot find FDE for pc %#" PRIx64 " in .debug_frame", pc);
  }
  return Success();
}

// In order to read less memory, this function assumes the address encoding of all CIEs is the same,
// so that it only needs to decode the first CIE.
Error CfiModule::BuildDebugFrameMap() {
  debug_frame_map_.clear();
  uint8_t fde_address_encoding = 0;
  for (uint64_t p = debug_frame_ptr_, next_p; p < debug_frame_end_; p = next_p) {
    uint64_t hdr_p = p;
    uint64_t cie_id;
    if (auto err = DecodeCieFdeHdrAndAdvance(debug_info_elf_, UnwindTableSectionType::kDebugFrame,
                                             p, next_p, cie_id);
        err.has_err()) {
      return err;
    }
    if (CheckCieId(UnwindTableSectionType::kDebugFrame, cie_id)) {
      if (fde_address_encoding) {
        // Assume address encoding is the same for all CIEs. Skip all other CIEs to accelerate.
        continue;
      }
      DwarfCie cie;
      // This will only be called once, so it's fine that |DecodeCie| decodes the header again.
      if (auto err = DecodeCie(UnwindTableSectionType::kDebugFrame, debug_info_elf_, hdr_p, cie);
          err.has_err()) {
        return err;
      }
      fde_address_encoding = cie.fde_address_encoding;
    } else {  // is FDE
      // We only need pc_begin, so don't go through |DecodeFde|.
      uint64_t pc_begin;
      if (auto err = binary_elf_->ReadEncoded(p, pc_begin, fde_address_encoding,
                                              loaded_elf_module_.load_address());
          err.has_err()) {
        return err;
      }
      debug_frame_map_.emplace(pc_begin, hdr_p);
    }
  }

  if (debug_frame_map_.empty()) {
    return Error("empty .debug_frame");
  }
  return Success();
}

Error CfiModule::DecodeCie(UnwindTableSectionType type, Memory* elf, uint64_t cie_ptr,
                           DwarfCie& cie) const {
  uint64_t cie_id;
  if (auto err = DecodeCieFdeHdrAndAdvance(elf, type, cie_ptr, cie.instructions_end, cie_id);
      err.has_err()) {
    return err;
  }

  if (!CheckCieId(type, cie_id)) {
    return Error("CIE_ID invalid for this version = %d, cie_id = %" PRIx64, type, cie_id);
  }

  // Versions should match.
  uint8_t this_version;
  if (auto err = elf->ReadAndAdvance(cie_ptr, this_version); err.has_err()) {
    return err;
  }
  if (this_version != type) {
    return Error("unexpected CIE version: %d", this_version);
  }

  std::string augmentation_string;
  while (true) {
    char ch;
    if (auto err = elf->ReadAndAdvance(cie_ptr, ch); err.has_err()) {
      return err;
    }
    if (ch) {
      augmentation_string.push_back(ch);
    } else {
      break;
    }
  }

  if (type == UnwindTableSectionType::kDebugFrame) {
    // Read the address_size.
    uint8_t address_size;
    if (auto err = elf->ReadAndAdvance(cie_ptr, address_size); err.has_err()) {
      return err;
    }
    // Set fde_address_encoding to DW_EH_PE_datarel so that we can set the base to elf_ptr_.
    switch (address_size) {
      case 2:
        cie.fde_address_encoding = 0x32;
        break;
      case 4:
        cie.fde_address_encoding = 0x33;
        break;
      case 8:
        cie.fde_address_encoding = 0x34;
        break;
      default:
        return Error("unsupported CIE address_size: %d", address_size);
    }
    // Skip the segment_selector_size.
    cie_ptr++;
  }

  if (auto err = elf->ReadULEB128AndAdvance(cie_ptr, cie.code_alignment_factor); err.has_err()) {
    return err;
  }
  if (auto err = elf->ReadSLEB128AndAdvance(cie_ptr, cie.data_alignment_factor); err.has_err()) {
    return err;
  }
  if (type == UnwindTableSectionType::kDebugFrame) {
    uint64_t return_address_register;
    if (auto err = elf->ReadULEB128AndAdvance(cie_ptr, return_address_register); err.has_err()) {
      return err;
    }
    cie.return_address_register = static_cast<RegisterID>(return_address_register);
  } else {
    if (auto err = elf->ReadAndAdvance(cie_ptr, cie.return_address_register); err.has_err()) {
      return err;
    }
  }

  if (augmentation_string.empty()) {
    cie.instructions_begin = cie_ptr;
    cie.fde_have_augmentation_data = false;
  } else {
    // DWARF standard doesn't say anything about the possibility of the augmentation string and
    // we have never seen a use case of augmentation string in .debug_frame, which is understandable
    // because the augmentation string is mainly useful for unwinding during an exception.
    // For now, we don't support it.
    if (type == UnwindTableSectionType::kDebugFrame) {
      return Error("unsupported augmentation string in .debug_frame: %s",
                   augmentation_string.c_str());
    }
    if (augmentation_string[0] != 'z') {
      return Error("invalid augmentation string: %s", augmentation_string.c_str());
    }
    uint64_t augmentation_length;
    if (auto err = elf->ReadULEB128AndAdvance(cie_ptr, augmentation_length); err.has_err()) {
      return err;
    }
    cie.instructions_begin = cie_ptr + augmentation_length;
    cie.fde_have_augmentation_data = true;

    for (char ch : augmentation_string) {
      switch (ch) {
        case 'L':
          // LSDA (language-specific data area) is used by some languages such as C++ to ensure
          // the correct destruction of objects on stack. We don't need to handle it.
          uint8_t lsda_encoding;
          if (auto err = elf->ReadAndAdvance(cie_ptr, lsda_encoding); err.has_err()) {
            return err;
          }
          break;
        case 'P':
          // The personality routine is used to handle language and vendor-specific tasks to ensure
          // the correct unwinding. We don't need to handle it.
          uint8_t enc;
          if (auto err = elf->ReadAndAdvance(cie_ptr, enc); err.has_err()) {
            return err;
          }
          uint64_t personality;
          if (auto err = elf->ReadEncodedAndAdvance(cie_ptr, personality, enc, 0); err.has_err()) {
            return err;
          }
          break;
        case 'R':
          if (auto err = elf->ReadAndAdvance(cie_ptr, cie.fde_address_encoding); err.has_err()) {
            return err;
          }
          break;
        case 'S':
          cie.is_signal_frame = true;
          break;
      }
    }
  }

  return Success();
}

Error CfiModule::DecodeFde(UnwindTableSectionType section_type, Memory* elf, uint64_t fde_ptr,
                           DwarfCie& cie, DwarfFde& fde) const {
  uint64_t cie_offset;
  if (auto err =
          DecodeCieFdeHdrAndAdvance(elf, section_type, fde_ptr, fde.instructions_end, cie_offset);
      err.has_err()) {
    return err;
  }

  if (fde.instructions_end == 0) {
    // This is a terminator FDE.
    return Success();
  }

  uint64_t cie_ptr;
  if (section_type == UnwindTableSectionType::kDebugFrame) {
    cie_ptr = debug_frame_ptr_ + cie_offset;
  } else {
    cie_ptr = fde_ptr - 4 - cie_offset;
  }
  if (auto err = DecodeCie(section_type, elf, cie_ptr, cie); err.has_err()) {
    return err;
  }

  if (auto err = elf->ReadEncodedAndAdvance(fde_ptr, fde.pc_begin, cie.fde_address_encoding,
                                            loaded_elf_module_.load_address());
      err.has_err()) {
    return err;
  }
  if (auto err = elf->ReadEncodedAndAdvance(fde_ptr, fde.pc_end, cie.fde_address_encoding & 0x0F);
      err.has_err()) {
    return err;
  }
  fde.pc_end += fde.pc_begin;

  if (cie.fde_have_augmentation_data) {
    uint64_t augmentation_length;
    if (auto err = elf->ReadULEB128AndAdvance(fde_ptr, augmentation_length); err.has_err()) {
      return err;
    }
    // We don't really care about the augmentation data.
    fde_ptr += augmentation_length;
  }
  fde.instructions_begin = fde_ptr;

  return Success();
}

}  // namespace unwinder
