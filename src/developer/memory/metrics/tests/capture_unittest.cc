// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/capture.h"

#include <zircon/types.h>

#include <cstdint>

#include <gtest/gtest.h>

#include "src/developer/memory/metrics/tests/test_utils.h"
#include "zircon/syscalls/object.h"

namespace memory::test {

using CaptureUnitTest = testing::Test;

namespace {
const zx_info_kmem_stats_t _kmem{
    .total_bytes = 300,
    .free_bytes = 100,
    .wired_bytes = 10,
    .total_heap_bytes = 20,
    .free_heap_bytes = 30,
    .vmo_bytes = 40,
    .mmu_overhead_bytes = 50,
    .ipc_bytes = 60,
    .other_bytes = 70,
    .vmo_reclaim_total_bytes = 15,
    .vmo_reclaim_newest_bytes = 4,
    .vmo_reclaim_oldest_bytes = 8,
    .vmo_discardable_locked_bytes = 3,
    .vmo_discardable_unlocked_bytes = 7,
};
const GetInfoResponse kmem_info{.handle = TestUtils::kRootHandle,
                                .topic = ZX_INFO_KMEM_STATS,
                                .values = &_kmem,
                                .value_size = sizeof(_kmem),
                                .value_count = 1,
                                .ret = ZX_OK};

const zx_info_kmem_stats_compression_t _kmem_compression{};

const GetInfoResponse kmem_compression_info{.handle = TestUtils::kRootHandle,
                                            .topic = ZX_INFO_KMEM_STATS_COMPRESSION,
                                            .values = &_kmem_compression,
                                            .value_size = sizeof(_kmem_compression),
                                            .value_count = 1,
                                            .ret = ZX_OK};

const zx_info_handle_basic_t _self{.koid = TestUtils::kSelfKoid};
const GetInfoResponse self_info{.handle = TestUtils::kSelfHandle,
                                .topic = ZX_INFO_HANDLE_BASIC,
                                .values = &_self,
                                .value_size = sizeof(_self),
                                .value_count = 1,
                                .ret = ZX_OK};

const zx_koid_t proc_koid = 10;
const zx_handle_t proc_handle = 100;
const char proc_name[] = "P1";
const GetPropertyResponse proc_prop{.handle = proc_handle,
                                    .property = ZX_PROP_NAME,
                                    .value = proc_name,
                                    .value_len = sizeof(proc_name),
                                    .ret = ZX_OK};
const GetProcessesCallback proc_cb{
    .depth = 1, .handle = proc_handle, .koid = proc_koid, .parent_koid = 0};

const zx_koid_t proc2_koid = 20;
const zx_handle_t proc2_handle = 200;
const zx_handle_t proc2_job = 1000;
const char proc2_name[] = "P2";
const GetPropertyResponse proc2_prop{.handle = proc2_handle,
                                     .property = ZX_PROP_NAME,
                                     .value = proc2_name,
                                     .value_len = sizeof(proc2_name),
                                     .ret = ZX_OK};
const GetProcessesCallback proc2_cb{
    .depth = 1, .handle = proc2_handle, .koid = proc2_koid, .parent_koid = proc2_job};

const zx_koid_t vmo_koid = 1000;
const uint64_t vmo_size = 10000;
const char vmo_name[] = "V1";
const zx_info_vmo_t _vmo{
    .koid = vmo_koid,
    .name = "V1",
    .size_bytes = vmo_size,
};
const zx_info_vmo_t _vmo_dup[]{{
                                   .koid = vmo_koid,
                                   .name = "V1",
                                   .size_bytes = vmo_size,
                               },
                               {
                                   .koid = vmo_koid,
                                   .name = "V1",
                                   .size_bytes = vmo_size,
                               }};
const GetInfoResponse vmos_info{.handle = proc_handle,
                                .topic = ZX_INFO_PROCESS_VMOS,
                                .values = &_vmo,
                                .value_size = sizeof(_vmo),
                                .value_count = 1,
                                .ret = ZX_OK};
const GetInfoResponse vmos_dup_info{.handle = proc_handle,
                                    .topic = ZX_INFO_PROCESS_VMOS,
                                    .values = _vmo_dup,
                                    .value_size = sizeof(_vmo),
                                    .value_count = 1,
                                    .ret = ZX_OK};

const zx_koid_t vmo2_koid = 2000;
const uint64_t vmo2_size = 20000;
const char vmo2_name[] = "V2";
const zx_info_vmo_t _vmo2{
    .koid = vmo2_koid,
    .name = "V2",
    .size_bytes = vmo2_size,
};
const GetInfoResponse vmos2_info{.handle = proc2_handle,
                                 .topic = ZX_INFO_PROCESS_VMOS,
                                 .values = &_vmo2,
                                 .value_size = sizeof(_vmo2),
                                 .value_count = 1,
                                 .ret = ZX_OK};
}  // namespace

TEST_F(CaptureUnitTest, KMEM) {
  Capture c;
  auto ret = TestUtils::GetCapture(&c, CaptureLevel::KMEM,
                                   {
                                       .get_info = {self_info, kmem_info, kmem_compression_info},
                                   });
  EXPECT_EQ(ZX_OK, ret);
  const auto& got_kmem = c.kmem();
  EXPECT_EQ(_kmem.total_bytes, got_kmem.total_bytes);
}

TEST_F(CaptureUnitTest, Process) {
  // Process and VMO need to capture the same info.
  Capture c;
  auto ret = TestUtils::GetCapture(
      &c, CaptureLevel::VMO,
      {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb}},
       .get_property = {proc_prop},
       .get_info = {self_info, kmem_info, kmem_compression_info, vmos_info, vmos_info}});
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(1U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc_koid);
  EXPECT_EQ(proc_koid, process.koid);
  EXPECT_STREQ(proc_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo_koid);
  EXPECT_EQ(vmo_koid, vmo.koid);
  EXPECT_STREQ(vmo_name, vmo.name);
}

