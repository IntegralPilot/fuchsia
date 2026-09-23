// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// This header defines what each architecture implementation
// will need to provide for testing.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_REGISTER_STATE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_REGISTER_STATE_H_

#include <unistd.h>
#include <zircon/syscalls-next.h>

#include <cassert>
#include <cinttypes>
#include <memory>

#include <bringup/lib/restricted-machine/internal/arch-helpers.h>
#include <bringup/lib/restricted-machine/internal/common.h>

namespace restricted_machine {

// Each valid RegisterState implementation must provide their own
// TlsStorage to contain TLS ABI data.
struct TlsStorage;

// Each valid register state (usually one per supported ELF machine spec) must
// provide implementations and/or derived implementations in restricted-mode.cc.
//
// This class is a wrapper around zx_restricted_state_t which allows for
// target-agnostic interactions with the state data.k
class RegisterState {
 public:
  // The number of bytes needed to store the floating point registers.
  static const uint16_t kFpuBufferSize;

  // The size needed to store TlsStorage.
  static const size_t kTlsStorageSize;

  // Default constructor and destructor.
  RegisterState() = default;
  virtual ~RegisterState() = default;

  // Initializes the register state for a new restricted mode machine.
  //
  // |tls_storage|: A pointer to the thread-local storage for the machine.
  virtual void InitializeRegisters();

  // Initializes the register state from a thread's general purpose registers.
  virtual void InitializeFromThreadState(const zx_thread_state_general_regs_t& regs);

  // Returns the program counter.
  virtual uintptr_t pc() const;

  // Sets the program counter.
  virtual void set_pc(uintptr_t pc);

  // Sets the stack pointer.
  virtual void set_sp(uintptr_t sp);

  // Sets the shadow stack pointer.
  virtual void set_shadow_sp(uintptr_t sp);

  // Sets the first two argument registers.
  virtual void set_arg_regs(uint64_t arg0, uint64_t arg1);

  // Sets the return value when returning from a syscall.
  virtual void set_syscall_return(uint64_t value);

  // Returns the syscall number from the last exit from restricted mode.
  virtual uint64_t syscall_number() const;

  // Sets the register that holds the system call number in restricted mode.
  virtual void set_syscall_number(uint64_t number);

  // Returns the value of a syscall argument register.
  virtual uintptr_t syscall_arg(unsigned index) const;

  // Returns true if the architecture is supported.
  virtual bool ArchSupported() const { return true; }

  // Returns the size of an instruction in bytes.
  virtual uint64_t instruction_size() const { return 4; }

  // Returns the size of a general purpose register in bytes.
  virtual uint64_t register_bytes() const { return 8; }

  // Prints the restricted mode state.
  virtual void PrintState(const zx_restricted_state_t& state);

  // Prints the restricted mode exception state.
  virtual void PrintExceptionState(const zx_restricted_exception_t& exc);

  // Prints an exception report.
  virtual void PrintExceptionReport(const zx_exception_report_t& report);

  // Provides access to the underlying zx_restricted_state_t.
  zx_restricted_state_t& restricted_state() { return state_; }

  // Provides access to the underlying zx_exception_report_t.
  zx_exception_report_t& exception_report() { return exception_report_; }

  // Returns a pointer to the thread-local storage.
  virtual TlsStorage* tls() const { return tls_; }

  // Sets the thread-local storage pointer and updates rhe registers.
  virtual void set_tls(TlsStorage* tls);

 protected:
  // This is selected by the compile target and contains the relevant register
  // state. If the target machine requires a different zx_restricted_state_t,
  // then this will need to be converted to a managed pointer.
  zx_restricted_state_t state_ = {};

  // Populated when reason code is ZX_RESTRICTED_REASON_EXCEPTION.
  zx_exception_report_t exception_report_ = {};

  // TlsStorage will be provided by the fixture during |InitializeRegisters|.
  // Ownership is not taken by this class.
  TlsStorage* tls_ = nullptr;
};

// A factory for creating RegisterState objects for a given machine type.
class RegisterStateFactory {
 public:
  // Creates a RegisterState object for the given machine type.
  static std::unique_ptr<RegisterState> Create(const MachineType& machine);
};

}  // namespace restricted_machine

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_REGISTER_STATE_H_
