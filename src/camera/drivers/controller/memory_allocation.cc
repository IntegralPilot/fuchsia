// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/camera/drivers/controller/memory_allocation.h"

#include <fidl/fuchsia.sysmem2/cpp/hlcpp_conversion.h>
#include <lib/ddk/debug.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>
#include <zircon/errors.h>

#include <sstream>

namespace camera {

ControllerMemoryAllocator::ControllerMemoryAllocator(
    fuchsia::sysmem2::AllocatorSyncPtr sysmem_allocator)
    : sysmem_allocator_(std::move(sysmem_allocator)) {}

template <typename T>
static std::string FormatMinMax(const T& v_min, const T& v_max) {
  if (v_min == v_max) {
    return std::to_string(v_min);
  }
  bool lowest_min = v_min == std::numeric_limits<T>::min();
  bool highest_max = v_max == std::numeric_limits<T>::max();
  if (lowest_min && highest_max) {
    return "any";
  }
  if (lowest_min) {
    return "≤ " + std::to_string(v_max);
  }
  if (highest_max) {
    return "≥ " + std::to_string(v_min);
  }
  return std::to_string(v_min) + " - " + std::to_string(v_max);
}

static std::string FormatConstraints(
    const std::vector<fuchsia::sysmem2::BufferCollectionConstraints>& constraints) {
  std::stringstream ss;
  for (uint32_t i = 0; i < constraints.size(); ++i) {
    const auto& element = constraints[i];
    ss << "  [" << i << "] " << element.min_buffer_count_for_camping() << " camping, "
       << element.min_buffer_count() << " total\n";
    if (element.has_image_format_constraints()) {
      for (uint32_t j = 0; j < element.image_format_constraints().size(); ++j) {
        const auto& format = element.image_format_constraints()[j];
        ss << "    [" << j << "] ("
           << FormatMinMax(format.required_min_size().width, format.required_max_size().width)
           << ") x ("
           << FormatMinMax(format.required_min_size().height, format.required_max_size().height)
           << ")\n";
      }
    }
  }
  return ss.str();
}

zx_status_t ControllerMemoryAllocator::AllocateSharedMemory(
    const std::vector<fuchsia::sysmem2::BufferCollectionConstraints>& constraints,
    BufferCollection& out_buffer_collection, const std::string& name) const {
  TRACE_DURATION("camera", "ControllerMemoryAllocator::AllocateSharedMemory");

  zxlogf(INFO, "AllocateSharedMemory (name = %s) with the following constraints:\n%s", name.c_str(),
         FormatConstraints(constraints).c_str());

  auto num_constraints = constraints.size();

  if (!num_constraints) {
    return ZX_ERR_INVALID_ARGS;
  }

  // Create tokens which we'll hold on to to get our buffer_collection.
  std::vector<fuchsia::sysmem2::BufferCollectionTokenSyncPtr> tokens(num_constraints);

  // Start the allocation process.
  fuchsia::sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
  allocate_shared_request.set_token_request(tokens[0].NewRequest());
  auto status = sysmem_allocator_->AllocateSharedCollection(std::move(allocate_shared_request));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create token");
    return status;
  }

