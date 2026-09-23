// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_FAKE_BTI_CPP_FAKE_BTI_H_
#define LIB_DRIVER_FAKE_BTI_CPP_FAKE_BTI_H_

#include <lib/stdcompat/span.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <limits.h>
#include <zircon/compiler.h>
#include <zircon/limits.h>
#include <zircon/types.h>

// API calibration.

namespace fake_bti {

// All physical addresses returned by zx_bti_pin with a fake BTI will be set to this value.
// ZX_MAX_PAGE_SIZE is chosen so that so superficial validity checks like "is the address correctly
// aligned" and "is the address non-zero" in the code under test will pass.
#define FAKE_BTI_PHYS_ADDR ZX_MAX_PAGE_SIZE

// Creates a fake BTI object.
zx::result<zx::bti> CreateFakeBti();

// Create a fake BTI object with the fake physical addresses from |paddrs| in its zx_bti_pin.
zx::result<zx::bti> CreateFakeBtiWithPaddrs(cpp20::span<const zx_paddr_t> paddrs);

// Updates the pinned paddrs for `bti` (which must have been previously created via
// [`CreateFakeBti`] or its siblings).
zx::result<> SetPaddrs(zx::unowned_bti bti, cpp20::span<const zx_paddr_t> paddrs);

// Contains the information for a pinned VMO in a fake BTI object.
struct FakeBtiPinnedVmoInfo {
  zx::vmo vmo;
  uint64_t size;
  uint64_t offset;
};

// Fake BTI stores all pinned VMOs for testing purposes. Tests can call this method to get
// duplicates of all pinned VMO handles, as well as the pinned pages size and offset for each VMO.
// It's the caller's responsibility to close all the returned VMO handles.
zx::result<std::vector<FakeBtiPinnedVmoInfo>> GetPinnedVmo(zx::unowned_bti bti);

// Fake BTI stores all the fake physical addresses that is returned by |zx_bti_pin|.
// Tests can call this method to get the fake physical addresses corresponding to |vmo_info|.
zx::result<std::vector<zx_paddr_t>> GetVmoPhysAddress(zx::unowned_bti bti,
                                                      FakeBtiPinnedVmoInfo& vmo_info);

}  // namespace fake_bti

#endif  // LIB_DRIVER_FAKE_BTI_CPP_FAKE_BTI_H_
