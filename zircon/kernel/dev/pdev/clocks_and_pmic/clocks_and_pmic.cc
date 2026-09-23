// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <lib/arch/intrin.h>

#include <dev/clocks_and_pmic.h>
#include <pdev/clocks_and_pmic.h>

namespace {
const struct pdev_clocks_and_pmic_ops default_ops = {
    .prepare_for_suspend = []() -> zx_status_t { return ZX_OK; },
    .wakeup_from_suspend = []() -> zx_status_t { return ZX_OK; },
};

const struct pdev_clocks_and_pmic_ops* clocks_and_pmic_ops = &default_ops;
}  // namespace

void pdev_register_clocks_and_pmic(const struct pdev_clocks_and_pmic_ops* ops) {
  // Note that registration of this interface must happen before the system has
  // brought up the secondary CPUs, so before LK_INIT_LEVEL_PLATFORM.  We'd like
  // to ASSERT that here, but unfortunately, the per-cpu init level of the
  // system is not published in the per-cpu data, merely passed to registered
  // init hooks, so we'll have to settle for a comment instead.
  //
  // Additionally, once a non-default interface has been registered, it may not
  // be changed afterwards.  We can at least assert that.  Do so now.
  ASSERT(clocks_and_pmic_ops == &default_ops);
  clocks_and_pmic_ops = ops;
}

zx_status_t clocks_and_pmic_prepare_for_suspend() {
  if (clocks_and_pmic_ops && clocks_and_pmic_ops->prepare_for_suspend) {
    return clocks_and_pmic_ops->prepare_for_suspend();
  }

  return ZX_ERR_NOT_SUPPORTED;
}

zx_status_t clocks_and_pmic_wakeup_from_suspend() {
  if (clocks_and_pmic_ops && clocks_and_pmic_ops->wakeup_from_suspend) {
    return clocks_and_pmic_ops->wakeup_from_suspend();
  }

  return ZX_ERR_NOT_SUPPORTED;
}
