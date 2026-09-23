// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Tests for the debuglog.

#include <lib/standalone-test/standalone.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/syscalls/log.h>

#include <zxtest/zxtest.h>

namespace {

TEST(DebugLogTest, WriteRead) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();

  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // Ensure something is written.
  static const char kTestMsg[] = "Debuglog test message.\n";
  ASSERT_OK(zx_debuglog_write(log_handle, 0, kTestMsg, sizeof(kTestMsg)));

  // In-case the read bound isn't respected create a buffer large enough to hopefully
  // prevent corruption.
  char buf[10240]{0};

  // But only report a smaller size for the buffer.
  const size_t read_len = 3;
  const auto status_or_size = zx_debuglog_read(log_handle, 0, buf, read_len);
  ASSERT_EQ(read_len, status_or_size);

  // Ensure that only read_len bytes were written to our buffer.
  const char empty[10]{0};
  ASSERT_EQ(0, memcmp(buf + read_len, empty, sizeof(empty)));

  ASSERT_OK(zx_handle_close(log_handle));
}

TEST(DebugLogTest, InvalidOptions) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();

  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  // Ensure giving invalid options returns an error.
  EXPECT_EQ(zx_debuglog_create(debuglog_resource.get(), 1, &log_handle), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(log_handle, 0);

  EXPECT_EQ(zx_debuglog_create(debuglog_resource.get(), 1 | ZX_LOG_FLAG_READABLE, &log_handle),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(log_handle, 0);
}

TEST(DebugLogTest, InvalidHandleAllowedOnlyForWrite) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();

  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  // Requesting a read-write handle with a valid debuglog resource should work
  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));
  ASSERT_OK(zx_handle_close(log_handle));
  log_handle = ZX_HANDLE_INVALID;

  // Requesting a write-only handle with an invalid resource should work,
  // since the dynamic linker needs to be able to log something somewhere when
  // it can't bootstrap a process
  ASSERT_OK(zx_debuglog_create(ZX_HANDLE_INVALID, 0, &log_handle));
  ASSERT_OK(zx_handle_close(log_handle));
  log_handle = ZX_HANDLE_INVALID;

  // But requesting a read-write handle with an invalid resource represents
  // an information leak, so the kernel checks the first arg, and if it's not a
  // valid handle, then we get an error
  EXPECT_EQ(zx_debuglog_create(ZX_HANDLE_INVALID, ZX_LOG_FLAG_READABLE, &log_handle),
            ZX_ERR_BAD_HANDLE);
}

TEST(DebugLogTest, InvalidResourceHandleReturnsWrongType) {
  zx_handle_t log_handle = ZX_HANDLE_INVALID;
  zx_handle_t event_handle = ZX_HANDLE_INVALID;

  ASSERT_OK(zx_event_create(0, &event_handle));
  ASSERT_NE(event_handle, ZX_HANDLE_INVALID);

  // Passing a valid handle that is not a resource should fail with ZX_ERR_WRONG_TYPE.
  EXPECT_EQ(zx_debuglog_create(event_handle, ZX_LOG_FLAG_READABLE, &log_handle), ZX_ERR_WRONG_TYPE);

  ASSERT_OK(zx_handle_close(event_handle));
}

TEST(DebugLogTest, MaxMessageSize) {
  zx_handle_t log_handle = ZX_HANDLE_INVALID;
  zx::unowned_resource system_resource = standalone::GetSystemResource();

  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // msg is too large and should be truncated.
  char msg[ZX_LOG_RECORD_DATA_MAX + 1];
  memset(msg, 'A', sizeof(msg));
  ASSERT_OK(zx_debuglog_write(log_handle, 0, msg, sizeof(msg)));

  // Use an oversized buffer to ensure that trunction is not caused by our buffer size.
  alignas(zx_log_record_t) char buf[2 * ZX_LOG_RECORD_MAX]{0};
  zx_log_record_t* const record = reinterpret_cast<zx_log_record_t*>(buf);

  // Read until we find our message.
  size_t size;
  do {
    zx_status_t status_or_size = zx_debuglog_read(log_handle, 0, buf, sizeof(buf));
    if (status_or_size < 0) {
      ASSERT_EQ(ZX_ERR_SHOULD_WAIT, status_or_size);
      continue;
    }
    ASSERT_GT(status_or_size, 0);
    size = status_or_size;
    ASSERT_LE(size, ZX_LOG_RECORD_MAX);
    ASSERT_LE(record->datalen, ZX_LOG_RECORD_DATA_MAX);
  } while (memcmp(record->data, msg, record->datalen) != 0);

  // See that the message was truncated to exactly the maximum size.
  ASSERT_EQ(size, ZX_LOG_RECORD_MAX);
  ASSERT_EQ(record->datalen, ZX_LOG_RECORD_DATA_MAX);

  ASSERT_OK(zx_handle_close(log_handle));
}

