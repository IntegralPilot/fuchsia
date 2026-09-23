// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#include <lib/magma_client/test_util/inflight_list.h>
#include <lib/magma_client/test_util/magma_map_cpu.h>
#include <lib/magma_client/test_util/test_device_helper.h>
#include <magma_intel_gen_defs.h>

#include <thread>
#include <vector>

#include <gtest/gtest.h>

namespace {

constexpr uint64_t kMapFlags = MAGMA_MAP_FLAG_READ | MAGMA_MAP_FLAG_WRITE | MAGMA_MAP_FLAG_EXECUTE;

const size_t kPageSize = zx_system_get_page_size();

class TestConnection : public magma::TestDeviceBase {
 public:
  TestConnection() : magma::TestDeviceBase(MAGMA_VENDOR_ID_INTEL) {
    magma_device_create_connection(device(), &connection_);

    magma_status_t status = magma_device_query(device(), kMagmaIntelGenQueryExtraPageCount, nullptr,
                                               &extra_page_count_);
    if (status != MAGMA_STATUS_OK) {
      fprintf(stderr, "Failed to query kMagmaIntelGenQueryExtraPageCount: %d\n", status);
      extra_page_count_ = 0;
    }
  }

  ~TestConnection() {
    if (connection_)
      magma_connection_release(connection_);
  }

