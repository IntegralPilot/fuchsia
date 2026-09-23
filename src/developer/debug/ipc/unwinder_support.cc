// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/ipc/unwinder_support.h"

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/arch.h"
#include "src/developer/debug/shared/register_info.h"
#include "src/lib/unwinder/unwind.h"

namespace debug_ipc {

unwinder::Frame::Trust ConvertTrust(debug_ipc::StackFrame::Trust trust) {
  switch (trust) {
    case StackFrame::Trust::kScan:
      return unwinder::Frame::Trust::kScan;
    case StackFrame::Trust::kSCS:
      return unwinder::Frame::Trust::kSCS;
    case StackFrame::Trust::kSigReturn:
      return unwinder::Frame::Trust::kSigReturn;
    case StackFrame::Trust::kFP:
      return unwinder::Frame::Trust::kFP;
    case StackFrame::Trust::kPLT:
      return unwinder::Frame::Trust::kPLT;
    case StackFrame::Trust::kArmEhAbi:
      return unwinder::Frame::Trust::kArmEhAbi;
    case StackFrame::Trust::kCFI:
      return unwinder::Frame::Trust::kCFI;
    case StackFrame::Trust::kContext:
      return unwinder::Frame::Trust::kContext;
    default:
      return unwinder::Frame::Trust::kScan;  // Fallback
  }
}

debug_ipc::StackFrame::Trust ConvertTrust(unwinder::Frame::Trust unwinder_trust) {
  switch (unwinder_trust) {
    case unwinder::Frame::Trust::kScan:
      return StackFrame::Trust::kScan;
    case unwinder::Frame::Trust::kSCS:
      return StackFrame::Trust::kSCS;
    case unwinder::Frame::Trust::kSigReturn:
      return StackFrame::Trust::kSigReturn;
    case unwinder::Frame::Trust::kFP:
      return StackFrame::Trust::kFP;
    case unwinder::Frame::Trust::kPLT:
      return StackFrame::Trust::kPLT;
    case unwinder::Frame::Trust::kArmEhAbi:
      return StackFrame::Trust::kArmEhAbi;
    case unwinder::Frame::Trust::kCFI:
      return StackFrame::Trust::kCFI;
    case unwinder::Frame::Trust::kContext:
      return StackFrame::Trust::kContext;
    default:
      return StackFrame::Trust::kUnknown;
  }
}

unwinder::Registers ConvertRegisters(debug::Arch arch,
                                     const std::vector<debug::RegisterValue>& regs) {
  std::optional<unwinder::Registers> res = std::nullopt;

  switch (arch) {
    case debug::Arch::kX64:
      res = unwinder::Registers(unwinder::Registers::Arch::kX64);
      // The first 4 registers are out-of-order.
      res->Set(unwinder::RegisterID::kX64_rax, static_cast<uint64_t>((regs)[0].GetValue()));
      res->Set(unwinder::RegisterID::kX64_rbx, static_cast<uint64_t>((regs)[1].GetValue()));
      res->Set(unwinder::RegisterID::kX64_rcx, static_cast<uint64_t>((regs)[2].GetValue()));
      res->Set(unwinder::RegisterID::kX64_rdx, static_cast<uint64_t>((regs)[3].GetValue()));
      for (int i = 4; i < static_cast<int>(unwinder::RegisterID::kX64_last); i++) {
        res->Set(static_cast<unwinder::RegisterID>(i), static_cast<uint64_t>((regs)[i].GetValue()));
      }
      break;
    case debug::Arch::kArm64:
      res = unwinder::Registers(unwinder::Registers::Arch::kArm64);
      for (int i = 0; i < static_cast<int>(unwinder::RegisterID::kArm64_last); i++) {
        res->Set(static_cast<unwinder::RegisterID>(i), static_cast<uint64_t>((regs)[i].GetValue()));
      }
      break;
    case debug::Arch::kRiscv64:
      FX_NOTREACHED() << "Riscv64 is not supported yet";
      break;
    default:
      FX_NOTREACHED() << "Unknown platform";
      break;
  }

  return *res;
}

std::vector<debug_ipc::StackFrame> ConvertFrames(const std::vector<unwinder::Frame>& frames) {
  std::vector<debug_ipc::StackFrame> res;

  for (const unwinder::Frame& frame : frames) {
    std::vector<debug::RegisterValue> frame_regs;
    uint64_t ip = 0;
    uint64_t sp = 0;
    frame.regs.GetSP(sp);
    frame.regs.GetPC(ip);
    if (!res.empty()) {
      res.back().cfa = sp;
    }
    debug::Arch arch;
    switch (frame.regs.arch()) {
      case unwinder::Registers::Arch::kX64:
        arch = debug::Arch::kX64;
        break;
      case unwinder::Registers::Arch::kArm32:
      case unwinder::Registers::Arch::kArm64:
        arch = debug::Arch::kArm64;
        break;
      case unwinder::Registers::Arch::kRiscv64:
        arch = debug::Arch::kRiscv64;
        break;
    }
    frame_regs.reserve(frame.regs.size());
    for (auto& [reg_id, val] : frame.regs) {
      if (auto* info = debug::DWARFToRegisterInfo(arch, static_cast<uint32_t>(reg_id))) {
        frame_regs.emplace_back(info->id, val);
      }
    }

    auto pc_is_return_address = frame.pc_is_return_address ? StackFrame::AddressType::kReturn
                                                           : StackFrame::AddressType::kExact;
    res.emplace_back(ip, sp, 0, ConvertTrust(frame.trust), pc_is_return_address, frame_regs);
  }

  return res;
}

}  // namespace debug_ipc
