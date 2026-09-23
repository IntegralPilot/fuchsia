// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_PDEV_POWER_LEVEL_CONTROLLER_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_PDEV_POWER_LEVEL_CONTROLLER_H_

#include <lib/power-management/power-level-controller.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <kernel/mutex.h>

namespace power_management {

class PDevPowerLevelController final : public PowerLevelController {
  enum PrivateConstructorTag : bool { PrivateConstructorValue };

 public:
  static zx::result<fbl::RefPtr<PDevPowerLevelController>> Get(uint32_t domain_id);

  static void ResetForTest();

  explicit PDevPowerLevelController(PrivateConstructorTag tag, size_t domain_count)
      : PowerLevelController(ControlInterface::kCpuDriver), domain_count_{domain_count} {}

  ~PDevPowerLevelController() final = default;

  zx::result<uint32_t> Post(const PowerLevelUpdateRequest& pending) final;

  zx::result<uint64_t> GetCurrentPowerLevel(uint32_t domain_id) const final;

  // Return a koid that will never collide with a valid dispatcher koid (i.e.
  // the PortDispatcher registered when setting up the domain).
  //
  // IMPORTANT: This prevents userspace from updating the active power level
  // bookkeeping when this power level controller is being used, since this id
  // is compared when handling zx_system_set_processor_power_state. It should be
  // impossible for userspace to supply a port object with a koid that matches
  // this id.
  uint64_t id() const final { return ZX_KOID_INVALID; }

  static bool IsSupported();

  size_t domain_count() const { return domain_count_; }

 private:
  DECLARE_SINGLETON_MUTEX(InstanceLock);

  TA_GUARDED(InstanceLock::Get())
  inline static fbl::RefPtr<PDevPowerLevelController> instance_;

  const size_t domain_count_;
};

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_PDEV_POWER_LEVEL_CONTROLLER_H_
