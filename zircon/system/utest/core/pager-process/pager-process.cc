// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/event.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/syscalls.h>

#include <mini-process/mini-process.h>
#include <zxtest/zxtest.h>

#include "test_thread.h"
#include "userpager.h"

// This file contains pager tests that require creating a separate process. For example, testing the
// kill signal requires a process since killing threads directly is not supported. These tests use
// the mini-process interface.

namespace pager_tests {

// Tests killing a thread blocked on a page request from a zx_vmo_read() on a pager-backed VMO.
TEST(PagerProcess, KillBlockedUserVmoRead) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-vmo-read";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Request the mini-process to read from the VMO.
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_READ, vmo->vmo().get()));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Tests killing a thread blocked on a page request from a user mode page fault.
TEST(PagerProcess, KillBlockedUserPageFault) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-user-fault";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(), &addr));

  // Ask the test process to read from the mapped address.
  ASSERT_OK(
      mini_process_cmd_send(cmd_channel, MINIP_CMD_READ_FROM_USER_ADDR, ZX_HANDLE_INVALID, addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Tests killing a thread blocked on a page request from a page fault in kernel mode (usercopy).
TEST(PagerProcess, KillBlockedUsercopy) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-usercopy";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                     zx_system_get_page_size(), &addr));

  // Ask the test process to get_info on another VMO with the mapped address as the user buffer.
  zx::vmo target_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &target_vmo));
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_GET_INFO, target_vmo.get(), addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Same as KillBlockedUsercopy but tests the usercopy version that captures faults and lets the
// caller (in the kernel) resolve it.
TEST(PagerProcess, KillBlockedUsercopyCaptureFaults) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-usercopy-cf";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(), &addr));

  // Ask the test process to do a zx_vmo_write() on another VMO with the mapped address as the user
  // buffer.
  zx::vmo target_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &target_vmo));
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_WRITE, target_vmo.get(), addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Tests killing after a suspending a thread blocked on a page request from a zx_vmo_read() on a
// pager-backed VMO.
TEST(PagerProcess, KillAfterSuspendVmoRead) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-kas-vmo-read";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Request the mini-process to read from the VMO.
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_READ, vmo->vmo().get()));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Suspend the thread.
  zx::suspend_token token;
  ASSERT_OK(thread.suspend(&token));

  // Verify that the thread was indeed suspended.
  ASSERT_OK(thread.wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Tests killing after suspending a thread blocked on a page request from a user mode page fault.
TEST(PagerProcess, KillAfterSuspendPageFault) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-kas-user-fault";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(), &addr));

  // Ask the test process to read from the mapped address.
  ASSERT_OK(
      mini_process_cmd_send(cmd_channel, MINIP_CMD_READ_FROM_USER_ADDR, ZX_HANDLE_INVALID, addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Suspend the thread.
  zx::suspend_token token;
  ASSERT_OK(thread.suspend(&token));

  // Verify that the thread was indeed suspended.
  ASSERT_OK(thread.wait_one(ZX_THREAD_SUSPENDED, zx::time::infinite(), nullptr));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Tests killing after a pending suspend on a thread blocked on a page request from a page fault in
// kernel mode (usercopy).
TEST(PagerProcess, KillAfterSuspendUsercopy) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-kas-usercopy";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, vmo->vmo(), 0,
                     zx_system_get_page_size(), &addr));

  // Ask the test process to get_info on another VMO with the mapped address as the user buffer.
  zx::vmo target_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &target_vmo));
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_GET_INFO, target_vmo.get(), addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Suspend the thread.
  zx::suspend_token token;
  ASSERT_OK(thread.suspend(&token));

  // Since the thread is blocked in the kernel, it won't be suspended. Wait a bit and verify that
  // the thread is still blocked. We're testing for inability to suspend, so the best we can do is
  // allow some time for the suspend to go through.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

// Same as KillAfterSuspendUsercopy but tests the usercopy version that captures faults and lets the
// caller (in the kernel) resolve it.
TEST(PagerProcess, KillAfterSuspendUsercopyCapture) {
  zx::process proc;
  zx::thread thread;
  zx::vmar vmar;

  // mini-process setup
  constexpr const char kTestName[] = "pager-kas-usercopy-cf";
  zx::unowned<zx::job> job = zx::job::default_job();
  ASSERT_OK(zx::process::create(*job, kTestName, sizeof(kTestName) - 1, 0, &proc, &vmar));
  ASSERT_OK(zx::thread::create(proc, kTestName, sizeof(kTestName) - 1, 0u, &thread));

  zx::event event;
  ASSERT_OK(zx::event::create(0u, &event));

  zx_handle_t cmd_channel;
  EXPECT_OK(start_mini_process_etc(proc.get(), thread.get(), vmar.get(), event.get(), true,
                                   &cmd_channel));

  UserPager pager;
  ASSERT_TRUE(pager.Init());

  // Create a pager-backed VMO.
  Vmo* vmo;
  ASSERT_TRUE(pager.CreateVmo(1, &vmo));

  // Map the pager-backed VMO in the test process' root VMAR.
  zx_vaddr_t addr;
  ASSERT_OK(vmar.map(ZX_VM_PERM_READ, 0, vmo->vmo(), 0, zx_system_get_page_size(), &addr));

  // Ask the test process to do a zx_vmo_write() on another VMO with the mapped address as the user
  // buffer.
  zx::vmo target_vmo;
  ASSERT_OK(zx::vmo::create(zx_system_get_page_size(), 0u, &target_vmo));
  ASSERT_OK(mini_process_cmd_send(cmd_channel, MINIP_CMD_VMO_WRITE, target_vmo.get(), addr));

  // Wait for the page request and verify that the thread is blocked.
  ASSERT_TRUE(pager.WaitForPageRead(vmo, 0, 1, ZX_TIME_INFINITE));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Suspend the thread.
  zx::suspend_token token;
  ASSERT_OK(thread.suspend(&token));

  // Since the thread is blocked in the kernel, it won't be suspended. Wait a bit and verify that
  // the thread is still blocked. We're testing for inability to suspend, so the best we can do is
  // allow some time for the suspend to go through.
  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  ASSERT_TRUE(TestThread::WaitForBlockedOnPager(thread));

  // Now kill the process which will also kill the contained thread.
  ASSERT_OK(proc.kill());

  // Verify that the thread was indeed terminated.
  ASSERT_OK(thread.wait_one(ZX_TASK_TERMINATED, zx::time::infinite(), nullptr));

  // Cleanup
  EXPECT_EQ(mini_process_cmd(cmd_channel, MINIP_CMD_EXIT_NORMAL, nullptr), ZX_ERR_PEER_CLOSED);
}

}  // namespace pager_tests
