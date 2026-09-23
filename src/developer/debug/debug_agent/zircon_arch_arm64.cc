// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <zircon/hw/debug/arm64.h>
#include <zircon/status.h>
#include <zircon/syscalls/exception.h>

#include <optional>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/arch_types.h"
#include "src/developer/debug/debug_agent/debugged_thread.h"
#include "src/developer/debug/debug_agent/zircon_arch.h"
#include "src/developer/debug/ipc/decode_exception.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/register_info.h"
#include "src/developer/debug/shared/zx_status.h"
#include "src/lib/fxl/strings/string_printf.h"

// See arch_arm64.cc for general notes on the architecture.

namespace debug_agent {
namespace arch {

namespace {

using debug::RegisterID;

// Implements a case statement for calling WriteRegisterValue assuming the Zircon register
// field matches the enum name. This avoids implementation typos where the names don't match.
#define IMPLEMENT_CASE_WRITE_REGISTER_VALUE(name)  \
  case RegisterID::kARMv8_##name:                  \
    status = WriteRegisterValue(reg, &regs->name); \
    break;

zx_status_t ReadGeneralRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_general_regs_t gen_regs;
  zx_status_t status = thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &gen_regs, sizeof(gen_regs));
  if (status != ZX_OK)
    return status;

  arch::SaveGeneralRegs(gen_regs, out);
  return ZX_OK;
}

zx_status_t ReadVectorRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_vector_regs_t vec_regs;
  zx_status_t status = thread.read_state(ZX_THREAD_STATE_VECTOR_REGS, &vec_regs, sizeof(vec_regs));
  if (status != ZX_OK)
    return status;

  out.emplace_back(RegisterID::kARMv8_fpcr, vec_regs.fpcr);
  out.emplace_back(RegisterID::kARMv8_fpsr, vec_regs.fpsr);

  auto base = static_cast<uint32_t>(RegisterID::kARMv8_v0);
  for (size_t i = 0; i < 32; i++)
    out.emplace_back(static_cast<RegisterID>(base + i), 16u, &vec_regs.v[i]);

  return ZX_OK;
}

zx_status_t ReadDebugRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_debug_regs_t debug_regs;
  zx_status_t status =
      thread.read_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs, sizeof(debug_regs));
  if (status != ZX_OK)
    return status;

  if (debug_regs.hw_bps_count >= AARCH64_MAX_HW_BREAKPOINTS) {
    LOGS(Error) << "Received too many HW breakpoints: " << debug_regs.hw_bps_count
                << " (max: " << AARCH64_MAX_HW_BREAKPOINTS << ").";
    return ZX_ERR_INVALID_ARGS;
  }

  // HW breakpoints.
  {
    auto bcr_base = static_cast<uint32_t>(RegisterID::kARMv8_dbgbcr0_el1);
    auto bvr_base = static_cast<uint32_t>(RegisterID::kARMv8_dbgbvr0_el1);
    for (size_t i = 0; i < debug_regs.hw_bps_count; i++) {
      out.emplace_back(static_cast<RegisterID>(bcr_base + i), debug_regs.hw_bps[i].dbgbcr);
      out.emplace_back(static_cast<RegisterID>(bvr_base + i), debug_regs.hw_bps[i].dbgbvr);
    }
  }

  // Watchpoints.
  {
    auto bcr_base = static_cast<uint32_t>(RegisterID::kARMv8_dbgwcr0_el1);
    auto bvr_base = static_cast<uint32_t>(RegisterID::kARMv8_dbgwvr0_el1);
    for (size_t i = 0; i < debug_regs.hw_bps_count; i++) {
      out.emplace_back(static_cast<RegisterID>(bcr_base + i), debug_regs.hw_wps[i].dbgwcr);
      out.emplace_back(static_cast<RegisterID>(bvr_base + i), debug_regs.hw_wps[i].dbgwvr);
    }
  }

  // TODO(donosoc): Currently this registers that are platform information are
  //                being hacked out as HW breakpoint values in order to know
  //                what the actual settings are.
  //                This should be changed to get the actual values instead, but
  //                check in for now in order to continue.
  out.emplace_back(RegisterID::kARMv8_id_aa64dfr0_el1,
                   debug_regs.hw_bps[AARCH64_MAX_HW_BREAKPOINTS - 1].dbgbvr);
  out.emplace_back(RegisterID::kARMv8_mdscr_el1,
                   debug_regs.hw_bps[AARCH64_MAX_HW_BREAKPOINTS - 2].dbgbvr);

  return ZX_OK;
}

}  // namespace

