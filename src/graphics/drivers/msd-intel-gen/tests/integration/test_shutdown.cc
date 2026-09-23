// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.gpu.magma/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/magma_client/test_util/inflight_list.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <lib/zx/channel.h>
#include <magma_intel_gen_defs.h>

#include <future>

#include <gtest/gtest.h>

namespace {

class TestConnection : public magma::TestDeviceBase {
 public:
  TestConnection() : magma::TestDeviceBase(MAGMA_VENDOR_ID_INTEL) {
    magma_device_create_connection(device(), &connection_);
    if (!connection_) {
      return;
    }

    magma_status_t status = magma_device_query(device(), kMagmaIntelGenQueryExtraPageCount, nullptr,
                                               &extra_page_count_);
    if (status != MAGMA_STATUS_OK) {
      DLOG("Failed to query kMagmaIntelGenQueryExtraPageCount: %d", status);
      extra_page_count_ = 0;
    }
  }

  ~TestConnection() {
    if (connection_)
      magma_connection_release(connection_);
  }

  static constexpr int64_t kOneSecondInNs = 1000000000;

  magma_status_t Test() {
    DASSERT(connection_);

    uint32_t context_id;
    magma::Status status = magma_connection_create_context(connection_, &context_id);
    if (!status.ok())
      return DRET(status.get());

    status = magma_connection_flush(connection_);
    if (!status.ok())
      return DRET(status.get());

    const uint64_t page_size = zx_system_get_page_size();
    uint64_t size;
    magma_buffer_t batch_buffer;
    magma_buffer_id_t batch_buffer_id;
    status = magma_connection_create_buffer(connection_, page_size, &size, &batch_buffer,
                                            &batch_buffer_id);
    if (!status.ok()) {
      magma_connection_release_context(connection_, context_id);
      return DRET(status.get());
    }

    constexpr uint64_t kMapFlags =
        MAGMA_MAP_FLAG_READ | MAGMA_MAP_FLAG_WRITE | MAGMA_MAP_FLAG_EXECUTE;

    status = magma_connection_map_buffer(connection_, gpu_addr_, batch_buffer, 0,
                                         magma::page_size(), kMapFlags);
    if (!status.ok()) {
      magma_connection_release_context(connection_, context_id);
      magma_connection_release_buffer(connection_, batch_buffer);
      return DRET(status.get());
    }

    gpu_addr_ += (1 + extra_page_count_) * page_size;

    EXPECT_TRUE(InitBatchBuffer(batch_buffer, size));

    magma_command_descriptor descriptor;
    magma_exec_command_buffer command_buffer;
    magma_exec_resource exec_resource;
    EXPECT_TRUE(InitCommand(&descriptor, &command_buffer, &exec_resource, batch_buffer,
                            batch_buffer_id, size));

    status = magma_connection_execute_command(connection_, context_id, &descriptor);
    if (!status.ok()) {
      magma_connection_release_context(connection_, context_id);
      magma_connection_release_buffer(connection_, batch_buffer);
      return DRET(status.get());
    }

    magma::InflightList list;
    status = list.WaitForCompletion(connection_, kOneSecondInNs);
    EXPECT_TRUE(status.get() == MAGMA_STATUS_OK || status.get() == MAGMA_STATUS_CONNECTION_LOST);

    magma_connection_release_context(connection_, context_id);
    magma_connection_release_buffer(connection_, batch_buffer);

    status = magma_connection_flush(connection_);
    return DRET(status.get());
  }

  bool InitBatchBuffer(magma_buffer_t buffer, uint64_t size) {
    void* vaddr;
    if (!magma::MapCpuHelper(buffer, 0 /*offset*/, size, &vaddr))
      return DRETF(false, "couldn't map batch buffer");

    memset(vaddr, 0, size);

    // Intel end-of-batch
    *reinterpret_cast<uint32_t*>(vaddr) = 0xA << 23;

    EXPECT_TRUE(magma::UnmapCpuHelper(vaddr, size));

    return true;
  }

  bool InitCommand(magma_command_descriptor* descriptor, magma_exec_command_buffer* command_buffer,
                   magma_exec_resource* exec_resource, magma_buffer_t batch_buffer,
                   magma_buffer_id_t batch_buffer_id, uint64_t batch_buffer_length) {
    exec_resource->buffer_id = batch_buffer_id;
    exec_resource->offset = 0;
    exec_resource->length = batch_buffer_length;

    command_buffer->resource_index = 0;
    command_buffer->start_offset = 0;

    descriptor->resource_count = 1;
    descriptor->command_buffer_count = 1;
    descriptor->wait_semaphore_count = 0;
    descriptor->signal_semaphore_count = 0;
    descriptor->resources = exec_resource;
    descriptor->command_buffers = command_buffer;
    descriptor->semaphore_ids = nullptr;
    descriptor->flags = 0;

    return true;
  }

 private:
  magma_connection_t connection_ = 0;
  uint64_t extra_page_count_ = 0;
  uint64_t gpu_addr_ = 0;
};

// TODO(https://fxbug.dev/351097268): The test is currently disabled because
// the DFv2 intel-i915 driver doesn't support the fuchsia.device/Controller
// protocol and does not export the devfs node.
TEST(Shutdown, DISABLED_Test) {
  // TODO(https://fxbug.dev/328808539)
}

}  // namespace
