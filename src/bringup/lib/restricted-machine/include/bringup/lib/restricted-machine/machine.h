// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_H_

#include <lib/elfldltl/constants.h>
#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/testonly-syscalls.h>
#include <zircon/types.h>

#include <type_traits>
#include <vector>

#include <bringup/lib/restricted-machine/environment.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/register-state.h>
#include <fbl/ref_ptr.h>

namespace restricted_machine {

// A Machine represents a single instance of a restricted mode execution
// environment. It is responsible for managing the thread, its stack, and the
// register state for entering and leaving restricted mode.
class Machine {
 public:
  Machine(fbl::RefPtr<Environment> environment) : environment_(environment) {}
  virtual ~Machine() = default;

  // The default number of bytes to reserve for the stack.
  static const uint64_t kDefaultStackBytes;

  // Instantiates a new restricted mode machine in the supplied environment.
  //
  // |reserved_stack_size| will be allocated for the stack along with a small
  // amount of memory for TLS usage.
  //
  // |exception_report| when null, the exception report will be returned in the
  // mode state vmo.  When non-null, an exception report generated while
  // executing in restricted mode will be copied here.
  virtual bool Initialize(uint64_t reserved_stack_size = kDefaultStackBytes,
                          zx_exception_report_t *exception_report = nullptr);

  // Enables or disables the loading and saving of FPU registers on entry to
  // and exit from restricted mode.
  virtual void enable_fpu_registers(bool enable_fpu_registers);

  // Returns a pointer to the FPU register state.
  //
  // The vector will be valid after a call to `enable_fpu_registers(true)`.
  // The vector must be exactly sized to RegisterState::kFpuBufferSize or
  // Enter() will not set or save the FPU registers.
  virtual std::vector<uint8_t> *FpuRegisters();

