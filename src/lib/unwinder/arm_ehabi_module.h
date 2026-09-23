// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_ARM_EHABI_MODULE_H_
#define SRC_LIB_UNWINDER_ARM_EHABI_MODULE_H_

#include "gtest/gtest_prod.h"
#include "src/lib/unwinder/loaded_elf_module.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

inline uint32_t SignExtendPrel31(uint32_t data) { return data | ((data & 0x40000000u) << 1); }

inline int32_t DecodePrel31(uint32_t ptr) { return static_cast<int32_t>(SignExtendPrel31(ptr)); }

class ArmEhAbiModule {
 public:
  // Constructs, validates, and loads a new |ArmEhAbiModule| from the given |loaded_elf_module|.
  // Returns any errors that occur in any of the above steps, and releases any allocated memory.
  // The object is guaranteed to be valid if this function returns fit::ok().
  static fit::result<Error, std::unique_ptr<ArmEhAbiModule>> FromLoadedElfModule(
      const LoadedElfModule& loaded_elf_module);

  [[nodiscard]] Error Step(Memory* stack, const Registers& current, Registers& next) const;

  void AsyncStep(AsyncMemory* stack, const Registers& current,
                 fit::callback<void(Error, Registers)>) const;

  struct IdxHeaderData {
    uint32_t fn_addr = 0;
    // Either the encoded handling table entry if the high bit is 1, otherwise a prel31 encoded
    // offset from the start of the table to the handling table entry in ARM.extab.
    uint32_t data = 0;
  };

  struct IdxHeader {
    IdxHeaderData header;

    enum class Type {
      // |header.data| is an offset into .ARM.extab for the unwinding instructions.
      kCompact,
      // |header.data| is an inlined compact model containing the unwinding instructions directly.
      kCompactInline,
      // The encoding instructions are inlined into |header.data|.
      kUnknown,
    } type = Type::kUnknown;
  };

 private:
  // Construct via |FromLoadedElfModule|.
  explicit ArmEhAbiModule(const LoadedElfModule& loaded_elf_module, Memory* memory,
                          uint32_t load_address)
      : loaded_elf_module_(loaded_elf_module), elf_(memory), elf_ptr_(load_address) {}

  FRIEND_TEST(ArmEhAbiModule, Search);
  FRIEND_TEST(ArmEhAbiParser, CollectInstructionsTableLookup);

  // Load the .ARM.exidx binary search table.
  fit::result<Error> Load();

  // Performs an upper bounds search for PC in the exidx table.
  Error Search(uint32_t pc, IdxHeader& entry) const;

  fit::result<Error, IdxHeader> PrepareToStep(const Registers& current) const;

  const LoadedElfModule& loaded_elf_module_;
  Memory* const elf_ = nullptr;
  const uint32_t elf_ptr_ = 0;

  // This is the start of the binary search lookup table. Each table entry is two 32 bit integers.
  uint32_t arm_exidx_start_ = 0;
  uint32_t arm_exidx_end_ = 0;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_ARM_EHABI_MODULE_H_