TEST_F(CaptureUnitTest, VMO) {
  Capture c;
  auto ret = TestUtils::GetCapture(
      &c, CaptureLevel::VMO,
      {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb}},
       .get_property = {proc_prop},
       .get_info = {self_info, kmem_info, kmem_compression_info, vmos_info, vmos_info}});
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(1U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc_koid);
  EXPECT_EQ(proc_koid, process.koid);
  EXPECT_STREQ(proc_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo_koid);
  EXPECT_EQ(vmo_koid, vmo.koid);
  EXPECT_STREQ(vmo_name, vmo.name);
}

TEST_F(CaptureUnitTest, VMODouble) {
  Capture c;
  auto ret =
      TestUtils::GetCapture(&c, CaptureLevel::VMO,
                            {
                                .get_processes = {.ret = ZX_OK, .callbacks = {proc_cb, proc2_cb}},
                                .get_property = {proc_prop, proc2_prop},
                                .get_info =
                                    {
                                        self_info,
                                        kmem_info,
                                        kmem_compression_info,
                                        vmos_info,
                                        vmos2_info,
                                    },
                            });
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(2U, c.koid_to_process().size());
  EXPECT_EQ(2U, c.koid_to_vmo().size());

  const auto& process = c.process_for_koid(proc_koid);
  EXPECT_EQ(proc_koid, process.koid);
  EXPECT_STREQ(proc_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(vmo_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo_koid);
  EXPECT_EQ(vmo_koid, vmo.koid);
  EXPECT_STREQ(vmo_name, vmo.name);

  const auto& process2 = c.process_for_koid(proc2_koid);
  EXPECT_EQ(proc2_koid, process2.koid);
  EXPECT_STREQ(proc2_name, process2.name);
  EXPECT_EQ(1U, process2.vmos.size());
  EXPECT_EQ(vmo2_koid, process2.vmos[0]);
  const auto& vmo2 = c.vmo_for_koid(vmo2_koid);
  EXPECT_EQ(vmo2_koid, vmo2.koid);
  EXPECT_STREQ(vmo2_name, vmo2.name);
}

TEST_F(CaptureUnitTest, VMOProcessDuplicate) {
  Capture c;
  auto ret = TestUtils::GetCapture(
      &c, CaptureLevel::VMO,
      {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb}},
       .get_property = {proc_prop},
       .get_info = {self_info, kmem_info, kmem_compression_info, vmos_dup_info, vmos_dup_info}});
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(1U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc_koid);
  EXPECT_EQ(proc_koid, process.koid);
  EXPECT_STREQ(proc_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo_koid);
  EXPECT_EQ(vmo_koid, vmo.koid);
  EXPECT_STREQ(vmo_name, vmo.name);
}

