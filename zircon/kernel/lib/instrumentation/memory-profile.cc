// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
#include <lib/heap.h>
#include <lib/instrumentation/debugdata.h>

#include <ktl/string_view.h>
#include <vm/vm_object_paged.h>

#include "private.h"

#if KERNEL_MEMORY_PROFILER
#include <lib/instrumentation/kernel-mapped-vmo.h>

namespace {
KernelMappedVmo memProfLiveData;
}  // namespace

InstrumentationDataVmo MemoryProfileVmo() {
  constexpr ktl::string_view kSink = "memory-profile";
  constexpr ktl::string_view kModule = "heap";
  constexpr ktl::string_view kSufix = "bin";
  const void *buf_addr = nullptr;
  size_t buf_size = 0;
  get_heap_profile(&buf_addr, &buf_size);

  fbl::RefPtr<VmObjectPaged> vmo;
  zx_status_t status = VmObjectPaged::CreateFromWiredPages(buf_addr, buf_size, false, &vmo);
  ZX_ASSERT(status == ZX_OK);

  status =
      memProfLiveData.Init(ktl::move(vmo), /*map_offset=*/0, buf_size, "memory-profile-live-data");
  ZX_ASSERT(status == ZX_OK);

  auto vmo_name = instrumentation::DebugdataVmoName(kSink, kModule, kSufix, /*is_static=*/false);
  return {
      .announce = "Memory profile",
      .sink_name = kSink,
      .handle =
          memProfLiveData.Publish(ktl::string_view(vmo_name.data(), vmo_name.size()), buf_size),
  };
}
#else

InstrumentationDataVmo MemoryProfileVmo() { return {}; }

#endif