TEST(DebugLogTest, DrainAndSignal) {
  ZXTEST_SKIP("TODO(https://fxbug.dev/555283122): Re-enable this test when it is stable.");

  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // Drain the log.
  alignas(zx_log_record_t) char buf[ZX_LOG_RECORD_MAX];
  while (true) {
    zx_status_t status = zx_debuglog_read(log_handle, 0, buf, sizeof(buf));
    if (status == ZX_ERR_SHOULD_WAIT) {
      break;
    }
    ASSERT_GT(status, 0);
  }

  // Verify ZX_LOG_READABLE is NOT asserted.
  zx_signals_t pending;
  EXPECT_EQ(zx_object_wait_one(log_handle, ZX_LOG_READABLE, zx_deadline_after(0), &pending),
            ZX_ERR_TIMED_OUT);
  EXPECT_FALSE(pending & ZX_LOG_READABLE);

  // Write a message.
  static const char kMsg[] = "Test message for signaling";
  ASSERT_OK(zx_debuglog_write(log_handle, 0, kMsg, sizeof(kMsg)));

  // Verify ZX_LOG_READABLE IS asserted.
  EXPECT_OK(
      zx_object_wait_one(log_handle, ZX_LOG_READABLE, zx_deadline_after(ZX_SEC(5)), &pending));
  EXPECT_TRUE(pending & ZX_LOG_READABLE);

  // Read it back.
  zx_status_t status = zx_debuglog_read(log_handle, 0, buf, sizeof(buf));
  ASSERT_GT(status, 0);
  zx_log_record_t* record = reinterpret_cast<zx_log_record_t*>(buf);
  ASSERT_EQ(record->datalen, sizeof(kMsg));
  EXPECT_BYTES_EQ(record->data, kMsg, record->datalen);

  // Drain again to clear signal.
  while (true) {
    status = zx_debuglog_read(log_handle, 0, buf, sizeof(buf));
    if (status == ZX_ERR_SHOULD_WAIT) {
      break;
    }
    ASSERT_GT(status, 0);
  }

  // Verify it is no longer readable.
  EXPECT_EQ(zx_object_wait_one(log_handle, ZX_LOG_READABLE, zx_deadline_after(0), &pending),
            ZX_ERR_TIMED_OUT);
  EXPECT_FALSE(pending & ZX_LOG_READABLE);

  ASSERT_OK(zx_handle_close(log_handle));
}

TEST(DebugLogTest, WriteErrors) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // 1. Invalid options
  EXPECT_EQ(zx_debuglog_write(log_handle, 0xFFFFFFFF, "test", 4), ZX_ERR_INVALID_ARGS);

  // 2. No write rights
  zx_handle_t read_only_handle = 0;
  ASSERT_OK(zx_handle_duplicate(log_handle, ZX_RIGHT_READ | ZX_RIGHT_TRANSFER, &read_only_handle));
  EXPECT_EQ(zx_debuglog_write(read_only_handle, 0, "test", 4), ZX_ERR_ACCESS_DENIED);
  ASSERT_OK(zx_handle_close(read_only_handle));

  // 3. Bad pointer
  EXPECT_EQ(zx_debuglog_write(log_handle, 0, nullptr, 4), ZX_ERR_INVALID_ARGS);

  ASSERT_OK(zx_handle_close(log_handle));
}

TEST(DebugLogTest, ReadErrors) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // 1. Invalid options
  char buf[ZX_LOG_RECORD_MAX];
  EXPECT_EQ(zx_debuglog_read(log_handle, 1, buf, sizeof(buf)), ZX_ERR_INVALID_ARGS);

  // 2. No read rights
  zx_handle_t write_only_handle = 0;
  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), 0, &write_only_handle));
  EXPECT_EQ(zx_debuglog_read(write_only_handle, 0, buf, sizeof(buf)), ZX_ERR_ACCESS_DENIED);
  ASSERT_OK(zx_handle_close(write_only_handle));

  // Write a message to log_handle so we have something to read.
  static const char kMsg[] = "Error test msg";
  ASSERT_OK(zx_debuglog_write(log_handle, 0, kMsg, sizeof(kMsg)));

  // 3. Bad pointer (small buffer)
  EXPECT_EQ(zx_debuglog_read(log_handle, 0, nullptr, 16), ZX_ERR_INVALID_ARGS);

  // 4. Bad pointer (large buffer)
  EXPECT_EQ(zx_debuglog_read(log_handle, 0, nullptr, sizeof(buf)), ZX_ERR_INVALID_ARGS);

  ASSERT_OK(zx_handle_close(log_handle));
}

TEST(DebugLogTest, ReadDataCopyFault) {
  zx_handle_t log_handle = 0;
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  zx::result<zx::resource> result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_DEBUGLOG_BASE);
  ASSERT_OK(result.status_value());
  zx::resource debuglog_resource = std::move(result.value());

  ASSERT_OK(zx_debuglog_create(debuglog_resource.get(), ZX_LOG_FLAG_READABLE, &log_handle));

  // Write a message.
  static const char kMsg[] = "Fault test msg";
  ASSERT_OK(zx_debuglog_write(log_handle, 0, kMsg, sizeof(kMsg)));

  // Allocate a VMAR of 2 pages.
  zx::vmar sub_vmar;
  uintptr_t vmar_addr;
  const size_t page_size = zx_system_get_page_size();
  ASSERT_OK(zx::vmar::root_self()->allocate(
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_SPECIFIC, 0, page_size * 2,
      &sub_vmar, &vmar_addr));

  // Create a VMO of 1 page.
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(page_size, 0, &vmo));

  // Map the VMO into the first page of the sub-VMAR.
  uintptr_t map_addr;
  ASSERT_OK(sub_vmar.map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_SPECIFIC, 0, vmo, 0, page_size,
                         &map_addr));
  ASSERT_EQ(map_addr, vmar_addr);

  // Position the destination buffer at the end of the first page.
  void* dst = reinterpret_cast<void*>(vmar_addr + page_size - sizeof(zx_log_record_t));

  EXPECT_EQ(zx_debuglog_read(log_handle, 0, dst, page_size), ZX_ERR_INVALID_ARGS);

  // Clean up.
  ASSERT_OK(sub_vmar.destroy());
  ASSERT_OK(zx_handle_close(log_handle));
}

}  // namespace
