// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/registers.h"

#include <cstdint>
#include <format>
#include <string>

namespace unwinder {

namespace {

RegisterID GetPcReg(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kX64:
      return RegisterID::kX64_rip;
    case Registers::Arch::kArm32:
      return RegisterID::kArm32_pc;
    case Registers::Arch::kArm64:
      return RegisterID::kArm64_pc;
    case Registers::Arch::kRiscv64:
      return RegisterID::kRiscv64_pc;
  }
}

RegisterID GetSpReg(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kX64:
      return RegisterID::kX64_rsp;
    case Registers::Arch::kArm32:
      return RegisterID::kArm32_sp;
    case Registers::Arch::kArm64:
      return RegisterID::kArm64_sp;
    case Registers::Arch::kRiscv64:
      return RegisterID::kRiscv64_sp;
  }
}

RegisterID GetFpReg(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kX64:
      return RegisterID::kX64_rbp;
    case Registers::Arch::kArm32:
      return RegisterID::kArm32_fp;
    case Registers::Arch::kArm64:
      return RegisterID::kArm64_x29;
    case Registers::Arch::kRiscv64:
      return RegisterID::kRiscv64_s0;
  }
}

RegisterID GetScsReg(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kX64:
      return RegisterID::kInvalid;
    case Registers::Arch::kArm32:
      return RegisterID::kInvalid;
    case Registers::Arch::kArm64:
      return RegisterID::kArm64_x18;
    case Registers::Arch::kRiscv64:
      return RegisterID::kRiscv64_gp;
  }
}

RegisterID GetReturnAddressRegister(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kX64:
      return RegisterID::kX64_rip;
    case Registers::Arch::kArm32:
      return RegisterID::kArm32_lr;
    case Registers::Arch::kArm64:
      return RegisterID::kArm64_lr;
    case Registers::Arch::kRiscv64:
      return RegisterID::kRiscv64_ra;
  }
}

}  // namespace

Error Registers::Get(RegisterID reg_id, uint64_t& val) const {
  auto it = regs_.find(reg_id);
  if (it == regs_.end()) {
    return Error("register %s is undefined", GetRegName(reg_id).c_str());
  }
  val = it->second;
  return Success();
}

Error Registers::Set(RegisterID reg_id, uint64_t val) {
  regs_[reg_id] = val;
  return Success();
}

Error Registers::Unset(RegisterID reg_id) {
  regs_.erase(reg_id);
  return Success();
}

Error Registers::GetSP(uint64_t& sp) const { return Get(GetSpReg(arch_), sp); }
Error Registers::SetSP(uint64_t sp) { return Set(GetSpReg(arch_), sp); }

Error Registers::GetPC(uint64_t& pc) const { return Get(GetPcReg(arch_), pc); }
Error Registers::SetPC(uint64_t pc) { return Set(GetPcReg(arch_), pc); }

Error Registers::GetFP(uint64_t& fp) const { return Get(GetFpReg(arch_), fp); }
Error Registers::SetFP(uint64_t fp) { return Set(GetFpReg(arch_), fp); }

Error Registers::GetSCS(uint64_t& scs) const { return Get(GetScsReg(arch_), scs); }
Error Registers::SetSCS(uint64_t scs) { return Set(GetScsReg(arch_), scs); }

Error Registers::GetReturnAddress(uint64_t& ra) const {
  return Get(GetReturnAddressRegister(arch_), ra);
}

Error Registers::SetReturnAddress(uint64_t ra) { return Set(GetReturnAddressRegister(arch_), ra); }

