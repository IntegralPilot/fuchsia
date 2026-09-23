// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Ported from zircon/kernel/dev/pdev/power/power.cc

#include <zircon/errors.h>
#include <zircon/types.h>

#include <dev/power.h>
#include <kernel/ffi.h>
#include <pdev/power.h>

extern "C" {

void rust_power_reboot(power_reboot_flags flags);
void rust_power_shutdown();
zx_status_t rust_power_cpu_off();
zx_status_t rust_power_cpu_on(uint64_t hw_cpu_id, paddr_t entry, uint64_t context);
zx_status_t rust_power_get_cpu_state(uint64_t hw_cpu_id, power_cpu_state* out_state);
zx_status_t rust_power_opp_set(uint32_t domain_id, uint64_t opp);
zx_status_t rust_power_opp_get(uint32_t domain_id, uint64_t* out_opp);
zx_status_t rust_power_opp_get_domain_count(size_t* out_count);

void rust_pdev_register_power(const pdev_power_ops* ops);
const pdev_power_ops* rust_pdev_swap_power_for_test(const pdev_power_ops* ops);

}  // extern "C"

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void power_reboot(power_reboot_flags flags) { rust_power_reboot(flags); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void power_shutdown() { rust_power_shutdown(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t power_cpu_off() { return rust_power_cpu_off(); }

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t power_cpu_on(uint64_t hw_cpu_id, paddr_t entry, uint64_t context) {
  return rust_power_cpu_on(hw_cpu_id, entry, context);
}

zx::result<power_cpu_state> power_get_cpu_state(uint64_t hw_cpu_id) {
  power_cpu_state state = power_cpu_state::OFF;
  zx_status_t status = rust_power_get_cpu_state(hw_cpu_id, &state);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(state);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE zx_status_t power_opp_set(uint32_t domain_id, uint64_t opp) {
  return rust_power_opp_set(domain_id, opp);
}

zx::result<uint64_t> power_opp_get(uint32_t domain_id) {
  uint64_t opp = 0;
  zx_status_t status = rust_power_opp_get(domain_id, &opp);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(opp);
}

zx::result<size_t> power_opp_get_domain_count() {
  size_t count = 0;
  zx_status_t status = rust_power_opp_get_domain_count(&count);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(count);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE void pdev_register_power(const pdev_power_ops* ops) {
  rust_pdev_register_power(ops);
}

// TODO(https://fxbug.dev/537458631): Remove the annotations once cross-language inlining works.
FFI_ALWAYS_INLINE const pdev_power_ops* pdev_swap_power_for_test(const pdev_power_ops* ops) {
  return rust_pdev_swap_power_for_test(ops);
}
