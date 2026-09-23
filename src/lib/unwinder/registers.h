// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_UNWINDER_REGISTERS_H_
#define SRC_LIB_UNWINDER_REGISTERS_H_

#include <lib/fit/result.h>

#include <cstdint>
#include <map>
#include <ranges>

#include "src/lib/unwinder/error.h"

namespace unwinder {

// The DWARF ID for each register. It's NOT exhaustive and |Registers| class may store some register
// ids not listed here.
enum class RegisterID : uint8_t {
  // x86_64. https://refspecs.linuxbase.org/elf/x86_64-abi-0.99.pdf Page 57
  // NOTE: the order is not RAX, RBX, RCX, RDX as in zx_x86_64_thread_state_general_regs_t.
  kX64_rax = 0,
  kX64_rdx = 1,
  kX64_rcx = 2,
  kX64_rbx = 3,
  kX64_rsi = 4,
  kX64_rdi = 5,
  kX64_rbp = 6,
  kX64_rsp = 7,
  kX64_r8 = 8,
  kX64_r9 = 9,
  kX64_r10 = 10,
  kX64_r11 = 11,
  kX64_r12 = 12,
  kX64_r13 = 13,
  kX64_r14 = 14,
  kX64_r15 = 15,
  // NOTE: x64 ABI assigns 16 as "Return Address", which is not an actual register, and doesn't
  // assign any id for RIP. A common practice (in libunwind and llvm-dwarfdump) is to use 16 to
  // represent RIP.
  kX64_rip = 16,
  kX64_last,

  // arm64
  // https://github.com/ARM-software/abi-aa/blob/main/aadwarf64/aadwarf64.rst#41dwarf-register-names
  kArm64_x0 = 0,
  kArm64_x1 = 1,
  kArm64_x2 = 2,
  kArm64_x3 = 3,
  kArm64_x4 = 4,
  kArm64_x5 = 5,
  kArm64_x6 = 6,
  kArm64_x7 = 7,
  kArm64_x8 = 8,
  kArm64_x9 = 9,
  kArm64_x10 = 10,
  kArm64_x11 = 11,
  kArm64_x12 = 12,
  kArm64_x13 = 13,
  kArm64_x14 = 14,
  kArm64_x15 = 15,
  kArm64_x16 = 16,
  kArm64_x17 = 17,
  kArm64_x18 = 18,
  kArm64_x19 = 19,
  kArm64_x20 = 20,
  kArm64_x21 = 21,
  kArm64_x22 = 22,
  kArm64_x23 = 23,
  kArm64_x24 = 24,
  kArm64_x25 = 25,
  kArm64_x26 = 26,
  kArm64_x27 = 27,
  kArm64_x28 = 28,
  kArm64_x29 = 29,
  kArm64_x30 = 30,
  kArm64_sp = 31,
  kArm64_pc = 32,
  kArm64_last,
  // Alias.
  kArm64_lr = kArm64_x30,

  // Aliases for ARM32.
  kArm32_fp = kArm64_x11,
  kArm32_sp = kArm64_x13,
  kArm32_lr = kArm64_x14,
  kArm32_pc = kArm64_x15,
  kArm32_last = kArm64_x16,

  // riscv64. https://github.com/riscv-non-isa/riscv-elf-psabi-doc/blob/master/riscv-dwarf.adoc
  // The name is chosen to keep consistency with zx_riscv64_thread_state_general_regs_t.
  kRiscv64_zero = 0,
  kRiscv64_ra = 1,
  kRiscv64_sp = 2,
  kRiscv64_gp = 3,
  kRiscv64_tp = 4,
  kRiscv64_t0 = 5,
  kRiscv64_t1 = 6,
  kRiscv64_t2 = 7,
  kRiscv64_s0 = 8,
  kRiscv64_s1 = 9,
  kRiscv64_a0 = 10,
  kRiscv64_a1 = 11,
  kRiscv64_a2 = 12,
  kRiscv64_a3 = 13,
  kRiscv64_a4 = 14,
  kRiscv64_a5 = 15,
  kRiscv64_a6 = 16,
  kRiscv64_a7 = 17,
  kRiscv64_s2 = 18,
  kRiscv64_s3 = 19,
  kRiscv64_s4 = 20,
  kRiscv64_s5 = 21,
  kRiscv64_s6 = 22,
  kRiscv64_s7 = 23,
  kRiscv64_s8 = 24,
  kRiscv64_s9 = 25,
  kRiscv64_s10 = 26,
  kRiscv64_s11 = 27,
  kRiscv64_t3 = 28,
  kRiscv64_t4 = 29,
  kRiscv64_t5 = 30,
  kRiscv64_t6 = 31,
  kRiscv64_last,

  // RISC-V DWARF Specification doesn't allocate a DWARF number for PC but do have a virtual
  // register "Alternate Frame Return Column". We use this for PC so that
  //  1) It matches the behavior on x64, where we use Return Address to represent RIP.
  //  2) The same logic to unwind from an async frame can be reused.
  kRiscv64_pc = 64,

  kInvalid = static_cast<uint8_t>(-1),
};

// Holds the register values. It's possible to get and set a register id that is not listed above.
class Registers {
 public:
  enum class Arch {
    kX64,
    kArm32,
    kArm64,
    kRiscv64,
  };

  explicit Registers(Arch arch) : arch_(arch) {}

  // Converts the current register object to the equivalent 32 bit register set. This means that
  // all general purpose registers are copied wholesale (without checking if the value can actually
  // fit in a 32 bit register), and the specially named registers are copied using the getters and
  // setters below.
  fit::result<Error, Registers> To32Bit() const;
  Error AdjustPCForThumb();

  Arch arch() const { return arch_; }

  // Delegate size(), begin() and end() to regs_.
  auto size() const { return regs_.size(); }
  auto begin() const { return regs_.begin(); }
  auto end() const { return regs_.end(); }

  // Yields a range of std::pair<std::string, uint64_t>, with the rhs actually
  // being whatever value_projection returns when called on uint64_t.
  auto WithNames(auto value_projection) const {
    return std::views::transform(regs_, [this, proj = std::move(value_projection)](auto reg) {
      auto [id, value] = reg;
      return std::make_pair(GetRegName(id), proj(value));
    });
  }
  auto WithNames() const { return WithNames(std::identity{}); }

  Error Get(RegisterID reg_id, uint64_t& val) const;
  Error Set(RegisterID reg_id, uint64_t val);
  Error Unset(RegisterID reg_id);

  Error GetSP(uint64_t& sp) const;
  Error SetSP(uint64_t sp);
  Error GetPC(uint64_t& pc) const;
  Error SetPC(uint64_t pc);
  Error GetFP(uint64_t& fp) const;
  Error SetFP(uint64_t fp);
  Error GetSCS(uint64_t& scs) const;
  Error SetSCS(uint64_t scs);

  // These are not formally specified for all architectures, but we define aliases for them.
  // It's RIP on x64, LR on Arm64, R14 on Arm, and RA on Riscv64. See the note above for why this
  // doesn't necessarily always make sense for x64.
  Error GetReturnAddress(uint64_t& ra) const;
  Error SetReturnAddress(uint64_t ra);

  // Return a string describing the value of all registers. Should be useful in debugging.
  std::string Describe() const;

  void Clear() { regs_.clear(); }

 private:
  std::string GetRegName(RegisterID reg_id) const;

  Arch arch_;
  std::map<RegisterID, uint64_t> regs_;
};

std::string ArchToString(Registers::Arch arch);

}  // namespace unwinder

#endif  // SRC_LIB_UNWINDER_REGISTERS_H_
