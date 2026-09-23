// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/capture_strategy.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/tests/test_utils.h"

using testing::AllOf;
using testing::Field;
using testing::Matcher;
using testing::Pair;
using testing::UnorderedElementsAre;
using testing::UnorderedElementsAreArray;

namespace memory {
// Printers for a nicer output when expectations fail.
extern std::ostream& operator<<(std::ostream& os, const std::vector<zx_koid_t>& vmos) {
  os << "[";
  for (const auto& koid : vmos) {
    os << koid << ", ";
  }
  os << "]";
  return os;
}

extern std::ostream& operator<<(std::ostream& os, const Process& proc) {
  os << "Process{.job: " << proc.job << ", .koid: " << proc.koid << ", .name: " << proc.name
     << ", .vmos: " << proc.vmos << "}";
  return os;
}

extern std::ostream& operator<<(std::ostream& os, const zx_info_vmo_t& vmo) {
  os << "zx_info_vmo_t{.koid: " << vmo.koid << ", .parent_koid: " << vmo.parent_koid
     << ", .name: " << vmo.name << ", .committed_bytes: " << vmo.committed_bytes << "}";
  return os;
}

extern std::ostream& operator<<(std::ostream& os, const Vmo& vmo) {
  os << "Vmo{.koid: " << vmo.koid << ", .parent_koid: " << vmo.parent_koid
     << ", .name: " << vmo.name
     << ", .committed_scaled_bytes: " << vmo.committed_scaled_bytes.integral
     << ", .allocated_bytes: " << vmo.allocated_bytes << ", .children: " << vmo.children << "}";
  return os;
}

namespace test {

namespace {
const zx_koid_t koid_job_0 = 10;
const zx_koid_t koid_process_0 = 100;
const zx_handle_t handle_process_0 = 1000;
const char kProcessName_0[] = "proc_0";

const zx_koid_t koid_job_1 = 11;
const zx_koid_t koid_process_1 = 101;
const zx_handle_t handle_process_1 = 1001;
const char kProcessName_1[] = "proc_1";

const zx_koid_t koid_vmo_0 = 200;

const zx_info_vmo_t _vmo_0{
    .koid = koid_vmo_0,
    .name = "vmo_0",
    .size_bytes = 0,
};
const GetInfoResponse vmo_0_info{.handle = handle_process_0,
                                 .topic = ZX_INFO_PROCESS_VMOS,
                                 .values = &_vmo_0,
                                 .value_size = sizeof(_vmo_0),
                                 .value_count = 1,
                                 .ret = ZX_OK};

const zx_koid_t koid_vmo_1 = 201;

const zx_info_vmo_t _vmo_1{
    .koid = koid_vmo_1,
    .name = "vmo_0",
    .size_bytes = 0,
};
const GetInfoResponse vmo_1_info{.handle = handle_process_1,
                                 .topic = ZX_INFO_PROCESS_VMOS,
                                 .values = &_vmo_1,
                                 .value_size = sizeof(_vmo_1),
                                 .value_count = 1,
                                 .ret = ZX_OK};

const zx_koid_t koid_job_2 = 12;
const zx_koid_t koid_process_2 = 102;
const zx_handle_t handle_process_2 = 1002;

const zx_koid_t koid_vmo_2 = 202;
const zx_vaddr_t addr_vmo_2 = 0x402000100000;

const zx_koid_t koid_vmo_2b = 212;
// VMO 2B is not mapped.

const zx_koid_t koid_job_3 = koid_job_2;
const zx_koid_t koid_process_3 = 103;
const zx_handle_t handle_process_3 = 1003;
const char kProcessName_3[] = "proc_3";

const zx_koid_t koid_vmo_3 = 203;
const zx_vaddr_t addr_vmo_3 = 0x100002;

const zx_info_vmo_t _vmo_2[]{{
                                 .koid = koid_vmo_2,
                                 .name = "vmo_2",
                                 .size_bytes = 0,
                             },
                             {
                                 .koid = koid_vmo_2b,
                                 .name = "vmo_2b",
                                 .size_bytes = 0,
                             },
                             {
                                 .koid = koid_vmo_3,
                                 .name = "vmo_3",
                                 .size_bytes = 0,
                             }};
const GetInfoResponse vmo_2_info{.handle = handle_process_2,
                                 .topic = ZX_INFO_PROCESS_VMOS,
                                 .values = _vmo_2,
                                 .value_size = sizeof(zx_info_vmo_t),
                                 .value_count = 3,
                                 .ret = ZX_OK};

const zx_info_maps_t _mappings_2[]{{
                                       .base = 0,
                                       .size = 0x400000000000,
                                       .depth = 1,
                                       .type = ZX_INFO_MAPS_TYPE_VMAR,
                                   },
                                   {
                                       .base = 0x400000000000,
                                       .size = 0x400000000000,
                                       .depth = 1,
                                       .type = ZX_INFO_MAPS_TYPE_VMAR,
                                   },
                                   {
                                       .base = addr_vmo_2,
                                       .type = ZX_INFO_MAPS_TYPE_MAPPING,
                                       .u = {.mapping = {.vmo_koid = koid_vmo_2}},
                                   }};
const GetInfoResponse maps_2_info{.handle = handle_process_2,
                                  .topic = ZX_INFO_PROCESS_MAPS,
                                  .values = _mappings_2,
                                  .value_size = sizeof(zx_info_maps_t),
                                  .value_count = 3,
                                  .ret = ZX_OK};

// |vmo_3_info| should contain the same VMOs as vmo_2_info per the shared handle table of shared
// processes used by Starnix.
const GetInfoResponse vmo_3_info{.handle = handle_process_3,
                                 .topic = ZX_INFO_PROCESS_VMOS,
                                 .values = _vmo_2,
                                 .value_size = sizeof(zx_info_vmo_t),
                                 .value_count = 3,
                                 .ret = ZX_OK};
const zx_info_maps_t _mappings_3[]{{
                                       .base = 0,
                                       .size = 0x400000000000,
                                       .depth = 1,
                                       .type = ZX_INFO_MAPS_TYPE_VMAR,
                                   },
                                   {
                                       .base = 0x400000000000,
                                       .size = 0x400000000000,
                                       .depth = 1,
                                       .type = ZX_INFO_MAPS_TYPE_VMAR,
                                   },
                                   {
                                       .base = addr_vmo_2,
                                       .type = ZX_INFO_MAPS_TYPE_MAPPING,
                                       .u = {.mapping = {.vmo_koid = koid_vmo_2}},
                                   },
                                   {
                                       .base = addr_vmo_3,
                                       .type = ZX_INFO_MAPS_TYPE_MAPPING,
                                       .u = {.mapping = {.vmo_koid = koid_vmo_3}},
                                   }};
const GetInfoResponse maps_3_info{.handle = handle_process_3,
                                  .topic = ZX_INFO_PROCESS_MAPS,
                                  .values = _mappings_3,
                                  .value_size = sizeof(zx_info_maps_t),
                                  .value_count = 4,
                                  .ret = ZX_OK};

Matcher<Process> MakeProcessMatcher(zx_koid_t process, zx_koid_t job, const std::string& name,
                                    const std::vector<zx_koid_t>& vmos) {
  return AllOf(Field(&Process::koid, process), Field(&Process::job, job),
               Field(&Process::name, testing::StrEq(name)),
               Field(&Process::vmos, UnorderedElementsAreArray(vmos)));
}

Matcher<Vmo> MakeVmoMatcher(zx_info_vmo_t vmo) {
  return AllOf(Field(&Vmo::koid, vmo.koid), Field(&Vmo::name, testing::StrEq(vmo.name)));
}
}  // namespace

using StarnixCaptureStrategyTest = testing::Test;

TEST_F(StarnixCaptureStrategyTest, NoStarnixProcess) {
  MockOS os({.get_info = {vmo_0_info, vmo_1_info}});
  StarnixCaptureStrategy strategy;

  memory::Process process_0{.koid = koid_process_0, .job = koid_job_0};
  std::strncpy(process_0.name, kProcessName_0, ZX_MAX_NAME_LEN);
  zx::handle handle_0(handle_process_0);

  memory::Process process_1{.koid = koid_process_1, .job = koid_job_1};
  std::strncpy(process_1.name, kProcessName_1, ZX_MAX_NAME_LEN);
  zx::handle handle_1(handle_process_1);

  strategy.OnNewProcess(os, std::move(process_0), std::move(handle_0));
  strategy.OnNewProcess(os, std::move(process_1), std::move(handle_1));
  auto result = StarnixCaptureStrategy::Finalize(os, std::move(strategy));
  EXPECT_TRUE(result.is_ok());

  auto& [koid_to_process, koid_to_vmo] = result.value();

  EXPECT_THAT(
      koid_to_process,
      UnorderedElementsAre(Pair(koid_process_0, MakeProcessMatcher(koid_process_0, koid_job_0,
                                                                   kProcessName_0, {koid_vmo_0})),
                           Pair(koid_process_1, MakeProcessMatcher(koid_process_1, koid_job_1,
                                                                   kProcessName_1, {koid_vmo_1}))));

  EXPECT_THAT(koid_to_vmo, UnorderedElementsAre(Pair(koid_vmo_0, MakeVmoMatcher(_vmo_0)),
                                                Pair(koid_vmo_1, MakeVmoMatcher(_vmo_1))));
}

TEST_F(StarnixCaptureStrategyTest, WithStarnixProcess) {
  MockOS os(
      {.get_info = {vmo_0_info, vmo_1_info, vmo_2_info, maps_2_info, vmo_3_info, maps_3_info}});
  StarnixCaptureStrategy strategy;

  memory::Process process_0{.koid = koid_process_0, .job = koid_job_0};
  std::strncpy(process_0.name, kProcessName_0, ZX_MAX_NAME_LEN);
  zx::handle handle_0(handle_process_0);

  memory::Process process_1{.koid = koid_process_1, .job = koid_job_1};
  std::strncpy(process_1.name, kProcessName_1, ZX_MAX_NAME_LEN);
  zx::handle handle_1(handle_process_1);

  memory::Process process_2{.koid = koid_process_2, .job = koid_job_2};
  std::strncpy(process_2.name, STARNIX_KERNEL_PROCESS_NAME, ZX_MAX_NAME_LEN);
  zx::handle handle_2(handle_process_2);

  memory::Process process_3{.koid = koid_process_3, .job = koid_job_3};
  std::strncpy(process_3.name, kProcessName_3, ZX_MAX_NAME_LEN);
  zx::handle handle_3(handle_process_3);

  strategy.OnNewProcess(os, std::move(process_0), std::move(handle_0));
  strategy.OnNewProcess(os, std::move(process_1), std::move(handle_1));
  strategy.OnNewProcess(os, std::move(process_2), std::move(handle_2));
  strategy.OnNewProcess(os, std::move(process_3), std::move(handle_3));
  auto result = StarnixCaptureStrategy::Finalize(os, std::move(strategy));
  EXPECT_TRUE(result.is_ok());

  auto& [koid_to_process, koid_to_vmo] = result.value();

  EXPECT_THAT(
      koid_to_process,
      UnorderedElementsAre(Pair(koid_process_0, MakeProcessMatcher(koid_process_0, koid_job_0,
                                                                   kProcessName_0, {koid_vmo_0})),
                           Pair(koid_process_1, MakeProcessMatcher(koid_process_1, koid_job_1,
                                                                   kProcessName_1, {koid_vmo_1})),
                           Pair(koid_process_2, MakeProcessMatcher(koid_process_2, koid_job_2,
                                                                   STARNIX_KERNEL_PROCESS_NAME,
                                                                   {koid_vmo_2, koid_vmo_2b})),
                           Pair(koid_process_3, MakeProcessMatcher(koid_process_3, koid_job_3,
                                                                   kProcessName_3, {koid_vmo_3}))));

  EXPECT_THAT(koid_to_vmo, UnorderedElementsAre(Pair(koid_vmo_0, MakeVmoMatcher(_vmo_0)),
                                                Pair(koid_vmo_1, MakeVmoMatcher(_vmo_1)),
                                                Pair(koid_vmo_2, MakeVmoMatcher(_vmo_2[0])),
                                                Pair(koid_vmo_2b, MakeVmoMatcher(_vmo_2[1])),
                                                Pair(koid_vmo_3, MakeVmoMatcher(_vmo_2[2]))));
}

}  // namespace test
}  // namespace memory
