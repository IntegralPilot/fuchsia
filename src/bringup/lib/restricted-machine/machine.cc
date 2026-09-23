// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <lib/elfldltl/constants.h>
#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <cstring>

#include <bringup/lib/restricted-machine/environment.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine.h>
#include <bringup/lib/restricted-machine/register-state.h>
#include <fbl/ref_ptr.h>

namespace restricted_machine {

extern "C" zx_status_t restricted_enter_wrapper(uint32_t options, uintptr_t vector_table,
                                                zx_restricted_reason_t* reason_code,
                                                void* fpu_register_data);
extern "C" void restricted_exit(uintptr_t context, zx_restricted_reason_t reason_code);

constexpr uint64_t Machine::kDefaultStackBytes = 8192;

bool Machine::Initialize(uint64_t reserved_stack_size, zx_exception_report_t* exception_report) {
  registers_ = RegisterStateFactory::Create(environment_->machine());
  registers_->InitializeRegisters();

  auto result = environment_->Allocate(RegisterState::kTlsStorageSize);
  if (result.is_error()) {
    RM_LOG(ERROR) << __func__ << ": failed. Allocation failed for TLS memory";
    return false;
  }
  tls_mem_ = std::move(result.value());
  memset(reinterpret_cast<void*>(tls_mem_->base), 0, RegisterState::kTlsStorageSize);
  // We set the TLS registers initially so that clients that don't use
  // Call/ThunkPrepare will still be able to use the allocation.
  registers_->set_tls(reinterpret_cast<TlsStorage*>(tls_mem_->base));

  stack_mem_size_ = reserved_stack_size;
  if (stack_mem_size_ > 0) {
    result = environment_->Allocate(stack_mem_size_);
    if (result.is_error()) {
      RM_LOG(ERROR) << __func__ << ": failed. Allocation failed for stack memory";
      return false;
    }
    stack_mem_ = std::move(result.value());

    result = environment_->Allocate(stack_mem_size_);
    if (result.is_error()) {
      RM_LOG(ERROR) << __func__ << ": failed. Allocation failed for shadow stack memory";
      return false;
    }
    shadow_stack_mem_ = std::move(result.value());
  }

  exception_report_ = exception_report;
  if (zx_restricted_bind_state == 0 ||
      ZX_OK != zx_restricted_bind_state(0, state_vmo_.reset_and_get_address(), exception_report_)) {
    RM_LOG(ERROR) << "Initialize: failed to reset restricted state";
    return false;
  }

  return true;
}

void Machine::enable_fpu_registers(bool enable_fpu_registers) {
  if (!enable_fpu_registers) {
    fpu_registers_.resize(0);
  } else {
    fpu_registers_.resize(RegisterState::kFpuBufferSize);
  }
}

std::vector<uint8_t>* Machine::FpuRegisters() {
  ZX_ASSERT(fpu_registers_.size() == RegisterState::kFpuBufferSize);
  return &fpu_registers_;
}

void Machine::LogState(std::optional<zx_restricted_reason_t> if_not_reason) {
  // Don't log if the last reason matches the optional reason.
  if (if_not_reason.has_value() && last_reason_code_ == if_not_reason.value()) {
    return;
  }

  switch (last_reason_code_) {
    case ZX_RESTRICTED_REASON_SYSCALL:
    case ZX_RESTRICTED_REASON_KICK: {
      zx_restricted_state_t rstate = {};
      if (state_vmo_.read(&rstate, 0, sizeof(rstate)) != ZX_OK) {
        return;
      }
      registers_->PrintState(rstate);
      return;
    }
    case ZX_RESTRICTED_REASON_EXCEPTION: {
      zx_restricted_exception_t exception_state = {};

      if (!exception_report_) {
        // If there's no exception report pointer, then the mode state vmo will contain a full
        // zx_restricted_exception_t.
        if (state_vmo_.read(&exception_state, 0, sizeof(exception_state)) != ZX_OK) {
          return;
        }
        RM_LOG(INFO) << "dumping exception state to stdout";
        registers_->PrintExceptionState(exception_state);
        return;
      }

      // There *is* an exception report pointer. Print the report and the state. The mode state vmo
      // will only contain a zx_restricted_state_t.
      RM_LOG(INFO) << "dumping exception state to stdout";
      registers_->PrintExceptionReport(*exception_report_);
      zx_restricted_state_t rstate = {};
      if (state_vmo_.read(&rstate, 0, sizeof(rstate)) != ZX_OK) {
        return;
      }

      registers_->PrintState(rstate);
      return;
    }

    case ZX_RESTRICTED_REASON_EXCEPTION_LOST:
      RM_LOG(ERROR) << "exception report was lost";
      return;

    default:
      RM_LOG(ERROR) << "unexpected reason code: " << last_reason_code_;
      return;
  }
}

// Read the state out of the thread.
zx::result<> Machine::LoadState() {
  ZX_ASSERT(0 == state_vmo_.read(&registers_->restricted_state(), 0,
                                 sizeof(registers_->restricted_state())));
  if (last_reason_code_ == ZX_RESTRICTED_REASON_EXCEPTION) {
    if (exception_report_) {
      // The exception report was copied out to exception_report_.
      registers_->exception_report() = *exception_report_;
    } else {
      // The exception report was placed in the mode state VMO.
      ZX_ASSERT(0 == state_vmo_.read(&registers_->exception_report(),
                                     sizeof(registers_->restricted_state()),
                                     sizeof(registers_->exception_report())));
    }
  } else if (last_reason_code_ == ZX_RESTRICTED_REASON_EXCEPTION_LOST) {
    // The report was lost. Zero out the struct to avoid confusion.
    registers_->exception_report() = {};
  }
  return zx::ok();
}

zx::result<> Machine::CommitState() {
  ZX_ASSERT(state_vmo_.write(&registers_->restricted_state(), 0,
                             sizeof(registers_->restricted_state())) == 0);
  return zx::ok();
}

zx::result<Environment::Allocation> Machine::ThunkPrepare(uint64_t fn_address, uint64_t arg0,
                                                          uint64_t arg1, uint64_t arg2,
                                                          uint64_t arg3) {
  auto thunk_addr = environment_->SymbolAddress(Environment::kThunkFunctionName);
  if (thunk_addr.is_error()) {
    return thunk_addr.take_error();
  }
  if (stack_mem_size_ == 0) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  // Prepare the arguments using the end of shared memory.
  auto alloc_result = environment_->Allocate(sizeof(uint64_t) * 6);
  if (alloc_result.is_error()) {
    return alloc_result.take_error();
  }
  auto arg_mem = std::move(alloc_result.value());
  uint64_t* arg_array = reinterpret_cast<uint64_t*>(arg_mem->base);
  arg_array[0] = fn_address;
  arg_array[1] = arg0;
  arg_array[2] = arg1;
  arg_array[3] = arg2;
  arg_array[4] = arg3;

  registers_->set_arg_regs(reinterpret_cast<uintptr_t>(arg_mem->base), 0);
  // Pick a stack around the middle of the allocation, but keep it page aligned.
  static const size_t kPageSize = zx_system_get_page_size();
  static constexpr size_t kMinAlignAt = 16;
  // If the stack is too small, then we align at 16 bytes.
  size_t align_at = kPageSize - 1;
  if (stack_mem_size_ >> 1 < kPageSize) {
    align_at = kMinAlignAt - 1;
  }
  uint64_t aligned_ptr = ((stack_mem_->base + stack_mem_size_ / 2) + align_at) & ~align_at;
  registers_->set_sp(aligned_ptr);
  aligned_ptr = ((shadow_stack_mem_->base + stack_mem_size_ / 2) + align_at) & ~align_at;
  registers_->set_shadow_sp(aligned_ptr);
  registers_->set_tls(reinterpret_cast<TlsStorage*>(tls_mem_->base));
  registers_->set_pc(thunk_addr.value());
  auto result = CommitState();
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(std::move(arg_mem));
}

zx::result<uint64_t> Machine::Thunk(uint64_t fn_address, uint64_t arg0, uint64_t arg1,
                                    uint64_t arg2, uint64_t arg3) {
  auto result = ThunkPrepare(fn_address, arg0, arg1, arg2, arg3);
  if (result.is_error()) {
    return result.take_error();
  }
  return Enter();
}

zx::result<uint64_t> Machine::Enter() {
  zx_status_t status = Continue();
  if (status != 0) {
    // Propagate failure.
    return zx::error(status);
  }

  auto result = LoadState();
  if (result.is_error()) {
    return result.take_error();
  }
  return zx::ok(static_cast<int>(last_reason_code_));
}

zx_status_t Machine::Continue() {
  void* fpu_register_data = nullptr;
  if (fpu_registers_.size() == RegisterState::kFpuBufferSize) {
    fpu_register_data = fpu_registers_.data();
  }

  return restricted_enter_wrapper(0, reinterpret_cast<uintptr_t>(&restricted_exit),
                                  &last_reason_code_, fpu_register_data);
}

zx_status_t Machine::Kick(uint32_t options, std::optional<zx_handle_t> thread) {
  if (zx_restricted_kick == 0) {
    return ZX_ERR_NOT_SUPPORTED;
  }
  if (thread.has_value()) {
    return zx_restricted_kick(thread.value(), options);
  } else {
    zx::unowned<zx::thread> current_thread(thrd_get_zx_handle(thrd_current()));
    // Issue a kick on ourselves which should apply to the next attempt to enter restricted mode.
    return zx_restricted_kick(current_thread->get(), options);
  }
}

}  // namespace restricted_machine
