// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_SYSTEM_INTERFACE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_SYSTEM_INTERFACE_H_

#include <memory>

#include "src/developer/debug/debug_agent/mock_component_manager.h"
#include "src/developer/debug/debug_agent/mock_job_handle.h"
#include "src/developer/debug/debug_agent/mock_limbo_provider.h"
#include "src/developer/debug/debug_agent/system_interface.h"

namespace debug_agent {

class MockSystemInterface final : public SystemInterface {
 public:
  explicit MockSystemInterface(MockJobHandle root_job)
      : root_job_(std::move(root_job)), component_manager_(this) {}

  MockLimboProvider& mock_limbo_provider() { return limbo_provider_; }
  MockComponentManager& mock_component_manager() { return component_manager_; }

  // SystemInterface implementation:
  uint32_t GetNumCpus() const override { return 2; }
  uint64_t GetPhysicalMemory() const override { return 1073741824; }  // 1GB
  std::unique_ptr<JobHandle> GetRootJob() const override;
  std::unique_ptr<BinaryLauncher> GetLauncher() const override;
  ComponentManager& GetComponentManager() override { return component_manager_; }
  LimboProvider& GetLimboProvider() override { return limbo_provider_; }
  std::string GetSystemVersion() override { return "Mock version"; }

  // Adds a new child job to the root job, with the given component info if provided.
  std::unique_ptr<JobHandle> AddJob(zx_koid_t koid,
                                    std::optional<debug_ipc::ComponentInfo> component_info);
  // Same as above but with a specified parent job.
  std::unique_ptr<JobHandle> AddJob(MockJobHandle child, zx_koid_t parent_job,
                                    std::optional<debug_ipc::ComponentInfo> component_info);
  // Creates a default process tree:
  // clang-format off
  //
  //  j: 1 root
  //    p: 2 root-p1
  //      t: 3 initial-thread
  //    p: 4 root-p2
  //      t: 5 initial-thread
  //    p: 6 root-p3
  //      t: 7 initial-thread
  //    j: 8 job1  /moniker  fuchsia-pkg://devhost/package#meta/component.cm
  //      p: 9 job1-p1
  //        t: 10 initial-thread
  //      p: 11 job1-p2
  //        t: 12 initial-thread
  //      j: 13 job11
  //        p: 14 job11-p1
  //          t: 15 initial-thread
  //          t: 16 second-thread
  //      j: 17 job12
  //        j: 18 job121
  //          p: 19 job121-p1
  //            t: 20 initial-thread
  //          p: 21 job121-p2
  //            t: 22 initial-thread
  //            t: 23 second-thread
  //            t: 24 third-thread
  //    j: 25 job2 /a/long/generated_to_here/fixed/moniker fuchsia-pkg://devhost/test_package#meta/component2.cm
  //      p: 26 job2-p1
  //        t: 27 initial-thread
  //    j: 28 job3 <many components, see mock_system_interface.cc>
  //      p: 29 job3-p1 process-host
  //        t: 30 initial-thread
  //        t: 31 second-thread
  //    c: /moniker/generated/test:test_root fuchsia-pkg://devhost/root_package#meta/root_component.cm
  //      j: 32 job4 /moniker/generated/test:test_root/driver #meta/subpackage.cm
  //        p: 33 job4-p1
  //          t: 34 initial-thread
  //    j: 35 job5 /some/moniker fuchsia-pkg://devhost/package#meta/component3.cm
  //      p: 36 job5-p1
  //        t: 37 initial-thread
  //      j: 38 job51 /some/other/moniker fuchsia-pkg://devhost/other_package#meta/component4.cm
  //        p: 39 job51-p1
  //          t: 40 initial-thread
  //    c: moniker/abcdef/test:test_root fuchsia-pkg://devhost/test_root_package#meta/root_test_component.cm
  //      j: 41 job6 moniker/abcdef/test:test_root/test_driver #meta/test_driver.cm
  //        p: 42 job6-p1
  //          t: 43 initial-thread
  //      j: 44 job7 moniker/abcdef/test:test_root/under_test #meta/under_test.cm
  //        p: 45 job7-p1
  //          t: 46 initial-thread
  //          t: 47 thread2
  //    j: 48 job8 moniker/abc123/test:test_root fuchsia-pkg://devhost/unittest#meta/unittest.cm
  //      p: 49 job8-p1
  //        t: 50 initial-thread
  // clang-format-on
  static std::unique_ptr<MockSystemInterface> CreateWithData();

 private:
  MockJobHandle root_job_;
  MockComponentManager component_manager_;
  MockLimboProvider limbo_provider_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_SYSTEM_INTERFACE_H_