  void CheckWorkarounds(uint32_t register_offset, uint32_t expected_value) {
    ASSERT_TRUE(connection_);

    uint32_t context_id;
    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_context(connection_, &context_id));

    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));

    uint64_t size;
    magma_buffer_t batch_buffer;
    magma_buffer_id_t batch_buffer_id;
    magma_buffer_t result_buffer;
    magma_buffer_id_t result_buffer_id;

    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, kPageSize, &size,
                                                              &batch_buffer, &batch_buffer_id));
    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_create_buffer(connection_, kPageSize, &size,
                                                              &result_buffer, &result_buffer_id));

    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_map_buffer(connection_, gpu_addr_, batch_buffer, 0,
                                                           magma::page_size(), kMapFlags));
    gpu_addr_ += (1 + extra_page_count_) * kPageSize;

    EXPECT_EQ(MAGMA_STATUS_OK, magma_connection_map_buffer(connection_, gpu_addr_, result_buffer, 0,
                                                           magma::page_size(), kMapFlags));

    EXPECT_TRUE(InitBatchBuffer(batch_buffer, size, register_offset, gpu_addr_));

    EXPECT_TRUE(ClearBuffer(result_buffer, size, 0xabcd1234));

    magma_command_descriptor descriptor;
    magma_exec_command_buffer command_buffer;
    std::vector<magma_exec_resource> exec_resources;
    EXPECT_TRUE(InitCommand(&descriptor, &command_buffer, &exec_resources, batch_buffer_id,
                            kPageSize, result_buffer_id, kPageSize));

    EXPECT_EQ(MAGMA_STATUS_OK,
              magma_connection_execute_command(connection_, context_id, &descriptor));

    {
      magma::InflightList list;
      static constexpr int64_t kOneSecondInNs = 1000000000;
      magma::Status status = list.WaitForCompletion(connection_, kOneSecondInNs);
      EXPECT_EQ(MAGMA_STATUS_OK, status.get());
    }

    uint32_t result;
    EXPECT_TRUE(ReadBufferAt(result_buffer, size, 0, &result));

    EXPECT_EQ(expected_value, result)
        << " expected: 0x" << std::hex << expected_value << " got: 0x" << result;

    magma_connection_release_buffer(connection_, result_buffer);
    magma_connection_release_buffer(connection_, batch_buffer);
    magma_connection_release_context(connection_, context_id);

    ASSERT_EQ(MAGMA_STATUS_OK, magma_connection_flush(connection_));
  }

  bool ReadBufferAt(magma_buffer_t buffer, size_t size, uint32_t dword_offset,
                    uint32_t* result_out) {
    void* vaddr;
    if (!magma::MapCpuHelper(buffer, 0 /*offset*/, size, &vaddr))
      return DRETF(false, "MapCpuHelper failed");

    *result_out = reinterpret_cast<uint32_t*>(vaddr)[dword_offset];

    if (!magma::UnmapCpuHelper(vaddr, size))
      return DRETF(false, "UnmapCpuHelper failed");

    return true;
  }

  bool ClearBuffer(magma_buffer_t buffer, size_t size, uint32_t value) {
    void* vaddr;
    if (!magma::MapCpuHelper(buffer, 0 /*offset*/, size, &vaddr))
      return DRETF(false, "CpuMapHelper failed");

    for (uint32_t i = 0; i < size / sizeof(uint32_t); i++) {
      reinterpret_cast<uint32_t*>(vaddr)[i] = value;
    }

    if (!magma::UnmapCpuHelper(vaddr, size))
      return DRETF(false, "UnmapCpuHelper failed");

    return true;
  }

  bool InitBatchBuffer(magma_buffer_t buffer, size_t size, uint32_t register_offset,
                       uint64_t target_gpu_addr) {
    void* vaddr;
    if (!magma::MapCpuHelper(buffer, 0 /*offset*/, size, &vaddr))
      return DRETF(false, "MapCpuHelper failed");

    memset(vaddr, 0, size);

    {
      auto batch_ptr = reinterpret_cast<uint32_t*>(vaddr);
      *batch_ptr++ = (0 << 29)       // command type: MI_COMMAND
                     | (0x24 << 23)  // command opcode: MI_STORE_REGISTER_MEM
                     | (2 << 0);     // number of dwords - 2
      *batch_ptr++ = register_offset;
      *batch_ptr++ = magma::lower_32_bits(target_gpu_addr);
      *batch_ptr++ = magma::upper_32_bits(target_gpu_addr);

      *batch_ptr++ = (0 << 29)       // command type: MI_COMMAND
                     | (0xA << 23);  // command opcode: MI_BATCH_BUFFER_END
    }

    if (!magma::UnmapCpuHelper(vaddr, size))
      return DRETF(false, "UnmapCpuHelper failed");

    return true;
  }

  bool InitCommand(magma_command_descriptor* descriptor, magma_exec_command_buffer* command_buffer,
                   std::vector<magma_exec_resource>* exec_resources,
                   magma_buffer_id_t batch_buffer_id, uint64_t batch_buffer_size,
                   magma_buffer_id_t result_buffer_id, uint64_t result_buffer_size) {
    exec_resources->clear();

    exec_resources->push_back(
        {.buffer_id = batch_buffer_id, .offset = 0, .length = batch_buffer_size});

    exec_resources->push_back(
        {.buffer_id = result_buffer_id, .offset = 0, .length = result_buffer_size});

    command_buffer->resource_index = 0;
    command_buffer->start_offset = 0;

    descriptor->resource_count = static_cast<uint32_t>(exec_resources->size());
    descriptor->command_buffer_count = 1;
    descriptor->wait_semaphore_count = 0;
    descriptor->signal_semaphore_count = 0;
    descriptor->resources = exec_resources->data(), descriptor->command_buffers = command_buffer,
    descriptor->semaphore_ids = nullptr;
    descriptor->flags = 0;

    return true;
  }

 private:
  magma_connection_t connection_;
  uint64_t extra_page_count_ = 0;
  uint64_t gpu_addr_ = 0;
};

}  // namespace

// TODO(https://fxbug.dev/42161918) - enable
TEST(Workarounds, DISABLED_Register0x7004) { TestConnection().CheckWorkarounds(0x7004, 0x29c2); }

TEST(Workarounds, Register0x7300) {
  auto helper = magma::TestDeviceBase(0x8086);
  if (helper.IsIntelGen12())
    GTEST_SKIP();
  TestConnection().CheckWorkarounds(0x7300, 0x810);
}
