// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/mock_system_interface.h"

#include "src/developer/debug/debug_agent/mock_component_manager.h"
#include "src/developer/debug/debug_agent/mock_job_handle.h"

namespace debug_agent {

std::unique_ptr<JobHandle> MockSystemInterface::GetRootJob() const {
  return std::make_unique<MockJobHandle>(root_job_);
}

std::unique_ptr<BinaryLauncher> MockSystemInterface::GetLauncher() const {
  // Unimplemented in this mock.
  FX_NOTREACHED();
  return nullptr;
}

std::unique_ptr<JobHandle> MockSystemInterface::AddJob(
    zx_koid_t koid, std::optional<debug_ipc::ComponentInfo> component_info) {
  root_job_.AddChildJob(MockJobHandle(koid));

  if (component_info) {
    mock_component_manager().AddComponentInfo(koid, *component_info);
  }

  return this->GetJob(koid);
}

std::unique_ptr<JobHandle> MockSystemInterface::AddJob(
    MockJobHandle child, zx_koid_t parent_job,
    std::optional<debug_ipc::ComponentInfo> component_info) {
  auto parent = root_job_.FindChildJob(parent_job);
  zx_koid_t child_koid = child.GetKoid();

  if (parent) {
    parent->AddChildJob(std::move(child));

    if (component_info) {
      mock_component_manager().AddComponentInfo(child_koid, *component_info);
    }
  }

  return this->GetJob(child_koid);
}

std::unique_ptr<MockSystemInterface> MockSystemInterface::CreateWithData() {
  // Job 121.
  MockProcessHandle job121_p1(19, "job121-p1");
  job121_p1.set_threads({MockThreadHandle(20, "initial-thread")});

  MockProcessHandle job121_p2(21, "job121-p2");
  job121_p2.set_threads({MockThreadHandle(22, "initial-thread"),
                         MockThreadHandle(23, "second-thread"),
                         MockThreadHandle(24, "third-thread")});

  MockJobHandle job121(18, "job121");
  job121.set_child_processes({job121_p1, job121_p2});

  // Job 12.
  MockJobHandle job12(17, "job12");
  job12.set_child_jobs({job121});

  // Job 11.
  MockProcessHandle job11_p1(14, "job11-p1");
  job11_p1.set_threads(
      {MockThreadHandle(15, "initial-thread"), MockThreadHandle(16, "second-thread")});

  MockJobHandle job11(13, "job11");
  job11.set_child_processes({job11_p1});

  // Job 1
  MockProcessHandle job1_p1(9, "job1-p1");
  job1_p1.set_threads({MockThreadHandle(10, "initial-thread")});

  MockProcessHandle job1_p2(11, "job1-p2");
  job1_p2.set_threads({MockThreadHandle(12, "initial-thread")});

  MockJobHandle job1(8, "job1");
  job1.set_child_processes({job1_p1, job1_p2});
  job1.set_child_jobs({job11, job12});

  // Job 2
  MockProcessHandle job2_p1(26, "job2-p1");
  job2_p1.set_threads({MockThreadHandle(27, "initial-thread")});

  MockJobHandle job2(25, "job2");
  job2.set_child_processes({job2_p1});

  // Job 3
  MockProcessHandle job3_p1(29, "job3-p1");
  job3_p1.set_threads(
      {MockThreadHandle(30, "initial-thread"), MockThreadHandle(31, "second-thread")});
  MockJobHandle job3(28, "job3");
  job3.set_child_processes({job3_p1});

  // Job 4
  // Note: Job4 is contained in a child component under a routing component which contains no ELF
  // executable. We keep no reference of such a component in our view of a running system, but will
  // be notified when these components are discovered. The routing component itself has no
  // associated job, and will not appear as "running" in any view of the system, but it's children
  // could contain ELF executables, which we may want to attach to. See "recursive" filters.
  MockJobHandle job4(32, "job4");
  MockProcessHandle job4_p1(33, "job4-p1");
  job4_p1.set_threads({MockThreadHandle(34, "initial-thread")});
  job4.set_child_processes({job4_p1});

  MockJobHandle job51(38, "job51");
  MockProcessHandle job51_p1(39, "job51-p1");
  job51_p1.set_threads({MockThreadHandle(40, "initial-thread")});
  job51.set_child_processes({job51_p1});

  MockJobHandle job5(35, "job5");
  MockProcessHandle job5_p1(36, "job5-p1");
  job5_p1.set_threads({MockThreadHandle(37, "initial-thread")});
  job5.set_child_processes({job5_p1});
  job5.set_child_jobs({job51});

  MockJobHandle job6(41, "job6");
  MockProcessHandle job6_p1(42, "job6-p1");
  job6_p1.set_threads({MockThreadHandle(43, "initial-thread")});
  job6.set_child_processes({job6_p1});

  MockJobHandle job7(44, "job7");
  MockProcessHandle job7_p1(45, "job7-p1");
  job7_p1.set_threads({MockThreadHandle(46, "initial-thread")});
  job7_p1.set_threads({MockThreadHandle(47, "thread2")});
  job7.set_child_processes({job7_p1});

  MockJobHandle job8(48, "job8");
  MockProcessHandle job8_p1(49, "job8-p1");
  job8_p1.set_threads({MockThreadHandle(50, "initial-thread")});
  job8.set_child_processes({job8_p1});

  // Root.
  MockProcessHandle root_p1(2, "root-p1");
  root_p1.set_threads({MockThreadHandle(3, "initial-thread")});

  MockProcessHandle root_p2(4, "root-p2");
  root_p2.set_threads({MockThreadHandle(5, "initial-thread")});

  MockProcessHandle root_p3(6, "root-p3");
  root_p3.set_threads({MockThreadHandle(7, "initial-thread")});

  MockJobHandle root(1, "root");
  root.set_child_processes({root_p1, root_p2, root_p3});
  root.set_child_jobs({job1, job2, job3, job4, job5, job6, job7, job8});

  auto system_interface = std::make_unique<MockSystemInterface>(std::move(root));

  system_interface->mock_component_manager().AddComponentInfo(
      job1.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "/moniker",
                               .url = "fuchsia-pkg://devhost/package#meta/component.cm"});

