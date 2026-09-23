// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/arm_ehabi_parser.h"

#include <elf.h>
#include <inttypes.h>

#include <span>

#include "src/lib/unwinder/arm_ehabi_module.h"
#include "src/lib/unwinder/error.h"
#include "src/lib/unwinder/memory.h"
#include "src/lib/unwinder/registers.h"

#define LOG_DEBUG(...)
// #define LOG_DEBUG(...) fprintf(stderr, __VA_ARGS__);

namespace unwinder {
namespace {
constexpr uint32_t kExIdxCantUnwind = 1;
constexpr uint32_t kFinishedIndicator = 0xb0;

fit::result<Error, uint64_t> ParseULEB128FromBytes(std::span<const uint8_t> bytes) {
  uint64_t res = 0;
  uint64_t shift = 0;
  uint8_t byte = 0;
  size_t i = 0;

  do {
    if (i == bytes.size()) {
      return fit::error(Error("Failed to parse ULEB128."));
    }
    byte = bytes[i++];
    res |= (byte & 0x7F) << shift;
    shift += 7;
  } while (byte & 0x80);

  return fit::ok(res);
}

uint8_t GetNextByteFromData(uint32_t data, size_t offset) {
  const uint8_t* byte_data = reinterpret_cast<uint8_t*>(&data);

  // This reverses the byte order from a little endian representation to be more like big endian
  // to make decoding simpler. The encoding scheme typically will have us walk from most significant
  // to least significant bytes within a 16 or 32 bit range and interpret different bit pairings
  // within there so we need to be careful to take things in the correct order. This works for
  // arbitrary offsets broken on a 4 byte boundary. For example, for offsets 0, 1, 2, 3, 4, ... the
  // returned bytes will be at offsets 3, 2, 1, 0, 7, ... and so on. Since this unwinder is
  // out-of-process, this function won't work for data beyond the particular 32 bits in |data|, the
  // rest of the data must be fetched from the remote process.
  //
  // An example: if you have an encoding for popping a register off the stack (see EHABI 10.3)
  // the binary encoding looks like this:
  //
  //   0b1000iiii iiiiiiii
  //         ^    ^
  //         |    |
  //         r15
  //          r14
  //           r13
  //            r12
  //              |
  //              r11
  //             ..
  //                    r5
  //                     r4
  //
  // On a little endian system this will look like this in memory:
  //
  //   0biiiiiiii iiii0010
  //     ^        ^
  //     |        |
  //     |        r13
  //     |         r12
  //     |          r15
  //     |           r14
  //     r5
  //      r4
  //       r7
  //        r6
  //
  // So when you ask for offset 0 from this 16 bit integer, you'll get back
  //
  //   0b1000iiii
  //         ^
  //         |
  //         r15
  //          r14
  //           r13
  //            r12
  //
  // And offset 1 will be
  //
  //   0biiiiiiii
  //     ^
  //     |
  //     r11
  //      r10
  //        ..
  //           r5
  //            r4
  return byte_data[(offset & ~static_cast<size_t>(0x03)) +
                   (3 - (offset & static_cast<size_t>(0x03)))];
}

}  // namespace

ArmEhAbiParser::ArmEhAbiParser(Memory* elf, const ArmEhAbiModule::IdxHeader& entry) : elf_(elf) {
  switch (entry.type) {
    case ArmEhAbiModule::IdxHeader::Type::kCompact:
      extab_offset_ = static_cast<int32_t>(entry.header.data);
      break;
    case ArmEhAbiModule::IdxHeader::Type::kCompactInline:
      data_ = entry.header.data;
      break;
    default:
      break;
  }
}

Error ArmEhAbiParser::Step(Memory* stack, const Registers& current, Registers& next) {
  // Create a copy so that if we fail while copying |current| to |next| we don't lose the values
  // that may already be populated in |next|.
  Registers maybe_next = next;
  if (auto err = PrepareToStep(current, maybe_next); err.has_err()) {
    return err;
  }

  // Now we can update the out parameter.
  next = std::move(maybe_next);
  if (auto result = CollectInstructions(); result.is_ok()) {
    LOG_DEBUG("Executing %zu instructions\n", result.value().size());
    if (auto err = ExecuteInstructions(stack, result.value(), next); err.has_err()) {
      return err;
    }
  } else {
    return result.error_value();
  }

  auto err = FinalizeStep(next);
  LOG_DEBUG("%s => %s\n", current.Describe().c_str(), next.Describe().c_str());

  return err;
}

void ArmEhAbiParser::AsyncStep(AsyncMemory* stack, const Registers& current,
                               fit::callback<void(Error, Registers)> cb) {
  Registers next(Registers::Arch::kArm32);
  if (auto err = PrepareToStep(current, next); err.has_err()) {
    cb(err, std::move(next));
    return;
  }

  auto instructions = CollectInstructions();
  if (instructions.is_error()) {
    cb(instructions.error_value(), std::move(next));
    return;
  }

  // Now we need to collect some amount of stack memory. Each unwinding operation could pop several
  // registers from the stack, but we don't know exactly how many yet, so we overestimate and try to
  // read once.
  uint64_t sp;
  if (auto err = current.GetSP(sp); err.has_err()) {
    cb(err, std::move(next));
    return;
  }

  // It is difficult to predict exactly how much data to read here. Because some unwinding operation
  // could reassign to to SP in the middle of the unwinding instructions, we could have a completely
  // different SP than when we started. This also means that we cannot exactly predict where we will
  // be popping registers from. To compensate for this, we read quite a bit more than we probably
  // need to to ensure that we have a wide enough range to synchronously execute all of our
  // instructions.
  constexpr uint32_t kDefaultReadSize = 4096;
  stack->FetchMemoryRanges({{sp, kDefaultReadSize}}, [=, this, next = std::move(next),
                                                      instructions = std::move(*instructions),
                                                      cb = std::move(cb)]() mutable {
    if (auto err = ExecuteInstructions(stack, instructions, next); err.has_err()) {
      cb(err, std::move(next));
      return;
    }

    auto err = FinalizeStep(next);
    LOG_DEBUG("%s => %s\n", current.Describe().c_str(), next.Describe().c_str());
    cb(err, std::move(next));
  });
}

Error ArmEhAbiParser::PrepareToStep(const Registers& current, Registers& next) const {
  if (data_ == 0 && extab_offset_ == 0) {
    return Error("Invalid IdxHeaderEntry.");
  } else if (data_ == kExIdxCantUnwind || extab_offset_ == kExIdxCantUnwind) {
    return Error("Got ExIdxCantUnwind.");
  }

  // The ARM EH ABI specifies that we're working with a singular "virtual register set" which is
  // always updated in place. We don't have that concept here, so every step operation has to start
  // with updating all of the registers in |next| to |current| with the exception of PC, which will
  // be set explicitly by one of the unwinding instructions or to LR (r14) if not set explicitly by
  // the end of the instructions.
  for (size_t i = 0; i < static_cast<size_t>(RegisterID::kArm32_last); i++) {
    uint64_t v;
    // Some registers in |next| may have been set from CFI instructions, which we don't want to
    // overwrite.
    if (i != static_cast<size_t>(RegisterID::kArm32_pc) &&
        next.Get(static_cast<RegisterID>(i), v).has_err()) {
      // Not an error if something isn't set here.
      if (current.Get(static_cast<RegisterID>(i), v).ok()) {
        next.Set(static_cast<RegisterID>(i), v);
      }
    }
  }

  return Success();
}

Error ArmEhAbiParser::FinalizeStep(Registers& next) const {
  Error err = Success();
  // Finally, restore PC to LR if not directly set by the unwinding instructions.
  if (uint64_t pc; next.GetPC(pc).has_err()) {
    // Undefined LR register usually means the end of unwinding. This is not considered an error.
    if (uint64_t return_address; next.GetReturnAddress(return_address).ok()) {
      if (err = next.SetPC(return_address); err.ok()) {
        next.Unset(RegisterID::kArm32_lr);
        // Don't overwrite the SetPC error if something went wrong.
        err = next.AdjustPCForThumb();
      }
    } else {
      err = Error("Could not set PC from LR!");
    }
  }

  return err;
}

fit::result<Error, std::vector<uint8_t>> ArmEhAbiParser::CollectInstructions() {
  uint32_t data = 0;
  if (auto result = GetFirstDataWord(); result.is_ok()) {
    data = result.value();
  } else {
    return result;
  }

  size_t offset = 0;
  uint8_t num_extra_words = 0;
  if (auto result = GetExtraWordsCountAndAdvance(data, offset); result.is_ok()) {
    num_extra_words = result.value();
  } else {
    return result;
  }

  return ParseToFinished(data, offset, num_extra_words);
}

fit::result<Error, uint8_t> ArmEhAbiParser::GetExtraWordsCountAndAdvance(uint32_t data,
                                                                         size_t& offset) {
  uint8_t byte = GetNextByteFromData(data, offset++);
  FrameHandlerType type = static_cast<FrameHandlerType>(byte & 0x0f);

  // The number of 32 bit words _after_ |data| that we need to read for all of the unwinding
  // instructions.
  uint8_t num_extra_words;
  switch (type) {
    case FrameHandlerType::kSu16:
      num_extra_words = 0;  // All instructions present in |data|.
      break;
    case FrameHandlerType::kLu16:
    case FrameHandlerType::kLu32:
      num_extra_words = GetNextByteFromData(data, offset++);
      break;
    default:
      return fit::error(Error("Unknown FrameHandlerType: %d\n", static_cast<uint8_t>(type)));
  }

  return fit::ok(num_extra_words);
}

// Will not read any more bytes other than what it is given. This won't return an error if it
// doesn't find |kFinishedIndicator|.
ArmEhAbiParser::ParsedResult ArmEhAbiParser::ParseWordFromOffset(uint32_t data, size_t offset) {
  uint8_t byte = GetNextByteFromData(data, offset++);
  std::vector<uint8_t> parsed_bytes;
  parsed_bytes.push_back(byte);

  while (offset < 4) {
    if (byte = GetNextByteFromData(data, offset++); byte == kFinishedIndicator)
      break;
    parsed_bytes.push_back(byte);
  }

  return {.data = parsed_bytes, .found_terminator_opcode = byte == kFinishedIndicator};
}

// Will continue to read bytes from the given offset until it finds a "finished" instruction, but
// will not decode any other instructions.
fit::result<Error, std::vector<uint8_t>> ArmEhAbiParser::ParseToFinished(uint32_t data,
                                                                         size_t offset,
                                                                         uint8_t num_extra_words) {
  auto res = ParseWordFromOffset(data, offset);

  if (num_extra_words == 0) {
    // The compact inline format will not include a finished instruction if it fills the allocated 4
    // bytes in the index entry exactly, so we cannot assert that the result from parsing the first
    // word has found the terminator opcode.
    return fit::ok(res.data);
  }

  std::vector<uint8_t> data_stream = std::move(res.data);

  // Results from parsing each word of data. These are outside of the loop so we can do checking at
  // the end to make sure that we actually encountered a terminator opcode and give a good error
  // message if we didn't.
  ParsedResult next_data_result;
  uint32_t next_data = 0;

  // Start at i = 1 because we already have the first word in |data|.
  for (size_t i = 1; i <= num_extra_words; i++) {
    if (auto err = elf_->Read(static_cast<uint64_t>(extab_offset_) + (i * 4), next_data);
        err.has_err()) {
      return fit::error(err);
    }
    next_data_result = ParseWordFromOffset(next_data, 0);
    data_stream.insert(data_stream.end(), next_data_result.data.begin(),
                       next_data_result.data.end());
  }

  if (!next_data_result.found_terminator_opcode) {
    return fit::error(
        Error("Failed to find finished indicator in final data word: 0x%" PRIx32, next_data));
  }

  return fit::ok(data_stream);
}

fit::result<Error, uint32_t> ArmEhAbiParser::GetFirstDataWord() {
  if (data_ > 0) {
    return fit::ok(data_);
  }

  // We have to read from |extab_offset_| to get the first data word.
  uint32_t data = 0;
  if (auto err = elf_->Read(static_cast<uint64_t>(extab_offset_), data); err.has_err()) {
    return fit::error(err);
  }

  return fit::ok(data);
}

Error ArmEhAbiParser::ExecuteInstructions(Memory* stack, std::span<const uint8_t> bytes,
                                          Registers& next) {
  for (size_t i = 0; i < bytes.size(); i++) {
    uint8_t byte = bytes[i];
    // Modifying SP with an immediate.
    if ((byte & 0x80) == 0) {
      uint64_t sp;
      if (auto err = next.GetSP(sp); err.has_err()) {
        return err;
      }

      // The addend is in the lower 6 bits of this byte.
      if (byte & 0x40) {
        // sp = sp - (XXXXXX << 2) - 4
        //    = sp - ((XXXXXX << 2) + 4)
        // Make sure to mask away the high bits so we don't accidentally subtract a negative.
        sp -= ((static_cast<uint32_t>(byte) & 0x3f) << 2) + 4;
      } else {
        // sp = sp + (XXXXXXX << 2) + 4
        sp += (static_cast<uint32_t>(byte) << 2) + 4;
      }

      LOG_DEBUG("SP_OFFSET [0x%" PRIx8 "]: new sp: 0x%" PRIx64 "\n", byte, sp);
      if (auto err = next.SetSP(sp); err.has_err()) {
        return err;
      }
    } else {
      // Have to decode the rest of the bits in the high nibble now that we know that MSB == 1.
      switch (byte & 0xf0) {
        case 0x80: {
          // Pop register values from stack.
          // Registers r4-r15 are packed into the low 4 bits of this byte and the entire next byte.
          // This mask decodes those low 12 bits and creates a mask where each register is in it's
          // respective bit position in the mask (i.e. r15 is in bit 15, r14 in bit 14, etc).
          uint32_t register_mask = (((static_cast<uint32_t>(byte & 0x0f)) << 12) |
                                    (static_cast<uint32_t>(bytes[++i])) << 4);

          LOG_DEBUG("SET_REGS_FROM_MASK [0x%" PRIx8 "]: mask: 0x%" PRIx32 "\n", byte,
                    register_mask);
          if (auto err = SetRegistersFromMask(stack, register_mask, next); err.has_err()) {
            return err;
          }
          break;
        }
        case 0x90: {
          // Set SP from a register.
          uint32_t reg = byte & 0x0f;
          if (reg == 13 || reg == 15) {
            return Error("Setting SP from register 13 or 15 is disallowed.");
          }

          uint64_t val;
          if (auto err = next.Get(static_cast<RegisterID>(reg), val); err.has_err()) {
            return err;
          }

          LOG_DEBUG("SP_REG [0x%" PRIx8 "]: from reg: %" PRId32 " val: 0x%" PRIx64 "\n", byte, reg,
                    val);

          if (auto err = next.SetSP(val); err.has_err()) {
            return err;
          }
          break;
        }
        case 0xa0: {
          // Pop registers from r4 thru r[4 + N] (inclusive).
          // N is encoded in the lower 3 bits. We'll create a register mask same as above, where
          // each bit corresponds the register index.
          uint32_t n = byte & 0x7;

          // Add 1 to |n| to get 1 past the last register we want to include. After shifting,
          // subtract 1 to set all the lower bits to 1 and shift up by 4 so the mask ends with r4.
          uint32_t register_mask = ((1u << (n + 1)) - 1) << 4;

          // If the high bit of the low nibble is set, we also pop r14.
          if (byte & 0x08) {
            register_mask |= 1 << 14;
          }

          LOG_DEBUG("SET_REGS_FROM_MASK [0x%" PRIx8 "]: mask: 0x%" PRIx32 "\n", byte,
                    register_mask);

          if (auto err = SetRegistersFromMask(stack, register_mask, next); err.has_err()) {
            return err;
          }

          break;
        }
        case 0xb0: {
          if (byte == kFinishedIndicator) {
            // If the lower 4 bits are all 0s then this is the finished indicator. This check is
            // outside of the switch below for clarity rather than handling the "0x0" case for the
            // lower bits.
            break;
          }

          // Need to do some decoding of the lower 4 bits.
          switch (byte & 0x0f) {
            case 0x01: {
              uint8_t next_byte = bytes[++i];
              if (next_byte == 0 || (next_byte & 0xf0) != 0) {
                // Spare. ARM Specification indicates that this is an unwinding failure.
                return Error("Encountered SPARE opcode, ending unwinding.");
              }

              // The mask is for r3-r0 (in MSB->LSB order) in the low nibble.
              uint32_t register_mask = next_byte & 0xf;
              LOG_DEBUG("SET_REGS_FROM_MASK [0x%" PRIx8 "]: maks: 0x%" PRIx32 "\n", byte,
                        register_mask);

              if (auto err = SetRegistersFromMask(stack, register_mask, next); err.has_err()) {
                return err;
              }

              break;
            }
            case 0x02: {
              // vsp = vsp + 0x204 + (uleb128 << 2)
              uint64_t uleb_value = 0;
              if (auto result = ParseULEB128FromBytes(bytes.subspan(++i)); result.is_ok()) {
                uleb_value = result.value();
              } else {
                return result.error_value();
              }

              uint64_t sp = 0;
              if (auto err = next.GetSP(sp); err.has_err()) {
                return err;
              }

              sp += 0x204 + (uleb_value << 2);

              LOG_DEBUG("SP_OFFSET [0x%" PRIx8 "]: modifying sp by offset: 0x%" PRIx64
                        " new sp: 0x%" PRIx64 "\n",
                        byte, 0x204 + (uleb_value << 2), sp);

              if (auto err = next.SetSP(sp); err.has_err()) {
                return err;
              }

              break;
            }
            case 0x3: {
              // This is for popping double-precision float registers. We ignore the values here for
              // now, but we need to update SP accordingly. The rule states that SP is incremented
              // by 8N + 4. where N is the distance between the high nibble and the low nibble,
              // which mark the beginning and end (inclusive) respectively.
              uint8_t next_byte = bytes[++i];
              uint8_t high = (next_byte & 0xf0) >> 4;
              uint8_t low = next_byte & 0x0f;
              // Add one since the range is inclusive.
              uint8_t count = low - high + 1;

              uint64_t sp = 0;
              if (auto err = next.GetSP(sp); err.has_err()) {
                return err;
              }

              sp += (8 * count) + 4;

              LOG_DEBUG("POP_VFA [0x%" PRIx8 "]: modifying sp by offset: 0x%" PRIx64
                        " new sp: 0x%" PRIx64 "\n",
                        byte, (8 * count) + 4, sp);

              if (auto err = next.SetSP(sp); err.has_err()) {
                return err;
              }

              break;
            }
          }
          break;
        }
        case 0xc0: {
          // This opcode is dealing with special double- and quad-word registers. We can ignore the
          // register values for now, but we have to make sure we modify SP accordingly.
          uint8_t next_byte = bytes[++i];
          // |count| does not include the 0th register included in the opcode, so we have to add one
          // to the count to get the true value of N.
          uint8_t count = (next_byte & 0xf) + 1;

          uint64_t sp;
          if (auto err = next.GetSP(sp); err.has_err()) {
            return err;
          }

          // This opcode is the "VPUSH" method, which says that SP should be incremented by 8 * N
          // where N is the count of registers, which is stored in the low nibble of |next_byte|.
          sp += count * 8;

          LOG_DEBUG("VPUSH [0x%" PRIx8 "]: modifying sp by offset: %" PRId32 " new sp: 0x%" PRIx64
                    "\n",
                    byte, count * 8, sp);

          if (auto err = next.SetSP(sp); err.has_err()) {
            return err;
          }

          break;
        }
        default: {
          return Error("OP 0x%" PRIx8 " not implemented yet", byte);
        }
      }
    }
  }

  return Success();
}

Error ArmEhAbiParser::SetRegistersFromMask(Memory* stack, uint32_t register_mask, Registers& next) {
  uint64_t sp;
  if (auto err = next.GetSP(sp); err.has_err()) {
    return err;
  }

  bool set_sp = false;
  for (size_t i = 0; i < static_cast<size_t>(RegisterID::kArm32_last); i++) {
    if (register_mask & (1 << i)) {
      uint32_t val;
      if (auto err = stack->ReadAndAdvance(sp, val); err.has_err()) {
        return err;
      }

      if (static_cast<RegisterID>(i) == RegisterID::kArm32_sp) {
        set_sp = true;
      }

      LOG_DEBUG("POP_REG: reg: %" PRId64 " new val: 0x%" PRIx32 "\n", i, val);
      next.Set(static_cast<RegisterID>(i), val);
    }
  }

  if (!set_sp) {
    // SP wasn't popped by the bitmask above, so we set it to the value after popping all registers
    // from the mask.
    next.SetSP(sp);
  }

  return Success();
}

}  // namespace unwinder
