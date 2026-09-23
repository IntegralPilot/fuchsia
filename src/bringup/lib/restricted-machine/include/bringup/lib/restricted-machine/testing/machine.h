// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
//
// testing::Machine wraps Machine to provide seamless calling of Environment-based
// symbols in normal mode or restricted mode.  If the MachineType is kNone,
// Call() (via Thunk) will directly invoke the function. Otherwise, it will use the
// underlying restricted_machine::Machine.
//
#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_MACHINE_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_MACHINE_H_

#include <bringup/lib/restricted-machine/internal/arch-helpers.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine.h>

namespace restricted_machine {

namespace testing {

// Wraps restricted_machine::Machine to enable parameterized testing to call
// code outside of restricted mode using the same restricted_machine interface.
//
// See //src/bringup/lib/restricted_machine/tests/example-tests.cc for use.
class Machine : public ::restricted_machine::Machine {
 public:
  Machine(fbl::RefPtr<Environment> environment) : ::restricted_machine::Machine(environment) {}
  virtual ~Machine() = default;

  // Indicates if Call() (via Thunk()) will use normal mode rather than restricted mode.
  virtual bool use_normal_mode() const { return use_normal_mode_; }
  virtual zx::result<> set_use_normal_mode(bool use_normal_mode) {
    if (use_normal_mode && environment()->machine() != MachineType::kNative) {
      RM_LOG(ERROR) << "Unable to use normal mode due to Environment machine mismatch: "
                    << environment()->machine();
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    }
    use_normal_mode_ = use_normal_mode;
    return zx::ok();
  }

  // Override Thunk to split behavior if |use_normal_mode_| is true.
  virtual zx::result<uint64_t> Thunk(uint64_t fn_address, uint64_t arg0 = 0, uint64_t arg1 = 0,
                                     uint64_t arg2 = 0, uint64_t arg3 = 0) override {
    if (use_normal_mode()) {
      RM_LOG(DEBUG) << "Calling 0x" << std::hex << fn_address << "in normal mode.";
      // Calling directly in normal mode requires creating a function prototype
      // to invoke.
      using CallType = uint64_t(uint64_t arg0, uint64_t arg1, uint64_t arg2, uint64_t arg3);
      std::function<CallType> fn(reinterpret_cast<CallType *>(fn_address));
      uint64_t result = fn(arg0, arg1, arg2, arg3);
      // Convince the caller of Thunk() that we ran this in RM.
      registers()->set_arg_regs(result, 0);  // All kNative are 64-bit.
      registers()->set_syscall_number(0xffe);
      return zx::ok(ZX_RESTRICTED_REASON_SYSCALL);
    }
    return restricted_machine::Machine::Thunk(fn_address, arg0, arg1, arg2, arg3);
  }

 private:
  bool use_normal_mode_{false};
};

}  // namespace testing
}  // namespace restricted_machine

#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_TESTING_MACHINE_H_
