// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/power-management/energy-model.h>
#include <lib/power-management/pdev-power-level-controller.h>
#include <lib/power-management/power-level-controller.h>
#include <lib/power-management/power-state.h>
#include <lib/unittest/unittest.h>
#include <zircon/errors.h>
#include <zircon/rights.h>

#include <pdev/power.h>
// TODO(https://fxbug.dev/415033686): Stop using `syscalls-next.h` on host.
#define FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_HOST
#include <zircon/syscalls-next.h>
#undef FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_HOST
#include <lib/stdcompat/utility.h>
#include <zircon/syscalls/port.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstdint>
#include <memory>

#include <arch/ops.h>
#include <fbl/alloc_checker.h>
#include <fbl/ref_ptr.h>
#include <kernel/cpu.h>
#include <kernel/deadline.h>
#include <kernel/event.h>
#include <kernel/spinlock.h>
#include <kernel/thread.h>
#include <ktl/limits.h>
#include <ktl/optional.h>
#include <object/handle.h>
#include <object/port_dispatcher.h>

namespace {

using power_management::ControlInterface;
using power_management::EnergyModel;
using power_management::PDevPowerLevelController;
using power_management::PowerDomain;
using power_management::PowerDomainSet;

using power_management::PowerLevelController;
using power_management::PowerLevelUpdateRequest;
using power_management::ProcessorPowerLevel;
using power_management::ProcessorPowerLevelTransition;

constexpr uint8_t kIdlePowerLevel = 0;
constexpr uint8_t kLowPowerLevel = 1;
constexpr uint8_t kMediumPowerLevel = 2;
constexpr uint8_t kHighPowerLevel = 3;
constexpr uint8_t kMaxPowerLevel = 4;

constexpr auto kPowerLevels = cpp20::to_array<ProcessorPowerLevel>({
    {
        .options = 0,
        .processing_rate = 0,                // 0%
        .power_coefficient_nw = 10'000'000,  // 10 mW
        .control_interface = ControlInterface::kArmWfi,
        .control_argument = kIdlePowerLevel,
        .diagnostic_name = "wfi",
    },
    {
        .options = 0,
        .processing_rate = 250,               // 25%
        .power_coefficient_nw = 100'000'000,  // 100 mW
        .control_interface = ControlInterface::kCpuDriver,
        .control_argument = kLowPowerLevel,
        .diagnostic_name = "OPP 1",
    },
    {
        .options = 0,
        .processing_rate = 500,               // 50%
        .power_coefficient_nw = 250'000'000,  // 250 mW
        .control_interface = ControlInterface::kCpuDriver,
        .control_argument = kMediumPowerLevel,
        .diagnostic_name = "OPP 1",
    },
    {
        .options = 0,
        .processing_rate = 750,               // 75%
        .power_coefficient_nw = 400'000'000,  // 400 mW
        .control_interface = ControlInterface::kCpuDriver,
        .control_argument = kHighPowerLevel,
        .diagnostic_name = "OPP 2",
    },
    {
        .options = 0,
        .processing_rate = 1000,              // 100%
        .power_coefficient_nw = 600'000'000,  // 600 mW
        .control_interface = ControlInterface::kCpuDriver,
        .control_argument = kMaxPowerLevel,
        .diagnostic_name = "OPP 3",
    },
});

constexpr auto kTransitions = cpp20::to_array<ProcessorPowerLevelTransition>({{}});

class FakePowerLevelController final : public PowerLevelController {
 public:
  FakePowerLevelController() : PowerLevelController(ControlInterface::kCpuDriver) {}

  zx::result<uint32_t> Post(const PowerLevelUpdateRequest& request) final {
    Guard<SpinLock, IrqSave> guard{&lock_};
    request_ = request;
    count_++;
    return zx::ok(0);
  }

  uint64_t id() const final { return 0; }

  ktl::optional<PowerLevelUpdateRequest> request() const {
    Guard<SpinLock, IrqSave> guard{&lock_};
    return request_;
  }

  void reset_request() {
    Guard<SpinLock, IrqSave> guard{&lock_};
    request_.reset();
  }

  size_t count() const {
    Guard<SpinLock, IrqSave> guard{&lock_};
    return count_;
  }

