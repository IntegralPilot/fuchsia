// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <zircon/status.h>
#include <zircon/syscalls/exception.h>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/debug_agent/debugged_thread.h"
#include "src/developer/debug/debug_agent/zircon_arch.h"
#include "src/developer/debug/ipc/decode_exception.h"
#include "src/developer/debug/shared/arch_x86.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/register_info.h"

// See arch_x64.cc for general notes on the architecture.

namespace debug_agent {
namespace arch {

namespace {

using debug::RegisterID;

// Implements a case statement for calling WriteRegisterValue assuming the Zircon register
// field matches the enum name. This avoids implementation typos where the names don't match.
#define IMPLEMENT_CASE_WRITE_REGISTER_VALUE(name)  \
  case RegisterID::kX64_##name:                    \
    status = WriteRegisterValue(reg, &regs->name); \
    break;

zx_status_t ReadGeneralRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_general_regs_t gen_regs;
  zx_status_t status = thread.read_state(ZX_THREAD_STATE_GENERAL_REGS, &gen_regs, sizeof(gen_regs));
  if (status != ZX_OK)
    return status;

  SaveGeneralRegs(gen_regs, out);
  return ZX_OK;
}

zx_status_t ReadFPRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_fp_regs_t fp_regs;
  zx_status_t status = thread.read_state(ZX_THREAD_STATE_FP_REGS, &fp_regs, sizeof(fp_regs));
  if (status != ZX_OK)
    return status;

  out.emplace_back(RegisterID::kX64_fcw, fp_regs.fcw);
  out.emplace_back(RegisterID::kX64_fsw, fp_regs.fsw);
  out.emplace_back(RegisterID::kX64_ftw, fp_regs.ftw);
  out.emplace_back(RegisterID::kX64_fop, fp_regs.fop);
  out.emplace_back(RegisterID::kX64_fip, fp_regs.fip);
  out.emplace_back(RegisterID::kX64_fdp, fp_regs.fdp);

  // Each entry is 16 bytes long, but only 10 are actually used.
  out.emplace_back(RegisterID::kX64_st0, 16u, &fp_regs.st[0]);
  out.emplace_back(RegisterID::kX64_st1, 16u, &fp_regs.st[1]);
  out.emplace_back(RegisterID::kX64_st2, 16u, &fp_regs.st[2]);
  out.emplace_back(RegisterID::kX64_st3, 16u, &fp_regs.st[3]);
  out.emplace_back(RegisterID::kX64_st4, 16u, &fp_regs.st[4]);
  out.emplace_back(RegisterID::kX64_st5, 16u, &fp_regs.st[5]);
  out.emplace_back(RegisterID::kX64_st6, 16u, &fp_regs.st[6]);
  out.emplace_back(RegisterID::kX64_st7, 16u, &fp_regs.st[7]);

  return ZX_OK;
}

zx_status_t ReadVectorRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_vector_regs_t vec_regs;
  zx_status_t status = thread.read_state(ZX_THREAD_STATE_VECTOR_REGS, &vec_regs, sizeof(vec_regs));
  if (status != ZX_OK)
    return status;

  out.emplace_back(RegisterID::kX64_mxcsr, vec_regs.mxcsr);

  auto base = static_cast<uint32_t>(RegisterID::kX64_zmm0);
  for (size_t i = 0; i < 32; i++)
    out.emplace_back(static_cast<RegisterID>(base + i), 64u, &vec_regs.zmm[i]);

  return ZX_OK;
}

zx_status_t ReadDebugRegs(const zx::thread& thread, std::vector<debug::RegisterValue>& out) {
  zx_thread_state_debug_regs_t debug_regs;
  zx_status_t status =
      thread.read_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs, sizeof(debug_regs));
  if (status != ZX_OK)
    return status;

  out.emplace_back(RegisterID::kX64_dr0, debug_regs.dr[0]);
  out.emplace_back(RegisterID::kX64_dr1, debug_regs.dr[1]);
  out.emplace_back(RegisterID::kX64_dr2, debug_regs.dr[2]);
  out.emplace_back(RegisterID::kX64_dr3, debug_regs.dr[3]);
  out.emplace_back(RegisterID::kX64_dr6, debug_regs.dr6);
  out.emplace_back(RegisterID::kX64_dr7, debug_regs.dr7);

  return ZX_OK;
}

}  // namespace