  // Calls the |symbol| in the machine's environment.
  //
  // The function is identified by its symbol name. Up to 4 pointer arguments
  // can be passed.
  //
  // If addressable memory differs between the caller and the restricted
  // machine environment, use Environment::MakeArgument<> to allocate
  // and construct arguments which may be passed safely into Call().
  //
  // On success, a 64-bit value will be returned. For 32-bit environments, this
  // normally is the span of two registers.
  //
  // If an error entering restricted mode occurs, it will be returned verbatim.
  // If the restricted mode call returns through an unexpected path, such as an
  // unexpected system call or exception, ZX_ERR_OUT_OF_RANGE is returned and
  // the reason code can be read with |last_reason()|.
  template <typename... Args>
  zx::result<uint64_t> Call(const std::string_view &symbol, Args... vargs) {
    std::vector<uint64_t> args;
    if (sizeof...(vargs) > 0) {
      auto result = prepArgs(&args, vargs...);
      if (result.is_error()) {
        RM_LOG(ERROR) << __func__ << ": error preparing arguments";
        return result.take_error();
      }
    }
    // Look up the symbol or fail
    auto addr = environment_->SymbolAddress(symbol);
    if (addr.is_error()) {
      RM_LOG(ERROR) << "failed to find requested symbol: " << symbol;
      return addr.take_error();
    }
    ZX_ASSERT(args.size() < 5);
    // Pad the arguments to always be 4, filling with zeros.
    while (args.size() < 4) {
      args.push_back(0);
    }
    zx::result<uint64_t> result = Thunk(addr.value(), args[0], args[1], args[2], args[3]);

    // If the reason code is something other than syscall, we return that in
    // the error.
    if (result.is_ok()) {
      if (result.value() == ZX_RESTRICTED_REASON_SYSCALL) {
        // The thunk code will call syscall 0xffe on completion with the return
        // register of the called function in syscall_arg(0).
        if (registers_->syscall_number() == 0xffe) {
          // If the environment is 32-bit, grab the first two args, otherwise
          // just the first. Anything more complicated will require the caller
          // to pass in a pointer as a parameter to retrieve the return value.
          if (registers_->register_bytes() == sizeof(uint64_t)) {
            return zx::ok(registers_->syscall_arg(0));
          } else {
            return zx::ok((registers_->syscall_arg(1) << 32) | registers_->syscall_arg(0));
          }
        }
        RM_LOG(ERROR) << "unexpected system call seen: " << registers_->syscall_number();
      } else {
        RM_LOG(ERROR) << "unexpected return reason seen: " << result.value();
        LogState();
      }
    } else {
      RM_LOG(ERROR) << "error entering restricted mode";
      return result.take_error();
    }
    // For unexpected errors returning from restricted mode, we use "out of range".
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  // Loads the machine state from the restricted machine, including registers
  // and exception state.
  //
  // This function is only useful when using the Enter*() calls.
  virtual zx::result<> LoadState();

  // Commits the machine state to the restricted machine, which will determine
  // its register values on the next entry into restricted mode.
  //
  // This function is only necessary when using the Enter() or Continue().
  virtual zx::result<> CommitState();

  // A wrapper around the restricted_enter() system call that uses the
  // arch-specific assembly.
  //
  // This "continues" execution without loading or committing state, but it does
  // update the last_reason().
  virtual zx_status_t Continue();

  // A wrapper around zx_restricted_kick() which kicks the current thread unless
  // another is provided.
  static zx_status_t Kick(uint32_t options = 0, std::optional<zx_handle_t> thread = std::nullopt);

  // Enters restricted mode using the current register state.
  virtual zx::result<uint64_t> Enter();

  // Attempts to execute code at |fn_address| with the given parameters packed
  // into an array for the Environment's thunk function to extract.
  virtual zx::result<uint64_t> Thunk(uint64_t fn_address, uint64_t arg0 = 0, uint64_t arg1 = 0,
                                     uint64_t arg2 = 0, uint64_t arg3 = 0);

  // Prepares the machine to execute at the thunk callsite on the next call to
  // Enter().
  //
  // This allows users to perform sensitive register changes with minimal code
  // before restricted mode entry.
  //
  // The result will contain the (unique_ptr) allocation which holds the
  // arguments for the call. When it goes out of scope, the memory will be
  // released.
  virtual zx::result<Environment::Allocation> ThunkPrepare(uint64_t fn_address, uint64_t arg0 = 0,
                                                           uint64_t arg1 = 0, uint64_t arg2 = 0,
                                                           uint64_t arg3 = 0);

  // Provides access to the register state without taking ownership.
  virtual RegisterState *registers() { return registers_.get(); }

  // Returns the reason for the last exit from restricted mode.
  virtual zx_restricted_reason_t last_reason() const { return last_reason_code_; }

  // Logs the current register state.
  //
  // If |if_not_reason| is provided, the state will only be logged if the last
  // exit reason is different.
  virtual void LogState(std::optional<zx_restricted_reason_t> if_not_reason = std::nullopt);

  // Returns a ref-counted pointer to the Environment object.
  virtual fbl::RefPtr<Environment> environment() { return environment_; }

 protected:
  // Validates and extracts a pointer to a restricted mode accessible
  // allocation.
  template <typename T>
  zx::result<uint64_t> CollectArgPtr(const T target_obj) {
    ZX_ASSERT(std::is_pointer<T>::value);
    if (environment_->address_limit() != 0) {
      if (reinterpret_cast<uint64_t>(target_obj) >= environment_->address_limit()) {
        RM_LOG(ERROR) << "Arguments not reachable. Please use Environment::MakeArgument()";
        return zx::error(ZX_ERR_OUT_OF_RANGE);
      }
    }
    return zx::ok(reinterpret_cast<uint64_t>(target_obj));
  }

  // Collects a pointer after validating its accessibility.
  template <typename T>
  zx::result<uint64_t> CollectArg(T target_obj) {
    if constexpr (std::is_pointer<T>::value) {
      return CollectArgPtr(target_obj);
    }
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Allocates addressable memory to store the function arguments and then
  // returns them in |args|.
  zx::result<> prepArgs(std::vector<uint64_t> *args) { return zx::ok(); }
  template <typename T, typename... Args>
  zx::result<> prepArgs(std::vector<uint64_t> *args, T t, Args... vargs) {
    std::vector<uint64_t> ret;
    auto result = CollectArg(t);
    if (result.is_error()) {
      return result.take_error();
    }
    args->push_back(result.value());
    return prepArgs(args, vargs...);
  }

 private:
  size_t stack_mem_size_ = 0;
  Environment::Allocation tls_mem_;
  Environment::Allocation stack_mem_;
  Environment::Allocation shadow_stack_mem_;

  // Restricted mode state vmo
  zx::vmo state_vmo_ = {};
  zx_restricted_reason_t last_reason_code_ = 0;
  // If null, the report will be in the mode state vmo.
  zx_exception_report *exception_report_{};

  // We use a unique_ptr here so that we can place the machine specific implementation.
  std::unique_ptr<RegisterState> registers_{};
  // Storage for optional FPU register loading and storing.
  std::vector<uint8_t> fpu_registers_{};
  // Holds a reference to the environment it depends on.
  fbl::RefPtr<Environment> environment_{};
};

}  // namespace restricted_machine
#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_MACHINE_H_