 private:
  mutable DECLARE_SPINLOCK(FakePowerLevelController) lock_;
  size_t count_ TA_GUARDED(lock_) = 0;
  ktl::optional<PowerLevelUpdateRequest> request_ TA_GUARDED(lock_) = ktl::nullopt;
};

bool SchedulerFlushesPendingControlRequests() {
  BEGIN_TEST;

  // Use the scheduler for the current CPU to test the power level control functionality. It doesn't
  // matter if the test thread remains on the same CPU, since power level requests can be initiated
  // from a different CPU than the target CPU.
  Scheduler& scheduler = percpu::GetCurrent().scheduler;

  zx::result energy_model = EnergyModel::Create(kPowerLevels, kTransitions);
  ASSERT_TRUE(energy_model.is_ok());

  fbl::AllocChecker ac;
  fbl::RefPtr controller = fbl::MakeRefCountedChecked<FakePowerLevelController>(&ac);
  ASSERT_TRUE(ac.check());

  auto domain = fbl::MakeRefCountedChecked<PowerDomain>(
      &ac, 1, cpu_num_to_mask(scheduler.this_cpu()), ktl::move(*energy_model), controller);
  ASSERT_TRUE(ac.check());

  // Set the current power domain, saving the previous domain to restore at the end of the test.
  const ktl::optional<uint8_t> restore_power_level = scheduler.GetActivePowerLevel();
  auto restore_domain = fit::defer([&, previous_domain_set = scheduler.ExchangePowerDomainSet(
                                           PowerDomainSet::CreateForTest(domain))] {
    scheduler.ExchangePowerDomainSet(previous_domain_set);
    if (restore_power_level.has_value()) {
      DEBUG_ASSERT(scheduler.UpdateActivePowerLevel(*restore_power_level).is_ok());
    }
  });
  ASSERT_EQ(domain.get(), scheduler.GetPowerDomainForTesting().get());
  ASSERT_OK(scheduler.UpdateActivePowerLevel(kMaxPowerLevel).status_value());

  // Request a transition to each active power level.
  for (size_t count = 0;
       uint8_t power_level : {kLowPowerLevel, kMediumPowerLevel, kHighPowerLevel, kMaxPowerLevel}) {
    scheduler.RequestPowerLevelForTesting(power_level);

    while (controller->count() < count + 1) {
      Thread::Current::SleepRelative(ZX_MSEC(1));
    }
    count++;
    ASSERT_EQ(controller->count(), count);

    const ktl::optional<PowerLevelUpdateRequest> request = controller->request();
    ASSERT_TRUE(request);
    EXPECT_EQ(request->control, ControlInterface::kCpuDriver);
    EXPECT_EQ(request->control_argument, power_level);
    EXPECT_EQ(request->domain_id, domain->id());
    EXPECT_EQ(request->options, 0u);
    EXPECT_EQ(request->target_id, domain->id());

    // Simulate the controller acking the transition. Failing to ack the transition will cause this
    // loop to get stuck in FakePowerLevelController::Wait if the requested transition matches the
    // current power level (i.e. kMaxPowerLevel set above), since redundant requests are dropped.
    ASSERT_OK(scheduler.UpdateActivePowerLevel(power_level).status_value());

    controller->reset_request();
  }

  // Requesting the same power level as the current power level should be ignored.
  const size_t count_before = controller->count();
  scheduler.RequestPowerLevelForTesting(kMaxPowerLevel);
  Thread::Current::SleepRelative(ZX_MSEC(100));
  EXPECT_EQ(controller->count(), count_before);

  END_TEST;
}

bool SchedulerElidesPendingControlRequests() {
  BEGIN_TEST;

  // Use the scheduler for the current CPU to test the power level control functionality. It doesn't
  // matter if the test thread remains on the same CPU, since power level requests can be initiated
  // from a different CPU than the target CPU.
  Scheduler& scheduler = percpu::GetCurrent().scheduler;

  zx::result energy_model = EnergyModel::Create(kPowerLevels, kTransitions);
  ASSERT_TRUE(energy_model.is_ok());

  fbl::AllocChecker ac;
  fbl::RefPtr controller = fbl::MakeRefCountedChecked<FakePowerLevelController>(&ac);
  ASSERT_TRUE(ac.check());

  auto domain = fbl::MakeRefCountedChecked<PowerDomain>(
      &ac, 1, cpu_num_to_mask(scheduler.this_cpu()), ktl::move(*energy_model), controller);
  ASSERT_TRUE(ac.check());

  // Set the current power domain, saving the previous domain to restore at the end of the test.
  const ktl::optional<uint8_t> restore_power_level = scheduler.GetActivePowerLevel();
  auto restore_domain = fit::defer([&, previous_domain_set = scheduler.ExchangePowerDomainSet(
                                           PowerDomainSet::CreateForTest(domain))] {
    scheduler.ExchangePowerDomainSet(previous_domain_set);
    if (restore_power_level.has_value()) {
      DEBUG_ASSERT(scheduler.UpdateActivePowerLevel(*restore_power_level).is_ok());
    }
  });
  ASSERT_EQ(domain.get(), scheduler.GetPowerDomainForTesting().get());
  ASSERT_OK(scheduler.UpdateActivePowerLevel(kMaxPowerLevel).status_value());

  for (size_t i = 0; i < 150; ++i) {
    scheduler.RequestPowerLevelForTesting(kHighPowerLevel);
  }

  while (controller->count() == 0) {
    Thread::Current::SleepRelative(ZX_MSEC(1));
  }
  ASSERT_GE(controller->count(), 1u);

  const ktl::optional<PowerLevelUpdateRequest> request = controller->request();
  ASSERT_TRUE(request);
  EXPECT_EQ(request->control, ControlInterface::kCpuDriver);
  EXPECT_EQ(request->control_argument, kHighPowerLevel);
  EXPECT_EQ(request->domain_id, domain->id());
  EXPECT_EQ(request->options, 0u);
  EXPECT_EQ(request->target_id, domain->id());

  END_TEST;
}

bool SchedulerCanPendControlRequestsInIrqContext() {
  BEGIN_TEST;
  // Use the scheduler for the current CPU to test the power level control functionality. It doesn't
  // matter if the test thread remains on the same CPU, since power level requests can be initiated
  // from a different CPU than the target CPU.
  Scheduler& scheduler = percpu::GetCurrent().scheduler;

  zx::result energy_model = EnergyModel::Create(kPowerLevels, kTransitions);
  ASSERT_TRUE(energy_model.is_ok());

  fbl::AllocChecker ac;
  fbl::RefPtr controller = fbl::MakeRefCountedChecked<FakePowerLevelController>(&ac);
  ASSERT_TRUE(ac.check());

  auto domain = fbl::MakeRefCountedChecked<PowerDomain>(
      &ac, 1, cpu_num_to_mask(scheduler.this_cpu()), ktl::move(*energy_model), controller);
  ASSERT_TRUE(ac.check());

  // Set the current power domain, saving the previous domain to restore at the end of the test.
  const ktl::optional<uint8_t> restore_power_level = scheduler.GetActivePowerLevel();
  auto restore_domain = fit::defer([&, previous_domain_set = scheduler.ExchangePowerDomainSet(
                                           PowerDomainSet::CreateForTest(domain))] {
    scheduler.ExchangePowerDomainSet(previous_domain_set);
    if (restore_power_level.has_value()) {
      DEBUG_ASSERT(scheduler.UpdateActivePowerLevel(*restore_power_level).is_ok());
    }
  });
  ASSERT_EQ(domain.get(), scheduler.GetPowerDomainForTesting().get());
  ASSERT_OK(scheduler.UpdateActivePowerLevel(kMaxPowerLevel).status_value());

  auto timer_handler = +[](Timer* timer, zx_instant_mono_t now, void* arg) {
    Scheduler* scheduler = static_cast<Scheduler*>(arg);
    scheduler->RequestPowerLevelForTesting(kHighPowerLevel);
  };

  Timer timer;
  timer.Set(Deadline::infinite_past(), timer_handler, &scheduler);

  while (controller->count() == 0) {
    Thread::Current::SleepRelative(ZX_MSEC(1));
  }
  ASSERT_EQ(controller->count(), 1u);

  const ktl::optional<PowerLevelUpdateRequest> request = controller->request();
  ASSERT_TRUE(request);
  EXPECT_EQ(request->control, ControlInterface::kCpuDriver);
  EXPECT_EQ(request->control_argument, kHighPowerLevel);
  EXPECT_EQ(request->domain_id, domain->id());
  EXPECT_EQ(request->options, 0u);
  EXPECT_EQ(request->target_id, domain->id());

  END_TEST;
}

bool SchedulerCanPendControlRequestsAcrossCpus() {
  BEGIN_TEST;

  if (arch_max_num_cpus() < 2) {
    printf("Skipping test that requires more than one CPU.\n");
    END_TEST;
  }

  // Use the scheduler for the current CPU to test the power level control functionality. It doesn't
  // matter if the test thread remains on the same CPU, since power level requests can be initiated
  // from a different CPU than the target CPU.
  Scheduler& scheduler = percpu::GetCurrent().scheduler;

  zx::result energy_model = EnergyModel::Create(kPowerLevels, kTransitions);
  ASSERT_TRUE(energy_model.is_ok());

  fbl::AllocChecker ac;
  fbl::RefPtr controller = fbl::MakeRefCountedChecked<FakePowerLevelController>(&ac);
  ASSERT_TRUE(ac.check());

  auto domain = fbl::MakeRefCountedChecked<PowerDomain>(
      &ac, 1, cpu_num_to_mask(scheduler.this_cpu()), ktl::move(*energy_model), controller);
  ASSERT_TRUE(ac.check());

  // Set the current power domain, saving the previous domain to restore at the end of the test.
  const ktl::optional<uint8_t> restore_power_level = scheduler.GetActivePowerLevel();
  auto restore_domain = fit::defer([&, previous_domain_set = scheduler.ExchangePowerDomainSet(
                                           PowerDomainSet::CreateForTest(domain))] {
    scheduler.ExchangePowerDomainSet(previous_domain_set);
    if (restore_power_level.has_value()) {
      DEBUG_ASSERT(scheduler.UpdateActivePowerLevel(*restore_power_level).is_ok());
    }
  });
  ASSERT_EQ(domain.get(), scheduler.GetPowerDomainForTesting().get());
  ASSERT_OK(scheduler.UpdateActivePowerLevel(kMaxPowerLevel).status_value());

  // Move the test thread to a different CPU than the target scheduler serves.
  const cpu_mask_t temporary_affinity =
      Scheduler::PeekActiveMask() & ~cpu_num_to_mask(scheduler.this_cpu());
  auto restore_affinity =
      fit::defer([previous_affinity = Thread::Current::Get()->SetCpuAffinity(temporary_affinity)] {
        Thread::Current::Get()->SetCpuAffinity(previous_affinity);
      });
  ASSERT_NE(scheduler.this_cpu(), arch_curr_cpu_num());

  scheduler.RequestPowerLevelForTesting(kHighPowerLevel);

  while (controller->count() == 0) {
    Thread::Current::SleepRelative(ZX_MSEC(1));
  }
  ASSERT_EQ(controller->count(), 1u);

  const ktl::optional<PowerLevelUpdateRequest> request = controller->request();
  ASSERT_TRUE(request);
  EXPECT_EQ(request->control, ControlInterface::kCpuDriver);
  EXPECT_EQ(request->control_argument, kHighPowerLevel);
  EXPECT_EQ(request->domain_id, domain->id());
  EXPECT_EQ(request->options, 0u);
  EXPECT_EQ(request->target_id, domain->id());

  END_TEST;
}

ktl::optional<uint32_t> g_mock_opp_set_domain;
ktl::optional<uint64_t> g_mock_opp_set_opp;
zx_status_t g_mock_opp_set_status;
ktl::optional<uint32_t> g_mock_opp_get_domain;
zx_status_t g_mock_opp_get_status;
uint64_t g_mock_opp_get_value;
zx_status_t g_mock_opp_get_domain_count_status;
size_t g_mock_opp_get_domain_count_value;

const pdev_power_ops kMockPowerOps = {
    .reboot = [](power_reboot_flags flags) -> zx_status_t { return ZX_OK; },
    .shutdown = []() -> zx_status_t { return ZX_OK; },
    .cpu_off = []() -> zx_status_t { return ZX_OK; },
    .cpu_on = [](uint64_t mpid, paddr_t entry, uint64_t context) -> zx_status_t { return ZX_OK; },
    .get_cpu_state = [](uint64_t hw_cpu_id, power_cpu_state* out_state) -> zx_status_t {
      return ZX_ERR_NOT_SUPPORTED;
    },
    .opp_set = [](uint32_t domain_id, uint64_t opp) -> zx_status_t {
      g_mock_opp_set_domain = domain_id;
      g_mock_opp_set_opp = opp;
      return g_mock_opp_set_status;
    },
    .opp_get = [](uint32_t domain_id, uint64_t* out_opp) -> zx_status_t {
      g_mock_opp_get_domain = domain_id;
      if (g_mock_opp_get_status == ZX_OK && out_opp) {
        *out_opp = g_mock_opp_get_value;
      }
      return g_mock_opp_get_status;
    },
    .opp_get_domain_count = [](size_t* out_count) -> zx_status_t {
      if (g_mock_opp_get_domain_count_status == ZX_OK && out_count) {
        *out_count = g_mock_opp_get_domain_count_value;
      }
      return g_mock_opp_get_domain_count_status;
    },
};

void ResetMockPowerOps() {
  g_mock_opp_set_domain = ktl::nullopt;
  g_mock_opp_set_opp = ktl::nullopt;
  g_mock_opp_set_status = ZX_OK;
  g_mock_opp_get_domain = ktl::nullopt;
  g_mock_opp_get_status = ZX_OK;
  g_mock_opp_get_value = 0;
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 0;
}

struct AutoMockPowerOps {
  AutoMockPowerOps() {
    ResetMockPowerOps();
    original_ops_ = pdev_swap_power_for_test(&kMockPowerOps);
  }
  ~AutoMockPowerOps() { pdev_swap_power_for_test(original_ops_); }