void SaveGeneralRegs(const zx_thread_state_general_regs_t& input,
                     std::vector<debug::RegisterValue>& out) {
  // Add the X0-X29 registers.
  uint32_t base = static_cast<uint32_t>(RegisterID::kARMv8_x0);
  for (int i = 0; i < 30; i++)
    out.emplace_back(static_cast<RegisterID>(base + i), input.r[i]);

  // Add the named ones.
  out.emplace_back(RegisterID::kARMv8_lr, input.lr);
  out.emplace_back(RegisterID::kARMv8_sp, input.sp);
  out.emplace_back(RegisterID::kARMv8_pc, input.pc);
  out.emplace_back(RegisterID::kARMv8_cpsr, input.cpsr);
  out.emplace_back(RegisterID::kARMv8_tpidr, input.tpidr);
}

zx_status_t ReadRegisters(const zx::thread& thread, const debug::RegisterCategory& cat,
                          std::vector<debug::RegisterValue>& out) {
  switch (cat) {
    case debug::RegisterCategory::kGeneral:
      return ReadGeneralRegs(thread, out);
    case debug::RegisterCategory::kFloatingPoint:
      // No FP registers
      return ZX_OK;
    case debug::RegisterCategory::kVector:
      return ReadVectorRegs(thread, out);
    case debug::RegisterCategory::kDebug:
      return ReadDebugRegs(thread, out);
    default:
      LOGS(Error) << "Invalid category: " << static_cast<uint32_t>(cat);
      return ZX_ERR_INVALID_ARGS;
  }
}

zx_status_t WriteRegisters(zx::thread& thread, const debug::RegisterCategory& category,
                           const std::vector<debug::RegisterValue>& registers) {
  switch (category) {
    case debug::RegisterCategory::kGeneral: {
      zx_thread_state_general_regs_t regs;
      zx_status_t res = thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs));
      if (res != ZX_OK)
        return res;

      // Overwrite the values.
      res = WriteGeneralRegisters(registers, &regs);
      if (res != ZX_OK)
        return res;

      return thread.write_state(ZX_THREAD_STATE_GENERAL_REGS, &regs, sizeof(regs));
    }
    case debug::RegisterCategory::kFloatingPoint: {
      return ZX_ERR_INVALID_ARGS;  // No floating point registers.
    }
    case debug::RegisterCategory::kVector: {
      zx_thread_state_vector_regs_t regs;
      zx_status_t res = thread.read_state(ZX_THREAD_STATE_VECTOR_REGS, &regs, sizeof(regs));
      if (res != ZX_OK)
        return res;

      // Overwrite the values.
      res = WriteVectorRegisters(registers, &regs);
      if (res != ZX_OK)
        return res;

      return thread.write_state(ZX_THREAD_STATE_VECTOR_REGS, &regs, sizeof(regs));
    }
    case debug::RegisterCategory::kDebug: {
      zx_thread_state_debug_regs_t regs;
      zx_status_t res = thread.read_state(ZX_THREAD_STATE_DEBUG_REGS, &regs, sizeof(regs));
      if (res != ZX_OK)
        return res;

      res = WriteDebugRegisters(registers, &regs);
      if (res != ZX_OK)
        return res;

      return thread.write_state(ZX_THREAD_STATE_DEBUG_REGS, &regs, sizeof(regs));
    }
    case debug::RegisterCategory::kNone:
    case debug::RegisterCategory::kLast:
      break;
  }
  FX_NOTREACHED();
  return ZX_ERR_INVALID_ARGS;
}

