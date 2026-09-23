// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_LEVEL_CONTROLLER_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_LEVEL_CONTROLLER_H_

#include <lib/zx/result.h>

#include <atomic>

#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>
#include <fbl/vector.h>

namespace power_management {

// Forward declaration.
struct PowerLevelUpdateRequest;

// Enum representing supported control interfaces.
enum class ControlInterface : uint64_t {
  kCpuDriver,
  kArmPsci,
  kArmWfi,
  kRiscvSbi,
  kRiscvWfi,
};

constexpr const char* ToString(ControlInterface control_interface) {
  switch (control_interface) {
    case ControlInterface::kCpuDriver:
      return "CPU_DRIVER";
    case ControlInterface::kArmPsci:
      return "ARM_PSCI";
    case ControlInterface::kArmWfi:
      return "ARM_WFI";
    case ControlInterface::kRiscvSbi:
      return "RISCV_SBI";
    case ControlInterface::kRiscvWfi:
      return "RISCV_WFI";
    default:
      return "[unknown]";
  }
}

// Base class for power level controllers that can control the active power
// levels CPU power domains.
class PowerLevelController : public fbl::RefCounted<PowerLevelController> {
 public:
  explicit PowerLevelController(ControlInterface control_interface)
      : control_interface_{control_interface} {}
  virtual ~PowerLevelController() = default;

  PowerLevelController(const PowerLevelController&) = delete;
  PowerLevelController& operator=(const PowerLevelController&) = delete;

  // Posts a pending request making it available for the entity in charge of executing the
  // transition. This method must be complemented by an acknowledgment of a transition.
  virtual zx::result<uint32_t> Post(const PowerLevelUpdateRequest& pending) = 0;

  // Returns the current device-specific power level (i.e. the value passed as a
  // control argument) for the given domain id, if supported by the controller.
  // May be implemented by in-kernel CPU drivers that are capable of querying
  // the current hardware state.
  virtual zx::result<uint64_t> GetCurrentPowerLevel(uint32_t domain_id) const {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  // Unique id of the `PowerLevelController`, used for validation.
  virtual uint64_t id() const = 0;

  // Determines whether the controller is serving requests or not.
  // Allows scheduler instances to interrogate whether they should consider any
  // active power levels or not.
  bool is_serving() const { return serving_.load(std::memory_order_relaxed); }

  // Returns the control interface this power level controller implements.
  ControlInterface control_interface() const { return control_interface_; }

 protected:
  // Implementations will determine when to switch this flag. The flag must always
  // be initialized as true.
  std::atomic<bool> serving_{true};

 private:
  ControlInterface control_interface_;
};

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_LEVEL_CONTROLLER_H_
