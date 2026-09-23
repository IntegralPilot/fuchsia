// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This file exists merely to offload logics from cfi_module.h to avoid the file being too large.

#ifndef SRC_LIB_UNWINDER_CFI_PARSER_H_
#define SRC_LIB_UNWINDER_CFI_PARSER_H_

#include <cstdint>
#include <vector>

#include "src/lib/unwinder/dwarf_expr.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/module.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

// Parse the call frame instructions to get the locations of CFA and registers.
class CfiParser {
 public:
  // arch is needed to default initialize register_locations_.
  CfiParser(Registers::Arch arch, Module::AddressSize size, uint64_t code_alignment_factor,
            int64_t data_alignment_factor);

  // Parse the CFA instructions until the (relative) pc reaches pc_limit.
  [[nodiscard]] Error ParseInstructions(Memory* elf, uint64_t instructions_begin,
                                        uint64_t instructions_end, uint64_t pc_limit);

  // Helper for DW_CFA_restore. This function should be called after CIE instructions are parsed but
  // before the FDE instructions are parsed.
  void Snapshot() { initial_register_locations_ = register_locations_; }

  // Apply the frame info to unwind one frame. |maybe_cfa| can be supplied if |PrepareToStep| has
  // been called before this function, which is typically the case of the |AsyncStep| function
  // before fetching necessary memory ahead of calling this synchronous method.
  [[nodiscard]] Error Step(Memory* stack, RegisterID return_address_register,
                           const Registers& current, Registers& next,
                           std::optional<uint64_t> maybe_cfa = std::nullopt);

  void AsyncStep(AsyncMemory* stack, RegisterID return_address_register, const Registers& current,
                 fit::callback<void(Error, Registers)> cb);

 private:
  struct RegisterLocation {
    enum class Type {
      kUndefined,   // the previous value is scratched, i.e. DW_CFA_undefined.
      kSameValue,   // the previous value is preserved, i.e. DW_CFA_same_value.
      kRegister,    // the previous value is stored in another register, i.e. DW_CFA_register.
      kOffset,      // the previous value is saved at CFA+offset, i.e. DW_CFA_offset.
      kValOffset,   // the previous value is just CFA+offset, i.e., DW_CFA_val_offset.
      kExpression,  // the previous value is saved at an address calculated by a DWARF expression.
      kValExpression,  // the previous value can be calculated by a DWARF expression.
    } type = Type::kUndefined;

    // The ID of the other register. Only valid when type is kRegister.
    RegisterID reg_id;

    // Only valid when type is kOffset or kValOffset.
    int64_t offset;

    // Only valid when type is kExpression or kValExpression.
    DwarfExpr expression;
  };

  const uint64_t code_alignment_factor_;
  const int64_t data_alignment_factor_;

  struct CfaLocation {
    enum class Type {
      kUndefined,   // CFA is not set yet.
      kOffset,      // CFA is reg+offset.
      kExpression,  // CFA can be calculated by a DWARF expression.
    } type = Type::kUndefined;

    // Only valid when type is kOffset.
    RegisterID reg = RegisterID::kInvalid;
    int64_t offset;

    // Only valid when type is kValExpression.
    DwarfExpr expression;
  } cfa_location_;

  // Compute the CFA from |cfa_location_|, which should already be populated via |ParseInstructions|
  // before calling this method.
  [[nodiscard]] Error PrepareToStep(Memory* stack, const Registers& current, uint64_t& cfa);

  Module::AddressSize address_size_;

  using RegisterLocations = std::map<RegisterID, RegisterLocation>;
  RegisterLocations register_locations_;

  // Copy of register_locations_ for DW_CFA_restore.
  RegisterLocations initial_register_locations_;

  // Stack of states for DW_CFA_remember_state and DW_CFA_restore_state.
  // CFA is also included, see https://dwarfstd.org/issues/230103.1.html.
  std::vector<std::pair<CfaLocation, RegisterLocations>> state_stack_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_CFI_PARSER_H_