TEST_F(CaptureUnitTest, ProcessPropBadState) {
  // If the process disappears we should ignore it and continue.
  Capture c;
  auto ret = TestUtils::GetCapture(
      &c, CaptureLevel::PROCESS,
      {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb, proc2_cb}},
       .get_property = {{.handle = proc_handle,
                         .property = ZX_PROP_NAME,
                         .value = nullptr,
                         .value_len = 0,
                         .ret = ZX_ERR_BAD_STATE},
                        proc2_prop},
       .get_info = {self_info, kmem_info, kmem_compression_info, vmos2_info, vmos2_info}});
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(1U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc2_koid);
  EXPECT_EQ(proc2_koid, process.koid);
  EXPECT_STREQ(proc2_name, process.name);
}

TEST_F(CaptureUnitTest, VMOCountBadState) {
  // If the process disappears we should ignore it and continue.
  Capture c;
  auto ret =
      TestUtils::GetCapture(&c, CaptureLevel::VMO,
                            {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb, proc2_cb}},
                             .get_property = {proc_prop, proc2_prop},
                             .get_info = {self_info,
                                          kmem_info,
                                          kmem_compression_info,
                                          {.handle = proc_handle,
                                           .topic = ZX_INFO_PROCESS_VMOS,
                                           .values = &_vmo,
                                           .value_size = sizeof(_vmo),
                                           .value_count = 1,
                                           .ret = ZX_ERR_BAD_STATE},
                                          vmos2_info}});
  EXPECT_EQ(ZX_OK, ret);
  // TODO(b/366157407): Decide whether it is fine that StarnixCaptureStrategy returns both
  // processes, given that this initially expected only 1.
  EXPECT_EQ(2U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc2_koid);
  EXPECT_EQ(proc2_koid, process.koid);
  EXPECT_STREQ(proc2_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo2_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo2_koid);
  EXPECT_EQ(vmo2_koid, vmo.koid);
  EXPECT_STREQ(vmo2_name, vmo.name);
}

TEST_F(CaptureUnitTest, VMOGetBadState) {
  // If the process disappears we should ignore it and continue.
  Capture c;
  auto ret =
      TestUtils::GetCapture(&c, CaptureLevel::VMO,
                            {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb, proc2_cb}},
                             .get_property = {proc_prop, proc2_prop},
                             .get_info = {self_info,
                                          kmem_info,
                                          kmem_compression_info,
                                          {.handle = proc_handle,
                                           .topic = ZX_INFO_PROCESS_VMOS,
                                           .values = &_vmo,
                                           .value_size = sizeof(_vmo),
                                           .value_count = 1,
                                           .ret = ZX_ERR_BAD_STATE},
                                          vmos2_info}});
  EXPECT_EQ(ZX_OK, ret);
  EXPECT_EQ(2U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc2_koid);
  EXPECT_EQ(proc2_koid, process.koid);
  EXPECT_STREQ(proc2_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo2_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo2_koid);
  EXPECT_EQ(vmo2_koid, vmo.koid);
  EXPECT_STREQ(vmo2_name, vmo.name);
}

