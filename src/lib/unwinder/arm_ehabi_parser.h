// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_ARM_EHABI_PARSER_H_
#define SRC_LIB_UNWINDER_ARM_EHABI_PARSER_H_

#include <span>

#include "gtest/gtest_prod.h"
#include "src/lib/unwinder/arm_ehabi_module.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

namespace unwinder {

class ArmEhAbiParser {
 public:
  ArmEhAbiParser(Memory* elf, const ArmEhAbiModule::IdxHeader& entry);

  [[nodiscard]] Error Step(Memory* stack, const Registers& current, Registers& next);
  void AsyncStep(AsyncMemory* stack, const Registers& current,
                 fit::callback<void(Error, Registers)> cb);

 private:
  FRIEND_TEST(ArmEhAbiParser, CollectInstructionsIndexInline);
  FRIEND_TEST(ArmEhAbiParser, CollectInstructionsTableLookup);

  enum class FrameHandlerType : uint8_t {
    // All instructions are within a single 32 bit word.
    kSu16 = 0x00,
    // The next byte is a number of 32 bit words to parse from the extab. The difference between
    // the 16 and 32 variants are to differentiate between types of "descriptors", which "define
    // regions of interest within a function". These are relevant for exception handling
    // specifically but not for unwinding. Both variants are handled identically for our purposes.
    kLu16 = 0x01,
    // The Lu32 descriptor is encoded as 3, despite the specification claiming that it should be 2.
    // https://github.com/llvm/llvm-project/blob/9542d0a0c661be92db950514b5dc9c5ea6d953af/libunwind/src/Unwind-EHABI.cpp#L58
    kLu32 = 0x03,
  };

  Error PrepareToStep(const Registers& current, Registers& next) const;
  Error FinalizeStep(Registers& next) const;

  // Given a mask of registers where the bit index corresponds to the register number, pop from the
  // stack (from low register -> high register), and store them in |next|.
  Error SetRegistersFromMask(Memory* stack, uint32_t register_mask, Registers& next);

  // Builds a list of opcodes and data to execute sequentially for unwinding, including any opcodes
  // that may be embedded in the first data word. The returned data stream does not include any
  // terminating bytes and there is no checking of validity for any opcodes. An error is returned if
  // no terminating byte is found for entries in the ARM.extab section. Inlined opcodes do not
  // require a terminating byte if they fit precisely in the 4 byte data field, so no error is
  // returned in that case.
  [[nodiscard]] fit::result<Error, std::vector<uint8_t>> CollectInstructions();

  // Parses and returns a vector of bytes in the correct order, starting with |data| at |offset|. If
  // |num_extra_words| is greater than 0, additional data will be read from |stack| at
  // |extab_offset_| until the unwinding terminator instruction is reached. The terminator will not
  // be included in the returned data vector. An error is returned if there was no terminator
  // instruction found within |num_extra_words|.
  fit::result<Error, std::vector<uint8_t>> ParseToFinished(uint32_t data, size_t offset,
                                                           uint8_t num_extra_words);

  // Converts a single 32 bit word to a vector of bytes in most significant to least significant
  // byte order. That is, bits 31-24 from |data| will be found at index 0, bits 23-16 at index 1,
  // etc. Parsing stops when either the end of the word has been parsed or a terminator instruction
  // has been found. The terminator is not included in the returned byte vector.
  struct ParsedResult {
    std::vector<uint8_t> data;
    bool found_terminator_opcode;
  };
  ParsedResult ParseWordFromOffset(uint32_t data, size_t offset);

  // Returns the number of extra words from the given offset in |data|, advancing |offset|.
  fit::result<Error, uint8_t> GetExtraWordsCountAndAdvance(uint32_t data, size_t& offset);

  // This method is an implementation of the ARM EHABI standard opcodes, found in this doc:
  // https://github.com/ARM-software/abi-aa/blob/c51addc3dc03e73a016a1e4edf25440bcac76431/ehabi32/ehabi32.rst#103frame-unwinding-instructions.
  //
  // The view of bytes is collected via |CollectInstructions|, and is constructed in such a way that
  // this method simply has to iterate through the entire view to examine each opcode and any
  // additional bytes required for that opcode in a simple to understand order.
  Error ExecuteInstructions(Memory* stack, std::span<const uint8_t> bytes, Registers& next);

  // Returns the first word of data, which depends on the type of index entry we got. If the data
  // was inlined, then |data_| contains the entire set of unwinding instructions. If the data is in
  // the .ARM.extab section, then we'll use the offset in |extab_offset_| to read the first word of
  // data from the table entry, which will tell us how many more words will follow in the
  // instruction sequence.
  fit::result<Error, uint32_t> GetFirstDataWord();

  uint32_t extab_offset_ = 0;
  uint32_t data_ = 0;

  Memory* elf_;
};

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_ARM_EHABI_PARSER_H_