void SaveGeneralRegs(const zx_thread_state_general_regs_t& input,
                     std::vector<debug::RegisterValue>& out) {
  out.emplace_back(RegisterID::kX64_rax, input.rax);
  out.emplace_back(RegisterID::kX64_rbx, input.rbx);
  out.emplace_back(RegisterID::kX64_rcx, input.rcx);
  out.emplace_back(RegisterID::kX64_rdx, input.rdx);
  out.emplace_back(RegisterID::kX64_rsi, input.rsi);
  out.emplace_back(RegisterID::kX64_rdi, input.rdi);
  out.emplace_back(RegisterID::kX64_rbp, input.rbp);
  out.emplace_back(RegisterID::kX64_rsp, input.rsp);
  out.emplace_back(RegisterID::kX64_r8, input.r8);
  out.emplace_back(RegisterID::kX64_r9, input.r9);
  out.emplace_back(RegisterID::kX64_r10, input.r10);
  out.emplace_back(RegisterID::kX64_r11, input.r11);
  out.emplace_back(RegisterID::kX64_r12, input.r12);
  out.emplace_back(RegisterID::kX64_r13, input.r13);
  out.emplace_back(RegisterID::kX64_r14, input.r14);
  out.emplace_back(RegisterID::kX64_r15, input.r15);
  out.emplace_back(RegisterID::kX64_rip, input.rip);
  out.emplace_back(RegisterID::kX64_rflags, input.rflags);
  out.emplace_back(RegisterID::kX64_fsbase, input.fs_base);
  out.emplace_back(RegisterID::kX64_gsbase, input.gs_base);
}

zx_status_t ReadRegisters(const zx::thread& thread, const debug::RegisterCategory& cat,
                          std::vector<debug::RegisterValue>& out) {
  switch (cat) {
    case debug::RegisterCategory::kGeneral:
      return ReadGeneralRegs(thread, out);
    case debug::RegisterCategory::kFloatingPoint:
      return ReadFPRegs(thread, out);
    case debug::RegisterCategory::kVector:
      return ReadVectorRegs(thread, out);
    case debug::RegisterCategory::kDebug:
      return ReadDebugRegs(thread, out);
    case debug::RegisterCategory::kNone:
    case debug::RegisterCategory::kLast:
      LOGS(Error) << "Asking to get none/last category";
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
      zx_thread_state_fp_regs_t regs;
      zx_status_t res = thread.read_state(ZX_THREAD_STATE_FP_REGS, &regs, sizeof(regs));
      if (res != ZX_OK)
        return res;

      // Overwrite the values.
      res = WriteFloatingPointRegisters(registers, &regs);
      if (res != ZX_OK)
        return res;

      return thread.write_state(ZX_THREAD_STATE_FP_REGS, &regs, sizeof(regs));
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
  uint32_t begin = static_cast<uint32_t>(RegisterID::kX64_rax);
  uint32_t last = static_cast<uint32_t>(RegisterID::kX64_rflags);

  uint64_t* output_array = reinterpret_cast<uint64_t*>(regs);

  for (const debug::RegisterValue& reg : updates) {
    if (reg.data.size() != 8)
      return ZX_ERR_INVALID_ARGS;

    // zx_thread_state_general_regs_t has the same layout as the RegisterID
    // enum for x64 general registers.
    uint32_t id = static_cast<uint32_t>(reg.id);
    if (id < begin || id > last)
      return ZX_ERR_INVALID_ARGS;

    // Insert the value to the correct offset.
    output_array[id - begin] = *reinterpret_cast<const uint64_t*>(reg.data.data());
  }

  return ZX_OK;
}

zx_status_t WriteFloatingPointRegisters(const std::vector<debug::RegisterValue>& updates,
                                        zx_thread_state_fp_regs_t* regs) {
  for (const auto& reg : updates) {
    zx_status_t status = ZX_OK;
    if (reg.id >= RegisterID::kX64_st0 && reg.id <= RegisterID::kX64_st7) {
      // FP stack value.
      uint32_t stack_index =
          static_cast<uint32_t>(reg.id) - static_cast<uint32_t>(RegisterID::kX64_st0);
      status = WriteRegisterValue(reg, &regs->st[stack_index]);
    } else {
      // FP control registers.
      switch (reg.id) {
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fcw);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fsw);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(ftw);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fop);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fip);
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(fdp);
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
  for (const auto& reg : updates) {
    zx_status_t status = ZX_OK;
    if (static_cast<uint32_t>(reg.id) >= static_cast<uint32_t>(RegisterID::kX64_zmm0) &&
        static_cast<uint32_t>(reg.id) <= static_cast<uint32_t>(RegisterID::kX64_zmm31)) {
      uint32_t stack_index =
          static_cast<uint32_t>(reg.id) - static_cast<uint32_t>(RegisterID::kX64_zmm0);
      status = WriteRegisterValue(reg, &regs->zmm[stack_index]);
    } else {
      switch (reg.id) {
        IMPLEMENT_CASE_WRITE_REGISTER_VALUE(mxcsr);
        default:
          status = ZX_ERR_INVALID_ARGS;
          break;
      }
      if (status != ZX_OK)
        return status;
    }
  }
  return ZX_OK;
}

