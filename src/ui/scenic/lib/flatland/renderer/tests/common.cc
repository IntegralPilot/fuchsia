// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/renderer/tests/common.h"

#include <lib/fdio/directory.h>
#include <lib/trace/event.h>

#include "src/lib/fsl/handles/object_info.h"
#include "src/ui/lib/escher/vk/pipeline_builder.h"
#include "src/ui/scenic/lib/flatland/renderer/vk_renderer.h"

namespace flatland {

std::pair<std::unique_ptr<escher::Escher>, std::unique_ptr<VkRenderer>>
CreateEscherAndPrewarmedRenderer(bool use_protected_memory) {
  TRACE_DURATION("gfx", "CreateEscherAndPrewarmedRenderer");

  auto env = escher::test::EscherEnvironment::GetGlobalTestEnvironment();
  std::unique_ptr<escher::Escher> escher;
  if (use_protected_memory) {
    escher = escher::test::CreateEscherWithProtectedMemoryEnabled();
    if (!escher) {
      return {nullptr, nullptr};
    }
  } else {
    escher = std::make_unique<escher::Escher>(env->GetVulkanDevice(), env->GetFilesystem(),
                                              /*gpu_allocator*/ nullptr);
  }

  {
    auto pipeline_builder = std::make_unique<escher::PipelineBuilder>(escher->vk_device());
    pipeline_builder->set_log_pipeline_creation_callback(
        [](const vk::GraphicsPipelineCreateInfo* graphics_info,
           const vk::ComputePipelineCreateInfo* compute_info) {
          if (compute_info) {
            FX_CHECK(false) << "Unexpected lazy creation of Vulkan compute pipeline.";
          }
          if (graphics_info) {
            FX_CHECK(false) << "Unexpected lazy creation of Vulkan graphics pipeline.";
          }
        });
    escher->set_pipeline_builder(std::move(pipeline_builder));
  }
  auto renderer = std::make_unique<VkRenderer>(escher->GetWeakPtr());
  renderer->WarmPipelineCache();
  renderer->set_disable_lazy_pipeline_creation(true);

  return {std::move(escher), std::move(renderer)};
}

void RendererTest::SetUp() {
  TRACE_DURATION("gfx", "flatland::RendererTest::SetUp");

  escher::test::TestWithVkValidationLayer::SetUp();
}

void RendererTest::TearDown() {
  TRACE_DURATION("gfx", "flatland::RendererTest::TearDown");
  escher::test::TestWithVkValidationLayer::TearDown();
}

fidl::WireClient<fuchsia_sysmem2::Allocator> RendererTest::CreateSysmemAllocatorClient(
    async_dispatcher_t* dispatcher) {
  // Create the SysmemAllocator.
  auto [client_end, server_end] = fidl::Endpoints<fuchsia_sysmem2::Allocator>::Create();
  zx_status_t status =
      fdio_service_connect("/svc/fuchsia.sysmem2.Allocator", server_end.TakeChannel().release());
  fidl::WireClient<fuchsia_sysmem2::Allocator> sysmem_allocator{std::move(client_end), dispatcher};
  fidl::Arena arena;
  fidl::OneWayStatus result = sysmem_allocator->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(arena, fsl::GetCurrentProcessName() + " RendererTest")
          .id(fsl::GetCurrentProcessKoid())
          .Build());
  if (!result.ok()) {
    return {};
  }
  return sysmem_allocator;
}

}  // namespace flatland