 private:
  const pdev_power_ops* original_ops_;
};

bool PDevPowerLevelControllerIsSupported() {
  BEGIN_TEST;

  AutoMockPowerOps mock;

  // The controller is not supported if retrieving the domain count fails.
  g_mock_opp_get_domain_count_status = ZX_ERR_NOT_SUPPORTED;
  EXPECT_FALSE(PDevPowerLevelController::IsSupported());

  // The controller is not supported if the domain count is 0.
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 0;
  EXPECT_FALSE(PDevPowerLevelController::IsSupported());

  // The controller is supported if the domain count is greater than 0.
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 2;
  EXPECT_TRUE(PDevPowerLevelController::IsSupported());

  END_TEST;
}

bool PDevPowerLevelControllerPostValidation() {
  BEGIN_TEST;

  // Save and clear the power domain set for all CPUs to prevent scheduler interference
  // and ensure consistent test behavior across architectures (e.g. even if the test
  // thread migrates to another CPU).
  struct RestorePowerDomains {
    PowerDomainSet previous_sets[SMP_MAX_CPUS];
    ~RestorePowerDomains() {
      percpu::ForEach([this](cpu_num_t cpu_num, percpu* percpu) {
        percpu->scheduler.ExchangePowerDomainSet(previous_sets[cpu_num]);
      });
    }
  } restore_state;

  percpu::ForEach([&restore_state](cpu_num_t cpu_num, percpu* percpu) {
    restore_state.previous_sets[cpu_num] =
        percpu->scheduler.ExchangePowerDomainSet(PowerDomainSet{});
  });

  PDevPowerLevelController::ResetForTest();

  AutoMockPowerOps mock;
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 2;

  zx::result<fbl::RefPtr<PDevPowerLevelController>> controller_result =
      PDevPowerLevelController::Get(0);
  ASSERT_TRUE(controller_result.is_ok());
  fbl::RefPtr controller = ktl::move(controller_result.value());

  // Validate that interface requests not matching kCpuDriver are rejected.
  PowerLevelUpdateRequest request_wfi = {
      .domain_id = 0,
      .target_id = 0,
      .control = ControlInterface::kArmWfi,
      .control_argument = 1,
      .options = 0,
  };
  const zx::result<uint32_t> post_wfi_result = controller->Post(request_wfi);
  EXPECT_EQ(post_wfi_result.error_value(), ZX_ERR_NOT_SUPPORTED);

  // Validate that domain_id is checked against the total domain count
  // (domain_id 2 >= count 2).
  PowerLevelUpdateRequest request_oob = {
      .domain_id = 2,
      .target_id = 2,
      .control = ControlInterface::kCpuDriver,
      .control_argument = 1,
      .options = 0,
  };
  const zx::result<uint32_t> post_oob_result = controller->Post(request_oob);
  EXPECT_EQ(post_oob_result.error_value(), ZX_ERR_OUT_OF_RANGE);

  // Validate that errors from power_opp_get_domain_count are correctly
  // propagated on creation.
  PDevPowerLevelController::ResetForTest();
  g_mock_opp_get_domain_count_status = ZX_ERR_BAD_STATE;
  const zx::result<fbl::RefPtr<PDevPowerLevelController>> get_error_result =
      PDevPowerLevelController::Get(0);
  EXPECT_EQ(get_error_result.error_value(), ZX_ERR_BAD_STATE);

  // Validate that domain_id is checked against domain count during Get().
  PDevPowerLevelController::ResetForTest();
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 2;
  const zx::result<fbl::RefPtr<PDevPowerLevelController>> get_oob_result =
      PDevPowerLevelController::Get(2);
  EXPECT_EQ(get_oob_result.error_value(), ZX_ERR_OUT_OF_RANGE);

  // Re-acquire the controller for the valid POST test.
  PDevPowerLevelController::ResetForTest();
  controller_result = PDevPowerLevelController::Get(0);
  ASSERT_TRUE(controller_result.is_ok());
  controller = ktl::move(controller_result.value());

  // Validate that a request passing all checks is allowed to proceed to the
  // registry update. Since domain 0 is not registered in this test registry,
  // UpdatePowerLevel returns ZX_ERR_NOT_FOUND, which verifies that all earlier
  // validation checks successfully passed.
  PowerLevelUpdateRequest request_valid = {
      .domain_id = 0,
      .target_id = 0,
      .control = ControlInterface::kCpuDriver,
      .control_argument = 1,
      .options = 0,
  };
  g_mock_opp_set_status = ZX_OK;
  const zx::result<uint32_t> post_valid_result = controller->Post(request_valid);
  EXPECT_EQ(post_valid_result.error_value(), ZX_ERR_NOT_FOUND);

  END_TEST;
}

bool PDevPowerLevelControllerGetPowerLevelValidation() {
  BEGIN_TEST;

  PDevPowerLevelController::ResetForTest();

  AutoMockPowerOps mock;
  g_mock_opp_get_domain_count_status = ZX_OK;
  g_mock_opp_get_domain_count_value = 2;

  zx::result<fbl::RefPtr<PDevPowerLevelController>> controller_result =
      PDevPowerLevelController::Get(0);
  ASSERT_TRUE(controller_result.is_ok());
  fbl::RefPtr controller = ktl::move(controller_result.value());

  // Validate that domain_id is checked against the total domain count
  // (domain_id 2 >= count 2).
  const zx::result<uint64_t> current_level_oob_result = controller->GetCurrentPowerLevel(2);
  EXPECT_EQ(current_level_oob_result.error_value(), ZX_ERR_OUT_OF_RANGE);

  // Validate that a valid domain query returns the value fetched from the pdev
  // backend.
  g_mock_opp_get_status = ZX_OK;
  g_mock_opp_get_value = 42;
  zx::result<uint64_t> current_power_level_result = controller->GetCurrentPowerLevel(0);
  ASSERT_TRUE(current_power_level_result.is_ok());
  ASSERT_TRUE(g_mock_opp_get_domain.has_value());
  EXPECT_EQ(current_power_level_result.value(), uint64_t{42});
  EXPECT_EQ(g_mock_opp_get_domain.value(), 0u);

  END_TEST;
}

UNITTEST_START_TESTCASE(pm_controller)
UNITTEST("Scheduler flushes pending requests to the controller.",
         SchedulerFlushesPendingControlRequests)
UNITTEST("Scheduler elides multiple pending requests to the controller.",
         SchedulerElidesPendingControlRequests)
UNITTEST("Scheduler control requests may occur in IRQ context.",
         SchedulerCanPendControlRequestsInIrqContext)
UNITTEST("Scheduler control requests may pend from a different CPU.",
         SchedulerCanPendControlRequestsAcrossCpus)
UNITTEST("PDev controller supported.", PDevPowerLevelControllerIsSupported)
UNITTEST("PDev controller post validation.", PDevPowerLevelControllerPostValidation)
UNITTEST("PDev controller get power level validation.",
         PDevPowerLevelControllerGetPowerLevelValidation)
UNITTEST_END_TESTCASE(pm_controller, "pm_controller", "Kernel CPU power level controller tests.")

}  // namespace
