// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_port.h>
#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/util/dlog.h>
#include <lib/magma/util/short_macros.h>

#include <chrono>
#include <thread>

#include <gtest/gtest.h>

#ifdef __Fuchsia__
#include <lib/zx/channel.h>
#include <lib/zx/counter.h>
#include <lib/zx/vmo.h>
#endif

namespace {

enum class SemaphoreType : uint8_t {
  Default,
  Counter,
};

class TestPlatformPort : public testing::Test {
 public:
  void TestSemaphore(SemaphoreType type) {
    std::shared_ptr<magma::PlatformPort> port(magma::PlatformPort::Create());

    std::unique_ptr<std::thread> thread;

    // Verify timeout
    thread.reset(new std::thread([port] {
      DLOG("Waiting for port");
      uint64_t key;
      EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, port->Wait(&key, 100).get());
      DLOG("Port wait returned");
    }));
    thread->join();

    std::shared_ptr<magma::PlatformSemaphore> semaphore;
    switch (type) {
      case SemaphoreType::Default:
        semaphore = std::shared_ptr<magma::PlatformSemaphore>(magma::PlatformSemaphore::Create());
        break;
      case SemaphoreType::Counter: {
#if !defined(__Fuchsia__)
        GTEST_SKIP();
#else
        zx::counter counter;
        ASSERT_EQ(ZX_OK, zx::counter::create(/*options=*/0, &counter));
        semaphore = std::shared_ptr<magma::PlatformSemaphore>(
            magma::PlatformSemaphore::Import(counter.release(), /*flags=*/0));
#endif
        break;
      }
    }
    ASSERT_TRUE(semaphore);

    // Verify WaitAsync/Signal/Reset then Wait flow (no autoreset when waiting on a port)
    semaphore->WaitAsync(port.get(), semaphore->global_id());
    semaphore->Signal();
    semaphore->Reset();
    thread.reset(new std::thread([port, semaphore] {
      DLOG("Waiting for port");
      uint64_t key;
      EXPECT_EQ(MAGMA_STATUS_OK, port->Wait(&key, 100).get());
      EXPECT_EQ(key, semaphore->global_id());
      DLOG("Port wait returned 0x%" PRIx64, semaphore->global_id());
    }));
    thread->join();

    // Verify unsignalled wait - timeout
    thread.reset(new std::thread([port] {
      DLOG("Waiting for port");
      uint64_t key;
      EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, port->Wait(&key, 100).get());
      DLOG("Port wait returned");
    }));
    thread->join();

    // Verify Wait then WaitAsync/Signal/Reset
    thread.reset(new std::thread([port, semaphore] {
      DLOG("Waiting for semaphore");
      uint64_t key;
      EXPECT_EQ(MAGMA_STATUS_OK, port->Wait(&key).get());
      EXPECT_EQ(key, semaphore->global_id());
      DLOG("Semaphore wait returned");
    }));
    std::this_thread::sleep_for(std::chrono::milliseconds(50));
    semaphore->WaitAsync(port.get(), semaphore->global_id());
    semaphore->Signal();
    semaphore->Reset();
    thread->join();

    // TODO(https://fxbug.dev/42105428) - enable: Verify WaitAsync/Wait then kill the handle
#if 0
        semaphore->WaitAsync(port.get());
        thread.reset(new std::thread([port] {
            DLOG("Waiting for semaphore");
            uint64_t key;
            EXPECT_EQ(MAGMA_STATUS_OK, port->Wait(&key).get());
            DLOG("Semaphore wait returned");
        }));
        std::this_thread::sleep_for(std::chrono::milliseconds(50));
        EXPECT_EQ(1u, semaphore.use_count());
        semaphore.reset();
        thread->join();
#endif

    // Verify close
    // TODO(https://fxbug.dev/42105428): test Close after Wait also
    port->Close();
    thread.reset(new std::thread([port, semaphore] {
      DLOG("Waiting for semaphore");
      uint64_t key;
      EXPECT_EQ(MAGMA_STATUS_INTERNAL_ERROR, port->Wait(&key).get());
      DLOG("Semaphore wait returned");
    }));
    thread->join();
  }

  void TestHandle() {
#ifdef __Fuchsia__
    zx::channel local, remote;
    ASSERT_EQ(ZX_OK, zx::channel::create(0 /*flags*/, &local, &remote));

    auto handle = magma::PlatformHandle::Create(local.release());
    ASSERT_TRUE(handle);

    auto port = magma::PlatformPort::Create();
    ASSERT_TRUE(port);

    uint64_t handle_key = handle->global_id();
    EXPECT_TRUE(handle->WaitAsync(port.get(), handle_key));

    uint64_t key;
    EXPECT_EQ(MAGMA_STATUS_TIMED_OUT, port->Wait(&key, 0).get());

    uint32_t dummy = 0;
    EXPECT_EQ(ZX_OK, remote.write(0 /* flags */, &dummy, sizeof(dummy), nullptr /* handles */,
                                  0 /* num_handles*/));

    // Close the peer
    remote.reset();

    EXPECT_EQ(MAGMA_STATUS_OK, port->Wait(&key, 0).get());
    EXPECT_EQ(handle_key, key);

    local.reset(handle->release());

    uint32_t actual_bytes;
    EXPECT_EQ(ZX_OK, local.read(0 /* flags */, &dummy, nullptr /*handles*/, sizeof(dummy),
                                0 /*num_handles*/, &actual_bytes, nullptr /*actual_handles*/));

    handle = magma::PlatformHandle::Create(local.release());

    EXPECT_TRUE(handle->WaitAsync(port.get(), handle->global_id()));

    EXPECT_EQ(MAGMA_STATUS_CONNECTION_LOST, port->Wait(&key, 0).get());
#else
    GTEST_SKIP();
#endif
  }
};

}  // namespace

TEST_F(TestPlatformPort, Semaphore) { TestSemaphore(SemaphoreType::Default); }

TEST_F(TestPlatformPort, CounterSemaphore) { TestSemaphore(SemaphoreType::Counter); }

TEST_F(TestPlatformPort, Handle) { TestHandle(); }