zx_status_t WriteDebugRegisters(const std::vector<debug::RegisterValue>& updates,
                                zx_thread_state_debug_regs_t* regs) {
  for (const auto& reg : updates) {
    zx_status_t status = ZX_OK;
    switch (reg.id) {
      case RegisterID::kX64_dr0:
        status = WriteRegisterValue(reg, &regs->dr[0]);
        break;
      case RegisterID::kX64_dr1:
        status = WriteRegisterValue(reg, &regs->dr[1]);
        break;
      case RegisterID::kX64_dr2:
        status = WriteRegisterValue(reg, &regs->dr[2]);
        break;
      case RegisterID::kX64_dr3:
        status = WriteRegisterValue(reg, &regs->dr[3]);
        break;
      case RegisterID::kX64_dr6:
        status = WriteRegisterValue(reg, &regs->dr6);
        break;
      case RegisterID::kX64_dr7:
        status = WriteRegisterValue(reg, &regs->dr7);
        break;
      default:
        status = ZX_ERR_INVALID_ARGS;
        break;
    }

    if (status != ZX_OK)
      return status;
  }
  return ZX_OK;
}

debug_ipc::ExceptionType DecodeExceptionType(const zx::thread& thread, uint32_t exception_type) {
  return debug_ipc::DecodeX64Exception(
      exception_type, [&thread]() -> std::optional<debug_ipc::X64DebugRegs> {
        zx_thread_state_debug_regs_t debug_regs;
        zx_status_t status =
            thread.read_state(ZX_THREAD_STATE_DEBUG_REGS, &debug_regs, sizeof(debug_regs));
        if (status != ZX_OK) {
          DEBUG_LOG(Archx64) << "Could not get debug regs: " << zx_status_get_string(status);
          return std::nullopt;
        }

        debug_ipc::X64DebugRegs ret;

        ret.dr0 = debug_regs.dr[0];
        ret.dr1 = debug_regs.dr[1];
        ret.dr2 = debug_regs.dr[2];
        ret.dr3 = debug_regs.dr[3];
        ret.dr6 = debug_regs.dr6;
        ret.dr7 = debug_regs.dr7;

        return ret;
      });
}

debug_ipc::ExceptionRecord FillExceptionRecord(const zx_exception_report_t& in) {
  debug_ipc::ExceptionRecord record;

  record.valid = true;
  record.arch.x64.vector = in.context.arch.u.x86_64.vector;
  record.arch.x64.err_code = in.context.arch.u.x86_64.err_code;
  record.arch.x64.cr2 = in.context.arch.u.x86_64.cr2;

  return record;
}

}  // namespace arch
}  // namespace debug_agent
