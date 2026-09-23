// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/power-management/pdev-power-level-controller.h"

#include <lib/power-management/power-state.h>

#include <dev/power.h>
#include <kernel/scheduler.h>

namespace power_management {

void PDevPowerLevelController::ResetForTest() {
  Guard<Mutex> guard{InstanceLock::Get()};
  instance_ = nullptr;
}

zx::result<fbl::RefPtr<PDevPowerLevelController>> PDevPowerLevelController::Get(
    uint32_t domain_id) {
  Guard<Mutex> guard{InstanceLock::Get()};
  if (instance_) {
    if (domain_id >= instance_->domain_count()) {
      return zx::error(ZX_ERR_OUT_OF_RANGE);
    }
    return zx::ok(instance_);
  }

  const zx::result<size_t> domain_count = power_opp_get_domain_count();
  if (domain_count.is_error()) {
    return zx::error(domain_count.error_value());
  }
  if (domain_id >= domain_count.value()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  fbl::AllocChecker ac;
  fbl::RefPtr<PDevPowerLevelController> instance =
      fbl::MakeRefCountedChecked<PDevPowerLevelController>(&ac, PrivateConstructorValue,
                                                           domain_count.value());
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  instance_ = ktl::move(instance);
  return zx::ok(instance_);
}

zx::result<uint32_t> PDevPowerLevelController::Post(const PowerLevelUpdateRequest& pending) {
  // Validate the inputs as defense-in-depth in case the pdev driver does not
  // validate them.
  if (pending.control != ControlInterface::kCpuDriver) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  if (pending.domain_id >= domain_count()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const zx_status_t status = power_opp_set(pending.domain_id, pending.control_argument);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return Scheduler::UpdatePowerLevel(pending.domain_id, id(), control_interface(),
                                     pending.control_argument);
}

zx::result<uint64_t> PDevPowerLevelController::GetCurrentPowerLevel(uint32_t domain_id) const {
  // Validate the input as defense-in-depth in case the pdev driver does not
  // validate it.
  if (domain_id >= domain_count()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  return power_opp_get(domain_id);
}

bool PDevPowerLevelController::IsSupported() {
  const zx::result<size_t> result = power_opp_get_domain_count();
  return result.is_ok() && result.value() > 0;
}

}  // namespace power_management
