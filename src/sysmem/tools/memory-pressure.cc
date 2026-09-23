// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "memory-pressure.h"

#include <fidl/fuchsia.sysmem/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <stdio.h>
#include <stdlib.h>

#include <string>

#include "src/lib/fsl/handles/object_info.h"
#include "src/lib/fxl/command_line.h"

void PrintHelp() {
  Log(""
      "Usage: sysmem-memory-pressure [--contiguous] [--help] [--heap=heap] [--usage=[cpu|vulkan]] "
      "size_bytes\n");
  Log("Options:\n");
  Log(" --help           Show this message.\n");
  Log(" --contiguous     Request physically-contiguous memory\n");
  Log(" --heap           Specifies the numeric value of the sysmem heap to request memory from. By "
      "default system ram is used.\n");
  Log(" --usage          Specifies what usage should be requested from sysmem. Vulkan is the "
      "default\n");
  Log(" size_bytes       The size of the memory in bytes.\n");
}

int MemoryPressureCommand(const fxl::CommandLine& command_line, bool sleep) {
  if (command_line.HasOption("help")) {
    PrintHelp();
    return 0;
  }

  if (command_line.positional_args().size() != 1) {
    LogError("Missing size to allocate\n");
    PrintHelp();
    return 1;
  }

  std::string size_string = command_line.positional_args()[0];
  char* endptr;
  uint64_t size = strtoull(size_string.c_str(), &endptr, 0);
  if (endptr != size_string.c_str() + size_string.size()) {
    LogError("Invalid size %s\n", size_string.c_str());
    PrintHelp();
    return 1;
  }

  fuchsia_sysmem::HeapType heap = fuchsia_sysmem::HeapType::kSystemRam;
  std::string heap_string;
  if (command_line.GetOptionValue("heap", &heap_string)) {
    char* endptr;
    heap = static_cast<fuchsia_sysmem::HeapType>(strtoull(heap_string.c_str(), &endptr, 0));
    if (endptr != heap_string.c_str() + heap_string.size()) {
      LogError("Invalid heap string: %s\n", heap_string.c_str());
      return 1;
    }
  }
  auto heap_type_result = sysmem::V2CopyFromV1HeapType(heap);
  ZX_ASSERT(heap_type_result.is_ok());
  auto heap_type = std::move(heap_type_result.value());

  bool physically_contiguous = command_line.HasOption("contiguous");

  fuchsia_sysmem2::BufferCollectionConstraints constraints;
  std::string usage;
  constraints.usage().emplace();
  if (command_line.GetOptionValue("usage", &usage)) {
    if (usage == "vulkan") {
      constraints.usage()->vulkan() = fuchsia_sysmem2::kVulkanImageUsageTransferDst;
    } else if (usage == "cpu") {
      constraints.usage()->cpu() = fuchsia_sysmem2::kCpuUsageRead;
    } else {
      LogError("Invalid usage %s\n", usage.c_str());
      PrintHelp();
      return 1;
    }
  } else {
    constraints.usage()->vulkan() = fuchsia_sysmem2::kVulkanImageUsageTransferDst;
  }
  constraints.min_buffer_count_for_camping() = 1;
  auto& mem_constraints = constraints.buffer_memory_constraints().emplace();
  mem_constraints.physically_contiguous_required() = physically_contiguous;
  mem_constraints.min_size_bytes() = static_cast<uint32_t>(size);
  mem_constraints.cpu_domain_supported() = true;
  mem_constraints.ram_domain_supported() = true;
  mem_constraints.inaccessible_domain_supported() = true;
  mem_constraints.permitted_heaps().emplace().emplace_back(sysmem::MakeHeap(heap_type, 0));
  zx::result client_end = component::Connect<fuchsia_sysmem2::Allocator>();
  if (client_end.is_error()) {
    LogError("Failed to connect to sysmem services, error %d\n", client_end.status_value());
    return 1;
  }
  fidl::SyncClient sysmem_allocator{std::move(client_end.value())};
  fuchsia_sysmem2::AllocatorSetDebugClientInfoRequest set_debug_request;
  set_debug_request.name() = fsl::GetCurrentProcessName();
  set_debug_request.id() = fsl::GetCurrentProcessKoid();
  if (const auto set_debug_result =
          sysmem_allocator->SetDebugClientInfo(std::move(set_debug_request));
      !set_debug_result.is_ok()) {
    LogError("Failed to set debug client info - error: %s\n",
             set_debug_result.error_value().status_string());
    return 1;
  };

  auto [client_collection_channel, server_collection] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fuchsia_sysmem2::AllocatorAllocateNonSharedCollectionRequest allocate_non_shared_request;
  allocate_non_shared_request.collection_request(std::move(server_collection));
  if (const auto allocate_non_shared_result =
          sysmem_allocator->AllocateNonSharedCollection(std::move(allocate_non_shared_request));
      !allocate_non_shared_result.is_ok()) {
    LogError("Failed to allocate collection - error: %s\n",
             allocate_non_shared_result.error_value().status_string());
    return 1;
  }
  fidl::SyncClient collection(std::move(client_collection_channel));

  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority() = 1000000;
  set_name_request.name() = "sysmem-memory-pressure";
  if (const auto set_name_result = collection->SetName(std::move(set_name_request));
      !set_name_result.is_ok()) {
    LogError("Failed to set collection name - error: %s\n",
             set_name_result.error_value().status_string());
    return 1;
  }

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints() = std::move(constraints);
  if (const auto set_constraits_result =
          collection->SetConstraints(std::move(set_constraints_request));
      !set_constraits_result.is_ok()) {
    LogError("Failed to set collection constraints - error: %s\n",
             set_constraits_result.error_value().status_string());
    return 1;
  }

  auto wait_result = collection->WaitForAllBuffersAllocated();
  if (!wait_result.is_ok()) {
    if (wait_result.error_value().is_framework_error()) {
      LogError("Lost connection to sysmem services - framework_error: %s\n",
               wait_result.error_value().framework_error().status_string());
    } else {
      LogError("Allocation error %u\n",
               static_cast<uint32_t>(wait_result.error_value().domain_error()));
    }
    return 1;
  }
  Log("Allocated %ld bytes. Sleeping forever\n", size);

  if (sleep) {
    zx::nanosleep(zx::time::infinite());
  }

  return 0;
}