  // Duplicate the tokens.
  for (uint32_t i = 1; i < num_constraints; i++) {
    fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
    dup_request.set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);
    dup_request.set_token_request(tokens[i].NewRequest());
    status = tokens[0]->Duplicate(std::move(dup_request));
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to duplicate token");
      return status;
    }
  }

  // Now convert into a Logical BufferCollection:
  std::vector<fuchsia::sysmem2::BufferCollectionSyncPtr> buffer_collections(num_constraints);

  fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.set_token(std::move(tokens[0]));
  bind_shared_request.set_buffer_collection_request(buffer_collections[0].NewRequest());
  status = sysmem_allocator_->BindSharedCollection(std::move(bind_shared_request));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create logical buffer collection");
    return status;
  }

  fuchsia::sysmem2::Node_Sync_Result sync_result;
  status = buffer_collections[0]->Sync(&sync_result);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to sync");
    return status;
  }

  constexpr uint32_t kNamePriority = 10u;
  fuchsia::sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.set_priority(kNamePriority);
  set_name_request.set_name(name);
  buffer_collections[0]->SetName(std::move(set_name_request));

  // Create rest of the logical buffer collections
  for (uint32_t i = 1; i < num_constraints; i++) {
    fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
    bind_shared_request.set_token(std::move(tokens[i]));
    bind_shared_request.set_buffer_collection_request(buffer_collections[i].NewRequest());
    status = sysmem_allocator_->BindSharedCollection(std::move(bind_shared_request));
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to create logical buffer collection");
      return status;
    }
  }

  // Set constraints
  for (uint32_t i = 0; i < num_constraints; i++) {
    fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
    set_constraints_request.set_constraints(fidl::Clone(constraints[i]));
    status = buffer_collections[i]->SetConstraints(std::move(set_constraints_request));
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to set buffer collection constraints");
      return status;
    }
  }

  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
  status = buffer_collections[0]->WaitForAllBuffersAllocated(&wait_result);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to wait for buffer collection info.");
    return status;
  }
  if (wait_result.is_framework_err()) {
    zxlogf(ERROR, "Failed to allocate buffer collection (framework_err): %d",
           wait_result.framework_err());
    return ZX_ERR_INTERNAL;
  }
  if (wait_result.is_err()) {
    zxlogf(ERROR, "Failed to allocate buffer collection (err): %u",
           static_cast<uint32_t>(wait_result.err()));
    auto v1_status = sysmem::V1CopyFromV2Error(fidl::HLCPPToNatural(wait_result.err()));
    return v1_status;
  }
  out_buffer_collection.buffers =
      std::move(*wait_result.response().mutable_buffer_collection_info());

  // Leave first collection handle open to return
  out_buffer_collection.ptr = buffer_collections[0].Unbind().Bind();

  for (uint32_t i = 1; i < num_constraints; i++) {
    status = buffer_collections[i]->Release();
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to close producer buffer collection");
      return status;
    }
  }

  return ZX_OK;
}

fuchsia::sysmem2::BufferCollectionHandle ControllerMemoryAllocator::AttachObserverCollection(
    fuchsia::sysmem2::BufferCollectionTokenHandle& token) {
  // Temporarily bind the provided token so it can be duplicated.
  auto ptr = token.BindSync();
  fuchsia::sysmem2::BufferCollectionTokenHandle observer;
  fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
  dup_request.set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS);
  dup_request.set_token_request(observer.NewRequest());
  ZX_ASSERT(ptr->Duplicate(std::move(dup_request)) == ZX_OK);
  {
    fuchsia::sysmem2::Node_Sync_Result sync_result;
    ZX_ASSERT(ptr->Sync(&sync_result) == ZX_OK);
  }
  // Return the channel to the provided token.
  token = ptr.Unbind();

  // Bind the new token to a collection, set constraints, and return the client end to the caller.
  fuchsia::sysmem2::BufferCollectionSyncPtr collection;
  fuchsia::sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
  bind_shared_request.set_token(std::move(observer));
  bind_shared_request.set_buffer_collection_request(collection.NewRequest());
  ZX_ASSERT(sysmem_allocator_->BindSharedCollection(std::move(bind_shared_request)) == ZX_OK);
  fuchsia::sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& constraints = *set_constraints_request.mutable_constraints();
  constraints.mutable_usage()->set_none(fuchsia::sysmem2::NONE_USAGE);
  ZX_ASSERT(collection->SetConstraints(std::move(set_constraints_request)) == ZX_OK);
  {
    fuchsia::sysmem2::Node_Sync_Result sync_result;
    ZX_ASSERT(collection->Sync(&sync_result) == ZX_OK);
  }
  return collection.Unbind();
}

}  // namespace camera