  system_interface->mock_component_manager().AddComponentInfo(
      job2.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "/a/long/generated_to_here/fixed/moniker",
                               .url = "fuchsia-pkg://devhost/test_package#meta/component2.cm"});

  system_interface->mock_component_manager().AddComponentInfo(
      job3.GetKoid(), debug_ipc::ComponentInfo{.moniker = "a/generated/moniker:1000",
                                               .url = "fuchsia-boot:///url#meta/cm0.base.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job3.GetKoid(), debug_ipc::ComponentInfo{.moniker = "a/generated/moniker:1001",
                                               .url = "fuchsia-boot:///url#meta/cm1.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job3.GetKoid(), debug_ipc::ComponentInfo{.moniker = "a/generated/moniker:1002",
                                               .url = "fuchsia-boot:///url#meta/cm2.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job3.GetKoid(), debug_ipc::ComponentInfo{.moniker = "bootstrap/hosts:host-1",
                                               .url = "fuchsia-boot:///url#meta/host.cm"});
  system_interface->mock_component_manager().AddNonElfComponentInfo(
      {.moniker = "/moniker/generated/test:test_root",
       .url = "fuchsia-pkg://devhost/root_package#meta/root_component.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job4.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "/moniker/generated/test:test_root/driver",
                               .url = "#meta/child.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job5.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "/some/moniker",
                               .url = "fuchsia-pkg://devhost/package#meta/component3.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job51.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "/some/other/moniker",
                               .url = "fuchsia-pkg://devhost/package#meta/component4.cm"});
  system_interface->mock_component_manager().AddNonElfComponentInfo(
      {.moniker = "moniker/abcdef/test:test_root",
       .url = "fuchsia-pkg://devhost/test_root_package#meta/root_test_component.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job6.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "moniker/abcdef/test:test_root/test_driver",
                               .url = "#meta/test_driver.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job7.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "moniker/abcdef/test:test_root/under_test",
                               .url = "#meta/under_test.cm"});
  system_interface->mock_component_manager().AddComponentInfo(
      job8.GetKoid(),
      debug_ipc::ComponentInfo{.moniker = "moniker/abc123/test:test_root",
                               .url = "fuchsia-pkg://devhost/unittest#meta/unittest.cm"});
  return system_interface;
}

}  // namespace debug_agent
