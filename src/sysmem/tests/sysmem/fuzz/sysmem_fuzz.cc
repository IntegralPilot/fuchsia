// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.sysmem/cpp/fidl.h>

#include "src/graphics/display/lib/fake-display-stack/fake-sysmem-device-hierarchy.h"
#include "src/sysmem/server/sysmem.h"
#include "sysmem_fuzz_common.h"

#define DBGRTN 0

#define LOGRTN(status, ...)           \
  {                                   \
    if (status != ZX_OK) {            \
      if (DBGRTN) {                   \
        fprintf(stderr, __VA_ARGS__); \
        fflush(stderr);               \
      }                               \
      return 0;                       \
    }                                 \
  }
#define LOGRTNC(condition, ...)       \
  {                                   \
    if ((condition)) {                \
      if (DBGRTN) {                   \
        fprintf(stderr, __VA_ARGS__); \
        fflush(stderr);               \
      }                               \
      return 0;                       \
    }                                 \
  }

extern "C" int LLVMFuzzerTestOneInput(uint8_t* data, size_t size) {
  const size_t kRequiredFuzzingBytes = sizeof(fuchsia_sysmem::wire::BufferCollectionConstraints);

  LOGRTNC(size != kRequiredFuzzingBytes, "size: %zu != kRequiredFuzzingBytes: %zu\n", size,
          kRequiredFuzzingBytes);
  auto inproc_sysmem = display::FakeSysmemDeviceHierarchy::Create();

  auto allocator_client_result = inproc_sysmem->ConnectAllocator();
  ZX_ASSERT(allocator_client_result.is_ok());
  auto allocator_client = std::move(allocator_client_result.value());

  fidl::WireSyncClient<fuchsia_sysmem::Allocator> allocator(std::move(allocator_client));

  auto [token_client_end, token_server_end] =
      fidl::Endpoints<fuchsia_sysmem::BufferCollectionToken>::Create();

  auto allocate_result = allocator->AllocateSharedCollection(std::move(token_server_end));
  LOGRTN(allocate_result.status(), "Failed to allocate shared collection.\n");

  auto [collection_client_end, collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem::BufferCollection>::Create();

  auto bind_result = allocator->BindSharedCollection(std::move(token_client_end),
                                                     std::move(collection_server_end));
  LOGRTN(bind_result.status(), "Failed to bind shared collection.\n");

  fuchsia_sysmem::wire::BufferCollectionConstraints constraints;
  memcpy(&constraints, data, kRequiredFuzzingBytes);

  fidl::WireSyncClient<fuchsia_sysmem::BufferCollection> collection(
      std::move(collection_client_end));
  auto set_constraints_result = collection->SetConstraints(true, constraints);
  LOGRTN(set_constraints_result.status(), "Failed to set buffer collection constraints.\n");

  fidl::WireResult result = collection->WaitForBuffersAllocated();
  // This is the first round-trip to/from sysmem.  A failure here can be
  // due to any step above failing async.
  LOGRTN(result.status(), "Failed on WaitForBuffersAllocated.\n");
  LOGRTN(result.value().status, "Bad allocation_status on WaitForBuffersAllocated.\n");

  return 0;
}
