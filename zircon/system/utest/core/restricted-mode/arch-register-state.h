// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// This header defines what each architecture implementation
// will need to provide for testing.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_REGISTER_STATE_H_
#define ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_REGISTER_STATE_H_

#include <lib/elfldltl/machine.h>
#include <unistd.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>

#include <cassert>
#include <cinttypes>
#include <memory>

// Each valid ArchRegisterState implementation must provide their own
// TlsStorage to contain TLS ABI data.
struct TlsStorage;

// The size needed to store TlsStorage.
extern const size_t kTlsStorageSize;

// The bytes needed to store the floating point registers.
extern const uint16_t kFpuBufferSize;

// The normal-mode view of restricted-mode state will change slightly depending on
// if the exit to normal-mode was caused by a syscall or an exception.
enum class RegisterMutation {
  kFromSyscall,
  kFromException,
};

// Each valid register state (usually one per supported ELF machine spec) must
// provide implementations and/ore derived implementations in restricted-mode.cc.
class ArchRegisterState {
 public:
  virtual ~ArchRegisterState() = default;
  virtual void InitializeRegisters(TlsStorage* tls_storage);
  virtual void InitializeFromThreadState(const zx_thread_state_general_regs_t& regs);
  virtual void VerifyTwiddledRestrictedState(RegisterMutation mutation) const;
  virtual void VerifyArchSpecificRestrictedState() const;
  virtual uintptr_t pc() const;
  virtual void set_pc(uintptr_t pc);
  virtual void set_arg_regs(uint64_t arg0, uint64_t arg1);
  virtual bool ArchSupported() const { return true; }

  virtual void PrintState(const zx_restricted_state_t& state);
  virtual void PrintExceptionState(const zx_restricted_exception_t& exc);

  zx_restricted_state_t& restricted_state() { return state_; }
  TlsStorage* tls() const { return tls_; }
  void set_tls(TlsStorage* tls) { tls_ = tls; }

 protected:
  // This is selected by the compile target and contains the relevant register
  // state. If the target machine requires a different zx_restricted_state_t,
  // then this will need to be converted to a managed pointer.
  zx_restricted_state_t state_ = {};

  // TlsStorage will be provided by the fixture during |InitializeRegisters|.
  // Ownership is not taken by this class.
  TlsStorage* tls_ = nullptr;
};

// Returns the correct ArchRegisterState for the ELF machine.
class ArchRegisterStateFactory {
 public:
  static std::unique_ptr<ArchRegisterState> Create(elfldltl::ElfMachine machine);
};

#endif  // ZIRCON_SYSTEM_UTEST_CORE_RESTRICTED_MODE_ARCH_REGISTER_STATE_H_
