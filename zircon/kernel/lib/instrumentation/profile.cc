// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <align.h>
#include <lib/instrumentation/debugdata.h>
#include <lib/instrumentation/kernel-mapped-vmo.h>
#include <lib/llvm-profdata/llvm-profdata.h>
#include <lib/page/size.h>
#include <stdint.h>
#include <string.h>
#include <zircon/assert.h>

#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <ktl/utility.h>
#include <phys/boot-constants.h>
#include <vm/vm_object_paged.h>

#include "private.h"

#include <ktl/enforce.h>

namespace {

constexpr ktl::string_view kSink = "llvm-profile";
constexpr ktl::string_view kModule = "vmzircon";
constexpr ktl::string_view kSufix = "profraw";

// This holds the pinned mapping of the live-updated data.
KernelMappedVmo gProfdataLiveData;

}  // namespace

InstrumentationDataVmo LlvmProfdataVmo() {
  LlvmProfdata profdata;
  profdata.Init(kBootConstants.elf_build_id.get());
  if (profdata.size_bytes() == 0) {
    return {};
  }

  // Create a VMO to hold the whole profdata dump.
  fbl::RefPtr<VmObjectPaged> vmo;
  uint64_t aligned_size;
  zx_status_t status = VmObject::RoundSize(profdata.size_bytes(), &aligned_size);
  ZX_ASSERT(status == ZX_OK);
  status = VmObjectPaged::Create(PMM_ALLOC_FLAG_ANY, 0, aligned_size, &vmo);
  ZX_ASSERT(status == ZX_OK);

  // First fill in just the fixed data, by mapping the whole VMO into the
  // kernel address space.  Then let the mapping and pinning be cleaned up,
  // since we don't need the whole thing mapped into the kernel at runtime.
  {
    KernelMappedVmo mapped_vmo;
    status = mapped_vmo.Init(vmo, 0, profdata.size_bytes(), "llvm-profdata-setup");
    ZX_ASSERT(status == ZX_OK);
    ktl::span<ktl::byte> mapped_data{
        reinterpret_cast<ktl::byte*>(mapped_vmo.base_locking()),
        mapped_vmo.size_locking(),
    };
    profdata.WriteFixedData(mapped_data);
  }

  // Now map in just the pages holding the live data.  This mapping will be
  // kept alive permanently so the live data can be updated through it.
  const LlvmProfdata::LiveDataBounds bounds = profdata.bounds();
  const uint64_t map_offset = RoundDownPageSize(bounds.offset);
  const size_t map_size = RoundUpPageSize(bounds.offset + bounds.size) - map_offset;

  status = gProfdataLiveData.Init(ktl::move(vmo), map_offset, map_size, "llvm-profdata-live-data");
  ZX_ASSERT(status == ZX_OK);

  LlvmProfdata::LiveData live_data = profdata.LiveDataForBounds({
      reinterpret_cast<ktl::byte*>(bounds.offset - map_offset + gProfdataLiveData.base_locking()),
      bounds.size,
  });

  // Live data up to this point have collected in global variable space.
  // Copy those data into the mapped VMO data.
  profdata.CopyLiveData(live_data);

  // Switch instrumented code over to updating the mapped VMO data in place.
  // From this point on, the kernel's VMO mapping is used by all instrumented
  // code and must be kept valid and pinned.
  //
  // TODO(mcgrathr): We could theoretically decommit the global data pages
  // after this to recover that RAM.  That part of the kernel's global data
  // area should never be accessed again.
  LlvmProfdata::UseLiveData(live_data);

  auto vmo_name = instrumentation::DebugdataVmoName(kSink, kModule, kSufix, /*is_static=*/false);

  return {
      .announce = LlvmProfdata::kAnnounce,
      .sink_name = LlvmProfdata::kDataSinkName,
      .handle = gProfdataLiveData.Publish(ktl::string_view(vmo_name.data(), vmo_name.size()),
                                          profdata.size_bytes()),
  };
}