zx_status_t WriteGeneralRegisters(const std::vector<debug::RegisterValue>& updates,
                                  zx_thread_state_general_regs_t* regs) {
  uint32_t begin_general = static_cast<uint32_t>(RegisterID::kARMv8_x0);
  uint32_t last_general = static_cast<uint32_t>(RegisterID::kARMv8_x29);

  for (const debug::RegisterValue& reg : updates) {
    zx_status_t status = ZX_OK;
    if (reg.data.size() != 8)
      return ZX_ERR_INVALID_ARGS;

    uint32_t id = static_cast<uint32_t>(reg.id);
    if (id >= begin_general && id <= last_general) {
      // General register array.
      status = WriteRegisterValue(reg, &regs->r[id - begin_general]);
    } else {
      switch (reg.id) {
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(lr);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(sp);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(pc);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(cpsr);
        default:
          status = ZX_ERR_INVALID_ARGS;
          break;
      }
    }

    if (status != ZX_OK)
      return status;
  }

  return ZX_OK;
}

zx_status_t WriteVectorRegisters(const std::vector<debug::RegisterValue>& updates,
                                 zx_thread_state_vector_regs_t* regs) {
  uint32_t begin_vector = static_cast<uint32_t>(RegisterID::kARMv8_v0);
  uint32_t last_vector = static_cast<uint32_t>(RegisterID::kARMv8_v31);

  for (const auto& reg : updates) {
    zx_status_t status = ZX_OK;
    uint32_t id = static_cast<uint32_t>(reg.id);

    if (id >= begin_vector && id <= last_vector) {
      status = WriteRegisterValue(reg, &regs->v[id - begin_vector]);
    } else {
      switch (reg.id) {
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fpcr);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fpsr);
        default:
          status = ZX_ERR_INVALID_ARGS;
          break;
      }
    }

    if (status != ZX_OK)
      return status;
  }
  return ZX_OK;
}

zx_status_t WriteDebugRegisters(const std::vector<debug::RegisterValue>& updates,
                                zx_thread_state_debug_regs_t* regs) {
  uint32_t begin_bcr = static_cast<uint32_t>(RegisterID::kARMv8_dbgbcr0_el1);
  uint32_t last_bcr = static_cast<uint32_t>(RegisterID::kARMv8_dbgbcr15_el1);

  uint32_t begin_bvr = static_cast<uint32_t>(RegisterID::kARMv8_dbgbvr0_el1);
  uint32_t last_bvr = static_cast<uint32_t>(RegisterID::kARMv8_dbgbvr15_el1);

  // TODO(bug 40992) Add ARM64 hardware watchpoint registers here.

  for (const auto& reg : updates) {
    zx_status_t status = ZX_OK;
    uint32_t id = static_cast<uint32_t>(reg.id);

    if (id >= begin_bcr && id <= last_bcr) {
      status = WriteRegisterValue(reg, &regs->hw_bps[id - begin_bcr].dbgbcr);
    } else if (id >= begin_bvr && id <= last_bvr) {
      status = WriteRegisterValue(reg, &regs->hw_bps[id - begin_bvr].dbgbvr);
    } else {
      status = ZX_ERR_INVALID_ARGS;
    }

    if (status != ZX_OK)
      return status;
  }
  return ZX_OK;
}

debug_ipc::ExceptionType DecodeExceptionType(const zx::thread& thread, uint32_t exception_type) {
  return debug_ipc::DecodeArm64Exception(exception_type, [&thread]() -> std::optional<uint32_t> {
    zx_thread_state_debug_regs_t debug_regs;
    zx_status_t status = thread.read_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs,
                                           sizeof(zx_thread_state_debug_regs_t));
    if (status != ZX_OK) {
      DEBUG_LOG(ArchArm64) << "Could not get ESR: " << zx_status_get_string(status);
      return std::nullopt;
    }

    return debug_regs.esr;
  });
}

debug_ipc::ExceptionRecord FillExceptionRecord(const zx_exception_report_t& in) {
  debug_ipc::ExceptionRecord record;

  record.valid = true;
  record.arch.arm64.esr = in.context.arch.u.arm_64.esr;
  record.arch.arm64.far = in.context.arch.u.arm_64.far;

  return record;
}

}  // namespace arch
}  // namespace debug_agent
