// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/impl/command_buffer.h"

#include <memory_resource>

#include "src/lib/fxl/macros.h"
#include "src/ui/lib/escher/impl/descriptor_set_pool.h"
#include "src/ui/lib/escher/impl/vulkan_utils.h"
#include "src/ui/lib/escher/util/trace_macros.h"
#include "src/ui/lib/escher/vk/buffer.h"
#include "src/ui/lib/escher/vk/framebuffer.h"
#include "src/ui/lib/escher/vk/image.h"
#include "src/ui/lib/escher/vk/render_pass.h"

namespace escher {
namespace impl {

CommandBuffer::CommandBuffer(vk::Device device, vk::CommandBuffer command_buffer, vk::Fence fence,
                             vk::PipelineStageFlags pipeline_stage_mask, bool use_protected_memory)
    : device_(device),
      command_buffer_(command_buffer),
      fence_(fence),
      pipeline_stage_mask_(pipeline_stage_mask),
      use_protected_memory_(use_protected_memory) {}

CommandBuffer::~CommandBuffer() {
  FX_DCHECK(!is_active_ && !is_submitted_);
  // Owner is responsible for destroying command buffer and fence.
}

void CommandBuffer::Begin(uint64_t sequence_number) {
  TRACE_DURATION("gfx", "escher::CommandBuffer::Begin", "ptr", TA_POINTER(this));
  FX_DCHECK(!is_active_ && !is_submitted_);
  FX_DCHECK(sequence_number > sequence_number_);
  is_active_ = true;
  sequence_number_ = sequence_number;
  auto result = command_buffer_.begin(vk::CommandBufferBeginInfo());
  FX_DCHECK(result == vk::Result::eSuccess);
}

bool CommandBuffer::Submit(vk::Queue queue, CommandBufferFinishedCallback callback) {
  // NOTE: this name is important for benchmarking.  Do not remove or modify it
  // without also updating the "process_gfx_trace.go" script.
  TRACE_DURATION("gfx", "escher::CommandBuffer::Submit", "ptr", TA_POINTER(this));

  FX_DCHECK(is_active_ && !is_submitted_);
  is_submitted_ = true;
  callback_ = std::move(callback);

  auto end_command_buffer_result = command_buffer_.end();
  FX_DCHECK(end_command_buffer_result == vk::Result::eSuccess);

  static constexpr vk::ProtectedSubmitInfo kProtectedSubmitInfo =
      vk::ProtectedSubmitInfo().setProtectedSubmit(true);

  // Build raw semaphore arrays on the stack using a single PMR pool to avoid
  // heap allocations in the common case where we have few semaphores.
  std::byte buffer[256];
  std::pmr::monotonic_buffer_resource pool{buffer, sizeof(buffer)};

  std::pmr::vector<vk::Semaphore> raw_wait_semaphores{&pool};
  raw_wait_semaphores.reserve(wait_semaphores_.size());
  for (const auto& s : wait_semaphores_) {
    raw_wait_semaphores.push_back(s->vk_semaphore());
  }

  std::pmr::vector<vk::Semaphore> raw_signal_semaphores{&pool};
  raw_signal_semaphores.reserve(signal_semaphores_.size());
  for (const auto& s : signal_semaphores_) {
    raw_signal_semaphores.push_back(s->vk_semaphore());
  }

  const vk::SubmitInfo submit_info(
      /*waitSemaphoreCount=*/static_cast<uint32_t>(raw_wait_semaphores.size()),
      /*pWaitSemaphores*/ raw_wait_semaphores.data(),
      /*pWaitDstStageMask=*/wait_semaphore_stages_.data(),
      /*commandBufferCount=*/1,
      /*pCommandBuffers=*/&command_buffer_,
      /*signalSemaphoreCount=*/static_cast<uint32_t>(raw_signal_semaphores.size()),
      /*pSignalSemaphores=*/raw_signal_semaphores.data(),
      /*pNext=*/use_protected_memory_ ? &kProtectedSubmitInfo : nullptr);
  auto submit_result = queue.submit(1, &submit_info, fence_);
  if (submit_result != vk::Result::eSuccess) {
    FX_LOGS(WARNING) << "failed queue submission: " << to_string(submit_result);
    // Clearing these flags allows Retire() to make progress.
    is_active_ = is_submitted_ = false;
    return false;
  }
  return true;
}

vk::Result CommandBuffer::Wait(uint64_t nanoseconds) {
  if (!is_active_) {
    // The command buffer is already finished.
    return vk::Result::eSuccess;
  }
  FX_DCHECK(is_submitted_);
  return device_.waitForFences(1, &fence_, true, nanoseconds);
}

void CommandBuffer::AddWaitSemaphore(SemaphorePtr semaphore, vk::PipelineStageFlags stage) {
  FX_DCHECK(is_active_);
  if (semaphore) {
    wait_semaphore_stages_.push_back(stage);
    wait_semaphores_.push_back(std::move(semaphore));
  }
}

void CommandBuffer::AddSignalSemaphore(SemaphorePtr semaphore) {
  FX_DCHECK(is_active_);
  if (semaphore) {
    if (ContainsSignalSemaphore(semaphore)) {
      return;
    }
    signal_semaphores_.push_back(std::move(semaphore));
  }
}

bool CommandBuffer::ContainsSignalSemaphore(const SemaphorePtr& semaphore) const {
  return std::find(signal_semaphores_.begin(), signal_semaphores_.end(), semaphore) !=
         signal_semaphores_.end();
}

void CommandBuffer::KeepAlive(const Resource* resource) {
  FX_DCHECK(is_active_);
  if (sequence_number_ == resource->sequence_number()) {
    // The resource is already being kept alive by this CommandBuffer.
    return;
  }

  resource->KeepAlive(sequence_number_);
}

void CommandBuffer::CopyImage(const ImagePtr& src_image, const ImagePtr& dst_image,
                              vk::ImageLayout src_layout, vk::ImageLayout dst_layout,
                              vk::ImageCopy* region) {
  FX_CHECK(region);

  // If commandBuffer is a protected command buffer, then dstImage must not be an unprotected image.
  // https://www.khronos.org/registry/vulkan/specs/1.1-extensions/man/html/vkCmdCopyImage.html
  FX_CHECK(!use_protected_memory() || dst_image->use_protected_memory());

  if (region->srcOffset.x < 0 || region->srcOffset.y < 0 || region->srcOffset.z < 0 ||
      static_cast<uint64_t>(region->srcOffset.x) + region->extent.width > src_image->width() ||
      static_cast<uint64_t>(region->srcOffset.y) + region->extent.height > src_image->height() ||
      static_cast<uint64_t>(region->srcOffset.z) + region->extent.depth > 1U) {
    FX_DCHECK(false);
    return;
  }

  if (region->dstOffset.x < 0 || region->dstOffset.y < 0 || region->dstOffset.z < 0 ||
      static_cast<uint64_t>(region->dstOffset.x) + region->extent.width > dst_image->width() ||
      static_cast<uint64_t>(region->dstOffset.y) + region->extent.height > dst_image->height() ||
      static_cast<uint64_t>(region->dstOffset.z) + region->extent.depth > 1U) {
    FX_DCHECK(false);
    return;
  }

  command_buffer_.copyImage(src_image->vk(), src_layout, dst_image->vk(), dst_layout, 1, region);
  KeepAlive(src_image);
  KeepAlive(dst_image);
}

void CommandBuffer::CopyBuffer(const BufferPtr& src, const BufferPtr& dst, vk::BufferCopy region) {
  // If commandBuffer is a protected command buffer, then dstBuffer must not be an unprotected
  // buffer.
  // https://www.khronos.org/registry/vulkan/specs/1.1-extensions/man/html/vkCmdCopyBuffer.html
  // We do not use protected buffers.
  FX_CHECK(!use_protected_memory());

  if (region.srcOffset >= src->size() || region.dstOffset >= dst->size()) {
    FX_DCHECK(false);
    return;
  }

  vk::DeviceSize src_readable = src->size() - region.srcOffset;
  vk::DeviceSize dst_writeable = dst->size() - region.dstOffset;
  vk::DeviceSize clamped_size = std::min({region.size, src_readable, dst_writeable});
  FX_DCHECK(clamped_size == region.size);
  region.size = clamped_size;

  if (region.size == 0) {
    return;
  }

  command_buffer_.copyBuffer(src->vk(), dst->vk(), 1 /* region_count */, &region);
  KeepAlive(src);
  KeepAlive(dst);
}

void CommandBuffer::CopyBufferAfterBarrier(const BufferPtr& src, const BufferPtr& dst,
                                           vk::BufferCopy region, vk::AccessFlags src_access_mask,
                                           vk::PipelineStageFlags src_stage_mask) {
  vk::BufferMemoryBarrier barrier;
  barrier.srcAccessMask = src_access_mask;
  barrier.dstAccessMask = vk::AccessFlagBits::eTransferRead;
  barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
  barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
  barrier.buffer = dst->vk();
  barrier.offset = 0;
  barrier.size = dst->size();
  command_buffer_.pipelineBarrier(src_stage_mask, vk::PipelineStageFlagBits::eTransfer,
                                  vk::DependencyFlags(), 0, nullptr, 1, &barrier, 0, nullptr);
  CopyBuffer(src, dst, region);
}

// TODO(https://fxbug.dev/42117363): Move this function out to a separated utility function, rather
// than part of impl::CommandBuffer.
void CommandBuffer::TransitionImageLayout(const ImagePtr& image, vk::ImageLayout old_layout,
                                          vk::ImageLayout new_layout,
                                          uint32_t src_queue_family_index,
                                          uint32_t dst_queue_family_index) {
  TRACE_DURATION("gfx", "escher::CommandBuffer::TransitionImageLayout", "ptr", TA_POINTER(this));
  KeepAlive(image);

  vk::PipelineStageFlags src_stage_mask;
  vk::PipelineStageFlags dst_stage_mask;

  vk::ImageMemoryBarrier barrier;
  barrier.oldLayout = old_layout;
  barrier.newLayout = new_layout;
  barrier.srcQueueFamilyIndex = src_queue_family_index;
  barrier.dstQueueFamilyIndex = dst_queue_family_index;
  barrier.image = image->vk();

  if (image->has_depth() || image->has_stencil()) {
    if (image->has_depth()) {
      barrier.subresourceRange.aspectMask = vk::ImageAspectFlagBits::eDepth;
    }
    if (image->has_stencil()) {
      barrier.subresourceRange.aspectMask |= vk::ImageAspectFlagBits::eStencil;
    }
  } else {
    barrier.subresourceRange.aspectMask = vk::ImageAspectFlagBits::eColor;
  }

  // TODO: assert that image only has one level.
  barrier.subresourceRange.baseMipLevel = 0;
  barrier.subresourceRange.levelCount = 1;
  barrier.subresourceRange.baseArrayLayer = 0;
  barrier.subresourceRange.layerCount = 1;

  switch (old_layout) {
    case vk::ImageLayout::eColorAttachmentOptimal:
      barrier.srcAccessMask =
          vk::AccessFlagBits::eColorAttachmentRead | vk::AccessFlagBits::eColorAttachmentWrite;
      src_stage_mask = vk::PipelineStageFlagBits::eColorAttachmentOutput;
      break;
    case vk::ImageLayout::eDepthStencilAttachmentOptimal:
      barrier.srcAccessMask = vk::AccessFlagBits::eDepthStencilAttachmentRead |
                              vk::AccessFlagBits::eDepthStencilAttachmentWrite;
      src_stage_mask = vk::PipelineStageFlagBits::eLateFragmentTests;
      break;
    case vk::ImageLayout::eGeneral:
      barrier.srcAccessMask = vk::AccessFlagBits::eShaderRead | vk::AccessFlagBits::eShaderWrite;
      src_stage_mask = vk::PipelineStageFlagBits::eComputeShader;
      break;
    case vk::ImageLayout::ePreinitialized:
      barrier.srcAccessMask = vk::AccessFlagBits::eHostWrite;
      src_stage_mask = vk::PipelineStageFlagBits::eHost;
      break;
    case vk::ImageLayout::eShaderReadOnlyOptimal:
      barrier.srcAccessMask = vk::AccessFlagBits::eShaderRead;
      // TODO: investigate whether there are performance benefits to providing
      // a less-conservative mask.
      src_stage_mask = vk::PipelineStageFlagBits::eVertexShader |
                       vk::PipelineStageFlagBits::eTessellationControlShader |
                       vk::PipelineStageFlagBits::eTessellationEvaluationShader |
                       vk::PipelineStageFlagBits::eGeometryShader |
                       vk::PipelineStageFlagBits::eFragmentShader;
      break;
    case vk::ImageLayout::eTransferDstOptimal:
      barrier.srcAccessMask = vk::AccessFlagBits::eTransferWrite;
      src_stage_mask = vk::PipelineStageFlagBits::eTransfer;
      break;
    case vk::ImageLayout::eTransferSrcOptimal:
      barrier.srcAccessMask = vk::AccessFlagBits::eTransferRead;
      src_stage_mask = vk::PipelineStageFlagBits::eTransfer;
      break;
    case vk::ImageLayout::eUndefined:
      // If layout was eUndefined, we don't need a srcAccessMask.

      // Source images with eUndefined layout have not yet been initialized nor
      // used, or we do not care about their previously stored data. So we use
      // eTopOfPipe as the source stage mask, as it will never block the
      // pipeline barrier.
      src_stage_mask = vk::PipelineStageFlagBits::eTopOfPipe;
      break;
    default:
      FX_LOGS(ERROR) << "CommandBuffer does not know how to transition from layout: "
                     << vk::to_string(old_layout);
      FX_DCHECK(false);
  }

  switch (new_layout) {
    case vk::ImageLayout::eColorAttachmentOptimal:
      barrier.dstAccessMask =
          vk::AccessFlagBits::eColorAttachmentRead | vk::AccessFlagBits::eColorAttachmentWrite;
      dst_stage_mask = vk::PipelineStageFlagBits::eColorAttachmentOutput;
      break;
    case vk::ImageLayout::eDepthStencilAttachmentOptimal:
      barrier.dstAccessMask = vk::AccessFlagBits::eDepthStencilAttachmentRead |
                              vk::AccessFlagBits::eDepthStencilAttachmentWrite;
      dst_stage_mask = vk::PipelineStageFlagBits::eEarlyFragmentTests;
      break;
    case vk::ImageLayout::eGeneral:
      barrier.dstAccessMask = vk::AccessFlagBits::eShaderRead | vk::AccessFlagBits::eShaderWrite;
      dst_stage_mask = vk::PipelineStageFlagBits::eComputeShader;
      break;
    case vk::ImageLayout::ePresentSrcKHR:
      barrier.dstAccessMask = vk::AccessFlagBits::eMemoryRead;
      dst_stage_mask = vk::PipelineStageFlagBits::eAllGraphics;
      break;
    case vk::ImageLayout::eShaderReadOnlyOptimal:
      barrier.dstAccessMask = vk::AccessFlagBits::eShaderRead;
      // TODO: investigate whether there are performance benefits to providing
      // a less-conservative mask.
      dst_stage_mask = (pipeline_stage_mask_ & vk::PipelineStageFlagBits::eAllCommands)
                           ? vk::PipelineStageFlagBits::eAllCommands
                           : vk::PipelineStageFlagBits::eAllGraphics;
      break;
    case vk::ImageLayout::eTransferDstOptimal:
      barrier.dstAccessMask = vk::AccessFlagBits::eTransferWrite;
      dst_stage_mask = vk::PipelineStageFlagBits::eTransfer;
      break;
    case vk::ImageLayout::eTransferSrcOptimal:
      barrier.dstAccessMask = vk::AccessFlagBits::eTransferRead;
      dst_stage_mask = vk::PipelineStageFlagBits::eTransfer;
      break;
    case vk::ImageLayout::eUndefined:
    default:
      FX_LOGS(ERROR) << "CommandBuffer does not know how to transition to layout: "
                     << vk::to_string(new_layout);
      FX_DCHECK(false);
  }

  src_stage_mask = src_stage_mask & pipeline_stage_mask_;
  dst_stage_mask = dst_stage_mask & pipeline_stage_mask_;

  image->set_layout(new_layout);

  command_buffer_.pipelineBarrier(src_stage_mask, dst_stage_mask, vk::DependencyFlagBits::eByRegion,
                                  0, nullptr, 0, nullptr, 1, &barrier);
}

void CommandBuffer::BeginRenderPass(const escher::RenderPassPtr& render_pass,
                                    const escher::FramebufferPtr& framebuffer,
                                    const std::vector<vk::ClearValue>& clear_values,
                                    const vk::Rect2D viewport) {
  KeepAlive(render_pass);
  BeginRenderPass(render_pass->vk(), framebuffer, clear_values.data(), clear_values.size(),
                  viewport);
}

void CommandBuffer::BeginRenderPass(vk::RenderPass render_pass,
                                    const escher::FramebufferPtr& framebuffer,
                                    const std::vector<vk::ClearValue>& clear_values,
                                    const vk::Rect2D viewport) {
  BeginRenderPass(render_pass, framebuffer, clear_values.data(), clear_values.size(), viewport);
}

void CommandBuffer::BeginRenderPass(vk::RenderPass render_pass,
                                    const escher::FramebufferPtr& framebuffer,
                                    const vk::ClearValue* clear_values, size_t clear_value_count,
                                    vk::Rect2D viewport) {
  FX_DCHECK(is_active_);
  KeepAlive(framebuffer);

  vk::RenderPassBeginInfo info;
  info.renderPass = render_pass;
  info.renderArea = viewport;
  info.clearValueCount = static_cast<uint32_t>(clear_value_count);
  info.pClearValues = clear_values;
  info.framebuffer = framebuffer->vk();

  command_buffer_.beginRenderPass(&info, vk::SubpassContents::eInline);

  vk::Viewport vk_viewport;
  vk_viewport.x = static_cast<float>(viewport.offset.x);
  vk_viewport.y = static_cast<float>(viewport.offset.y);
  vk_viewport.width = static_cast<float>(viewport.extent.width);
  vk_viewport.height = static_cast<float>(viewport.extent.height);
  vk_viewport.minDepth = static_cast<float>(0.0f);
  vk_viewport.maxDepth = static_cast<float>(1.0f);
  command_buffer_.setViewport(0, 1, &vk_viewport);

  // TODO: probably unnecessary?
  vk::Rect2D scissor;
  scissor.extent.width = viewport.extent.width;
  scissor.extent.height = viewport.extent.height;
  scissor.offset.x = viewport.offset.x;
  scissor.offset.y = viewport.offset.y;
  command_buffer_.setScissor(0, 1, &scissor);
}

void CommandBuffer::EndRenderPass() { command_buffer_.endRenderPass(); }

bool CommandBuffer::Retire() {
  if (!is_active_) {
    // Submission failed, so proceed with cleanup.
    FX_DLOGS(INFO) << "CommandBuffer submission failed, proceeding with retirement";
  } else if (!is_submitted_) {
    return false;
  } else {
    FX_DCHECK(is_active_);
    // Check if fence has been reached.
    auto fence_status = device_.getFenceStatus(fence_);
    if (fence_status == vk::Result::eNotReady) {
      // Fence has not been reached; try again later.
      return false;
    }
  }
  is_active_ = is_submitted_ = false;
  ESCHER_CHECKED_VK_RESULT(device_.resetFences(1, &fence_));

  if (callback_) {
    TRACE_DURATION("gfx", "escher::CommandBuffer::Retire::callback", "ptr", TA_POINTER(this));
    callback_();
    callback_ = nullptr;
  }

  wait_semaphores_.clear();
  wait_semaphore_stages_.clear();
  signal_semaphores_.clear();

  auto result = command_buffer_.reset(vk::CommandBufferResetFlags());
  FX_DCHECK(result == vk::Result::eSuccess);

  return true;
}

}  // namespace impl
}  // namespace escher
