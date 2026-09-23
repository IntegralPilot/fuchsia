// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/power-management/energy-model.h>
#include <stdio.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/ref_ptr.h>
#include <kernel/cpu.h>
#include <kernel/ffi.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <ktl/bit.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <pdev/power.h>

extern "C" {

void cpp_rppm_dump() {
  const power_management::PowerDomainSet power_domain_set =
      percpu::Get(BOOT_CPU_ID).scheduler.GetPowerDomainSetForTesting();
  power_domain_set.Visit([](const fbl::RefPtr<power_management::PowerDomain>& domain) {
    printf("Power domain %u:\n", domain->id());
    for (size_t index = 0; const auto& power_level : domain->model().levels()) {
      printf("  %3zu. rate=%-8s power=%10" PRIu64 " nw cost=%10" PRIu64 " nw/rate control=%s\n",
             index++, Format(power_level.processing_rate(), ffl::String::Dec, 6).c_str(),
             power_level.power_coefficient_nw(), power_level.power_cost_nw_per_rate(),
             ToString(power_level.control()));
    }
  });

  printf("\n");

  percpu::ForEach([](cpu_num_t cpu, percpu* percpu) {
    Scheduler& scheduler = percpu->scheduler;

    fbl::RefPtr domain = scheduler.GetPowerDomainForTesting();
    ktl::optional<uint8_t> current_power_level = scheduler.GetActivePowerLevel();
    ktl::optional<uint64_t> active_power_coefficient_nw =
        scheduler.GetActivePowerCoefficientNwForTesting();
    ktl::optional<uint64_t> max_idle_power_coefficient_nw =
        scheduler.GetMaxIdlePowerCoefficientNwForTesting();

    printf("CPU %u:\n", cpu);
    printf("  Power domain %d:\n", domain ? static_cast<int>(domain->id()) : -1);
    printf("  Current power level %d.\n", static_cast<int8_t>(current_power_level.value_or(-1)));
    printf("  Active power coefficient %" PRIu64 " nw\n", active_power_coefficient_nw.value_or(0));
    printf("  Max idle power coefficient %" PRIu64 " nw\n",
           max_idle_power_coefficient_nw.value_or(0));
  });
}

zx_status_t cpp_rppm_update_active_power_level(cpu_num_t cpu, uint8_t power_level) {
  if (cpu >= percpu::processor_count()) {
    return ZX_ERR_INVALID_ARGS;
  }
  auto result = percpu::Get(cpu).scheduler.UpdateActivePowerLevel(power_level);
  return result.status_value();
}

bool cpp_rppm_request_power_level_for_testing(cpu_num_t cpu, uint8_t power_level) {
  if (cpu >= percpu::processor_count()) {
    return false;
  }
  return percpu::Get(cpu).scheduler.RequestPowerLevelForTesting(power_level);
}

zx_status_t cpp_rppm_get_active_power_level(cpu_num_t cpu, uint8_t* out_power_level) {
  if (cpu >= percpu::processor_count() || out_power_level == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  auto maybe_level = percpu::Get(cpu).scheduler.GetActivePowerLevel();
  if (!maybe_level.has_value()) {
    return ZX_ERR_NOT_FOUND;
  }
  *out_power_level = maybe_level.value();
  return ZX_OK;
}

zx_status_t cpp_rppm_update_processing_limits(uint64_t cpu_mask_val, uint64_t min_rate,
                                              uint64_t max_rate) {
  const size_t processor_count = percpu::processor_count();
  const uint64_t max_cpu_mask =
      (processor_count >= 64) ? UINT64_MAX : ((uint64_t{1} << processor_count) - 1);
  if (cpu_mask_val > max_cpu_mask) {
    return ZX_ERR_INVALID_ARGS;
  }
  cpu_mask_t cpu_mask = static_cast<cpu_mask_t>(cpu_mask_val);
  const size_t cpu_count = ktl::popcount(cpu_mask);

  fbl::AllocChecker checker;
  fbl::Array<zx_cpu_perf_limit_t> limits = fbl::MakeArray<zx_cpu_perf_limit_t>(&checker, cpu_count);
  if (!checker.check()) {
    return ZX_ERR_NO_MEMORY;
  }

  for (size_t i = 0; i < cpu_count; i++) {
    const cpu_num_t cpu = remove_cpu_from_mask(cpu_mask);
    DEBUG_ASSERT(cpu != INVALID_CPU);
    printf("Setting CPU%u rate limits: min=%" PRIu64 " max=%" PRIu64 "\n", cpu, min_rate, max_rate);
    limits[i] = {
        .logical_cpu_number = cpu,
        .limit_type = ZX_CPU_PERF_LIMIT_TYPE_RATE,
        .min = min_rate,
        .max = max_rate,
    };
  }
  DEBUG_ASSERT(cpu_mask == 0);

  Scheduler::UpdateProcessingLimits(ktl::span{limits.data(), limits.size()});
  return ZX_OK;
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE size_t cpp_rppm_get_processor_count() { return percpu::processor_count(); }

}  // extern "C"