fit::result<Error, Registers> Registers::To32Bit() const {
  Registers result = Registers(Arch::kArm32);
  uint64_t r_val;

  for (RegisterID reg_id = RegisterID::kArm64_x0; reg_id < RegisterID::kArm32_last;
       reg_id = static_cast<RegisterID>(static_cast<uint8_t>(reg_id) + 1)) {
    Get(reg_id, r_val);
    result.Set(reg_id, r_val);
  }

  // We should always have all of SP, LR, and PC restored from Starnix's CFI directives.
  //
  // TODO(https://fxbug.dev/441470079): Have a better defined contract of what registers will be
  // where when we're transitioning to a 32 bit restricted mode frame.
  if (auto err = GetSP(r_val); err.ok()) {
    result.SetSP(r_val);
  } else {
    return fit::error(err);
  }

  if (auto err = GetReturnAddress(r_val); err.ok()) {
    result.SetReturnAddress(r_val);
  } else {
    return fit::error(err);
  }

  if (auto err = GetPC(r_val); err.ok()) {
    result.SetPC(r_val);
    // This function could be called from unwinders that don't know about or care to account for
    // THUMB so we do it here as well as in the 32 bit ARM EHABI unwinder. Calling this here will
    // only fail if PC isn't set or fails to be set for some reason.
    if (err = result.AdjustPCForThumb(); err.has_err()) {
      return fit::error(err);
    }
  } else {
    return fit::error(err);
  }

  return fit::ok(result);
}

Error Registers::AdjustPCForThumb() {
  if (arch() != Arch::kArm32) {
    return Error("Not kArm32 architecture.");
  }

  uint64_t pc;
  if (auto err = GetPC(pc); err.has_err()) {
    return err;
  }

  // Accounting for THUMB instructions is simply masking away the lowest bit of PC to get the
  // true address of the instruction.
  pc &= static_cast<uint64_t>(~1);

  if (auto err = SetPC(pc); err.has_err()) {
    return err;
  }

  return Success();
}

std::string Registers::Describe() const {
  constexpr auto format_reg = [](const auto& reg) {
    const auto& [name, value] = reg;
    return std::format("{}={:#x}", name, value);
  };
  return std::format("{:n:s}", std::views::transform(WithNames(), format_reg));
}

std::string Registers::GetRegName(RegisterID reg_id) const {
  static const char* x64_names[] = {
      "rax", "rdx", "rcx", "rbx", "rsi", "rdi", "rbp", "rsp", "r8",
      "r9",  "r10", "r11", "r12", "r13", "r14", "r15", "rip",
  };
  static const char* arm64_names[] = {
      "x0",  "x1",  "x2",  "x3",  "x4",  "x5",  "x6",  "x7",  "x8",  "x9",  "x10",
      "x11", "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21",
      "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "lr",  "sp",  "pc",
  };
  static const char* riscv64_names[] = {
      "zero", "ra", "sp", "gp", "tp",  "t0",  "t1", "t2", "s0", "s1", "a0",
      "a1",   "a2", "a3", "a4", "a5",  "a6",  "a7", "s2", "s3", "s4", "s5",
      "s6",   "s7", "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
  };

  const char** names;
  size_t length;
  switch (arch_) {
    case Arch::kX64:
      names = x64_names;
      length = sizeof(x64_names) / sizeof(char*);
      break;
    case Arch::kArm32:
    case Arch::kArm64:
      names = arm64_names;
      length = sizeof(arm64_names) / sizeof(char*);
      break;
    case Arch::kRiscv64:
      if (reg_id == RegisterID::kRiscv64_pc) {
        return "pc";
      }
      names = riscv64_names;
      length = sizeof(riscv64_names) / sizeof(char*);
      break;
  }

  if (static_cast<size_t>(reg_id) < length) {
    return names[static_cast<size_t>(reg_id)];
  }

  return std::to_string(static_cast<size_t>(reg_id));
}

std::string ArchToString(Registers::Arch arch) {
  switch (arch) {
    case Registers::Arch::kArm32:
      return "Arm32";
    case Registers::Arch::kArm64:
      return "Arm64";
    case Registers::Arch::kX64:
      return "X64";
    case Registers::Arch::kRiscv64:
      return "riscv64";
  };
}

}  // namespace unwinder