TEST_F(CaptureUnitTest, VMORooted) {
  Capture c;
  TestUtils::CreateCapture(
      &c, {.vmos =
               {
                   {.koid = 1, .name = "R1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                   {.koid = 2, .name = "C1", .size_bytes = 50, .parent_koid = 1},
                   {.koid = 3, .name = "C2", .size_bytes = 25, .parent_koid = 2},
               },
           .processes =
               {
                   {.koid = 10, .name = "p1", .vmos = {1, 2, 3}},
               },
           .rooted_vmo_names = {"R1"}});
  // Carve up the rooted vmo into child and grandchild.
  EXPECT_EQ(50U, c.vmo_for_koid(1).committed_scaled_bytes.integral);
  EXPECT_EQ(25U, c.vmo_for_koid(2).committed_scaled_bytes.integral);
  EXPECT_EQ(25U, c.vmo_for_koid(3).committed_scaled_bytes.integral);
}

TEST_F(CaptureUnitTest, VMORootedPartialCommit) {
  Capture c;
  TestUtils::CreateCapture(
      &c, {.vmos =
               {
                   {.koid = 1, .name = "R1", .committed_bytes = 75, .committed_scaled_bytes = 75},
                   {.koid = 2, .name = "C1", .size_bytes = 77, .parent_koid = 1},
                   {.koid = 3, .name = "C2", .size_bytes = 100, .parent_koid = 2},
               },
           .processes =
               {
                   {.koid = 10, .name = "p1", .vmos = {1, 2, 3}},
               },
           .rooted_vmo_names = {"R1"}});
  // The grandchild should take all available committed bytes from the root.
  EXPECT_EQ(0U, c.vmo_for_koid(1).committed_scaled_bytes.integral);
  EXPECT_EQ(0U, c.vmo_for_koid(2).committed_scaled_bytes.integral);
  EXPECT_EQ(75U, c.vmo_for_koid(3).committed_scaled_bytes.integral);
}

TEST_F(CaptureUnitTest, Compression) {
  const zx_info_kmem_stats_compression_t _kmem_compression_1 = {
      .uncompressed_storage_bytes = 500,
      .compressed_storage_bytes = 100,
  };

  const GetInfoResponse kmem_compression_info_1{.handle = TestUtils::kRootHandle,
                                                .topic = ZX_INFO_KMEM_STATS_COMPRESSION,
                                                .values = &_kmem_compression_1,
                                                .value_size = sizeof(_kmem_compression_1),
                                                .value_count = 1,
                                                .ret = ZX_OK};

  const static zx_info_vmo_t _vmo_compressed{
      .koid = vmo_koid,
      .name = "V1",
      .size_bytes = vmo_size,
      .committed_bytes = 0,
      .populated_bytes = 2 * vmo_size,
      .committed_scaled_bytes = 0,
      .populated_scaled_bytes = 2 * vmo_size,
      .committed_fractional_scaled_bytes = 0,
      .populated_fractional_scaled_bytes = 0,
  };
  const static GetInfoResponse vmos_info_compressed{.handle = proc_handle,
                                                    .topic = ZX_INFO_PROCESS_VMOS,
                                                    .values = &_vmo_compressed,
                                                    .value_size = sizeof(_vmo_compressed),
                                                    .value_count = 1,
                                                    .ret = ZX_OK};

  // Process and VMO need to capture the same info.
  Capture c;
  auto ret = TestUtils::GetCapture(&c, CaptureLevel::VMO,
                                   {.get_processes = {.ret = ZX_OK, .callbacks = {proc_cb}},
                                    .get_property = {proc_prop},
                                    .get_info = {self_info, kmem_info, kmem_compression_info_1,
                                                 vmos_info_compressed, vmos_info_compressed}});
  EXPECT_EQ(ZX_OK, ret);

  EXPECT_EQ(_kmem_compression_1.uncompressed_storage_bytes,
            c.kmem_compression()->uncompressed_storage_bytes);
  EXPECT_EQ(_kmem_compression_1.compressed_storage_bytes,
            c.kmem_compression()->compressed_storage_bytes);

  EXPECT_EQ(1U, c.koid_to_process().size());
  const auto& process = c.process_for_koid(proc_koid);
  EXPECT_EQ(proc_koid, process.koid);
  EXPECT_STREQ(proc_name, process.name);
  EXPECT_EQ(1U, process.vmos.size());
  EXPECT_EQ(1U, c.koid_to_vmo().size());
  EXPECT_EQ(vmo_koid, process.vmos[0]);
  const auto& vmo = c.vmo_for_koid(vmo_koid);
  EXPECT_EQ(vmo_koid, vmo.koid);
  EXPECT_STREQ(vmo_name, vmo.name);
  EXPECT_EQ(2 * vmo_size, vmo.populated_scaled_bytes.integral);
}

}  // namespace memory::test
