// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/debuglog.h>
#include <lib/kconcurrent/chainlock_transaction.h>
#include <lib/unittest/unittest.h>
#include <lib/zircon-internal/macros.h>
#include <zircon/types.h>

#include <ktl/string_view.h>
#include <ktl/unique_ptr.h>

#include "debuglog_internal.h"

#include <ktl/enforce.h>

namespace {
struct DLogOutputTest final : public DLog {
  AutounsignalEvent output;
  // Buffer to hold last rendered log - size = dlog max data + additional buffer for log header.
  char output_log_buffer[DLOG_MAX_DATA + 128] TA_GUARDED(output_lock);
  ktl::string_view last_output_log TA_GUARDED(output_lock);
  DECLARE_LOCK(DLogOutputTest, Mutex) output_lock;

 protected:
  void OutputLogMessage(ktl::string_view log) override {
    {
      Guard<Mutex> output_guard(&output_lock);
      memcpy(output_log_buffer, log.data(), log.length());
      last_output_log = {output_log_buffer, log.length()};
    }
    output.Signal();
  }
};
}  // namespace

struct DebuglogTests {
  static bool log_format() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check(), "");

    char msg[] = "Hello World";

    log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)});

    // log->data_[0] is not necessarily aligned as a dlog_header would be so copy rather than
    // access them directly.
    dlog_header header{};
    static_assert(sizeof(log->data_) >= sizeof(header));
    memcpy(&header, log->data_, sizeof(header));

    EXPECT_EQ(DEBUGLOG_WARNING, header.severity);
    EXPECT_EQ(0, header.flags);
    EXPECT_EQ(ZX_KOID_INVALID, header.pid);
    EXPECT_NE(ZX_KOID_INVALID, header.tid);
    ASSERT_EQ(12, header.datalen);

    uint8_t* msg_bytes = log->data_ + sizeof(dlog_header);

    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(msg), msg_bytes, sizeof(msg));

    END_TEST;
  }

  static bool log_wrap() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check(), "");

    char msg[] = "Hello World";
    const size_t kTruncateTarget =
        kDLogHeaderFifoAlignment + 1;  // An unaligned length somewhere in the middle of msg.
    const size_t pad = DLOG_ALIGN_TRUNC(DLOG_SIZE - sizeof(dlog_header) - kTruncateTarget);

    // We need to trigger some writes, but we don't care what we write, so we just need a buffer
    // large enough to be copied from. Doesn't much matter what's in it.
    char ignored_buffer[DLOG_MAX_DATA]{0};

    for (size_t to_write = pad; to_write;) {
      size_t write = to_write - sizeof(dlog_header);
      write = write > DLOG_MAX_DATA ? DLOG_MAX_DATA : write;

      log->Write(DEBUGLOG_WARNING, 0, {ignored_buffer, write});
      to_write -= write + sizeof(dlog_header);
    }

    {
      Guard<MonitoredSpinLock, IrqSave> guard{&log->lock_, SOURCE_TAG};
      EXPECT_EQ(pad, log->head_);
    }
    log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)});

    // log->data_[pad] is not necessarily aligned as a dlog_header would be so copy rather than
    // access them directly.
    dlog_header header{};
    static_assert(sizeof(log->data_) - pad >= sizeof(header));
    memcpy(&header, log->data_ + pad, sizeof(header));

    EXPECT_EQ(DEBUGLOG_WARNING, header.severity);
    EXPECT_EQ(0, header.flags);
    EXPECT_EQ(ZX_KOID_INVALID, header.pid);
    EXPECT_NE(ZX_KOID_INVALID, header.tid);
    ASSERT_EQ(sizeof(msg), header.datalen);

    uint8_t* msg_bytes = log->data_ + pad + sizeof(dlog_header);

    const size_t kTruncatedSize = DLOG_SIZE - pad - sizeof(dlog_header);

    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(&msg[0]), msg_bytes, kTruncatedSize);
    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(&msg[8]), log->data_, sizeof(msg) - kTruncatedSize);

    END_TEST;
  }

  // Read a record from the debuglog and verify its fields.
  static bool log_reader_read() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check());

    const zx_instant_boot_t now = current_boot_time();

    char msg[] = "Message!";
    ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)}));

    DlogReader reader;
    reader.Initialize(nullptr, nullptr, log.get());
    size_t got;
    dlog_record_t rec{};
    ASSERT_EQ(ZX_OK, reader.Read(0, &rec, &got));
    ASSERT_EQ(sizeof(dlog_header) + sizeof(msg), got);
    EXPECT_EQ(0u, rec.hdr.preamble);
    // Sequence should start at 0.
    EXPECT_EQ(0u, rec.hdr.sequence);
    EXPECT_EQ(sizeof(msg), rec.hdr.datalen);
    EXPECT_EQ(DEBUGLOG_WARNING, rec.hdr.severity);
    EXPECT_EQ(0, rec.hdr.flags);
    EXPECT_GE(rec.hdr.timestamp, now);
    EXPECT_EQ(0ull, rec.hdr.pid);
    EXPECT_EQ(Thread::Current::Get()->tid(), rec.hdr.tid);

    reader.Disconnect();

    END_TEST;
  }

  // Write to the log, exceeding its capacity and see that data is lost.
  static bool log_reader_dataloss() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check(), "");

    DlogReader reader;
    reader.Initialize(nullptr, nullptr, log.get());

    char msg[] = "Hello World";

    uint64_t num_written = 0;

    log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)});
    num_written++;

    dlog_record_t rec{};
    size_t got;

    ASSERT_EQ(ZX_OK, reader.Read(0, &rec, &got));
    ASSERT_EQ(sizeof(dlog_header) + sizeof(msg), got);

    EXPECT_EQ(DEBUGLOG_WARNING, rec.hdr.severity);
    EXPECT_EQ(0u, rec.hdr.preamble);
    EXPECT_EQ(0, rec.hdr.flags);
    EXPECT_EQ(ZX_KOID_INVALID, rec.hdr.pid);
    EXPECT_NE(ZX_KOID_INVALID, rec.hdr.tid);
    ASSERT_EQ(sizeof(msg), rec.hdr.datalen);

    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(msg), reinterpret_cast<uint8_t*>(rec.data),
                    sizeof(msg));

    for (size_t i = 0; i < DLOG_SIZE; i += sizeof(dlog_header) + sizeof(msg)) {
      log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)});
      num_written++;
    }

    uint64_t num_read = 0;
    zx_status_t status;
    uint64_t expected_sequence = 0;
    uint64_t dropped = 0;
    while ((status = reader.Read(0, &rec, &got)) == ZX_OK) {
      num_read++;
      ASSERT_EQ(sizeof(dlog_header) + sizeof(msg), got);
      EXPECT_EQ(DEBUGLOG_WARNING, rec.hdr.severity);
      EXPECT_EQ(0u, rec.hdr.preamble);
      EXPECT_EQ(0, rec.hdr.flags);
      EXPECT_EQ(ZX_KOID_INVALID, rec.hdr.pid);
      EXPECT_NE(ZX_KOID_INVALID, rec.hdr.tid);
      ASSERT_EQ(sizeof(msg), rec.hdr.datalen);
      EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(msg), reinterpret_cast<uint8_t*>(rec.data),
                      sizeof(msg));
      dropped += (rec.hdr.sequence - expected_sequence);
      expected_sequence = rec.hdr.sequence + 1;
    }
    EXPECT_EQ(ZX_ERR_SHOULD_WAIT, status);

    // See that we read fewer records than we wrote.
    EXPECT_LT(num_read, num_written);

    EXPECT_EQ(0, rec.hdr.flags);
    EXPECT_EQ(ZX_KOID_INVALID, rec.hdr.pid);
    EXPECT_NE(ZX_KOID_INVALID, rec.hdr.tid);
    ASSERT_EQ(sizeof(msg), rec.hdr.datalen);

    EXPECT_BYTES_EQ(reinterpret_cast<uint8_t*>(msg), reinterpret_cast<uint8_t*>(rec.data),
                    sizeof(msg));

    // See that all messages are acconted for.  That is, the number dropped matches the differnce.
    EXPECT_EQ(dropped, num_written - num_read);

    reader.Disconnect();
    END_TEST;
  }

  // Verify that logs written are output correctly by the DumperThread.
  static bool log_dumper_test() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLogOutputTest> log = ktl::make_unique<DLogOutputTest>(&ac);
    ASSERT_TRUE(ac.check());

    // Start the dumper thread.
    log->StartThreads();

    // This is the size of the header.
    const size_t min_output_size = []() {
      dlog_header_t empty_hdr;
      memset(&empty_hdr, 0, sizeof(empty_hdr));
      return DLog::FormatHeader({}, empty_hdr);
    }();

    // A small helper which will count the number of occurrences of |msg| in
    // |str|.
    auto CountOccurences = [min_output_size](const ktl::string_view& str,
                                             const ktl::string_view& msg) -> size_t {
      size_t count = 0;
      size_t offset = min_output_size;
      while ((offset = str.find(msg, offset)) != str.npos) {
        ++count;
        ++offset;
      }
      return count;
    };

    // Helper function to verify that a message is output correctly.
    auto write_log_and_check_output = [&log, &all_ok, min_output_size,
                                       &CountOccurences](char* msg) -> bool {
      auto len = strlen(msg);
      ASSERT_OK(log->Write(DEBUGLOG_INFO, 0, {msg, len}));
      // Wait for the log to get output by the DumperThread.
      ASSERT_OK(log->output.Wait());

      Guard<Mutex> output_guard(&log->output_lock);
      // Length of output log is at least header length + message length.
      EXPECT_GE(log->last_output_log.length(), min_output_size + len);
      ASSERT_GT(log->last_output_log.length(), 0lu);
      // Check that the log ends with a newline.
      EXPECT_EQ('\n', log->last_output_log[log->last_output_log.length() - 1]);
      // Check that the log contains the message (if not empty) after the header.
      if (len) {
        ASSERT_EQ(1u, CountOccurences(log->last_output_log, msg));
      }
      return all_ok;
    };

    // Check that a simple message appears in the log dump.
    char msg0[] = "Hello World!\n";
    EXPECT_TRUE(write_log_and_check_output(msg0));

    // Check that a message without newline appears in the log dump and contains newline.
    char msg1[] = "Hello!";
    EXPECT_TRUE(write_log_and_check_output(msg1));

    // Check that a message containing only newline in the log dump.
    char msg2[] = "\n";
    EXPECT_TRUE(write_log_and_check_output(msg2));

    // Check that an empty log still gets rendered and contains newline.
    char msg3[] = "";
    EXPECT_TRUE(write_log_and_check_output(msg3));

    log->Shutdown(ZX_TIME_INFINITE);

    END_TEST;
  }

  static bool render_to_crashlog() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check());

    // Define our test message and figure out a expected minimum size for the
    // message when rendered. While we are not 100% sure how large the header
    // for a given rendered record will be, we we know that a record with a
    // timestamp, pid, and tid of zero will have the smallest rendered header
    // possible.  Note that our test message does not end with a newline, but we
    // expect rendering to add one to each record which does not end with a
    // newline.
    const ktl::string_view msg{"Message!"};
    const size_t min_rendered_size = [&msg]() {
      dlog_header_t empty_hdr;
      memset(&empty_hdr, 0, sizeof(empty_hdr));
      return DLog::FormatHeader({}, empty_hdr) + msg.size() + 1;
    }();

    // Allocate two targets for rendering data into, one normal sized, and one
    // small.  Using a dedicated small buffer (instead of simply artificially
    // limiting the length of a span wrapped around the large buffer) should
    // help ASAN in detecting any buffer overflows.
    constexpr size_t kLargeRenderBufferSize = 1024;
    ktl::unique_ptr<char[]> large_storage{new (&ac) char[kLargeRenderBufferSize]};
    ASSERT_TRUE(ac.check());
    ASSERT_LE(min_rendered_size, kLargeRenderBufferSize);

    constexpr size_t kSmallRenderBufferSize = 1;
    ktl::unique_ptr<char[]> small_storage{new (&ac) char[kSmallRenderBufferSize]};
    ASSERT_TRUE(ac.check());

    ktl::span<char> large_target{large_storage.get(), kLargeRenderBufferSize};
    ktl::span<char> small_target{small_storage.get(), kSmallRenderBufferSize};

    // A small helper which will count the number of occurrences of |msg| in
    // |str|.
    auto CountOccurences = [](const ktl::string_view& str, const ktl::string_view& msg) -> size_t {
      size_t count = 0;
      size_t offset = 0;
      while ((offset = str.find(msg, offset)) != str.npos) {
        ++count;
        ++offset;
      }
      return count;
    };

    // A small helper which renders a debuglog into a span provided, and gives
    // back a string view of the result.
    auto DoRender = [](const DLog& log, ktl::span<char> target) -> ktl::string_view {
      return ktl::string_view{target.data(), log.RenderToCrashlog(target)};
    };

    // Rendering from an empty log should produce nothing.
    ktl::string_view rendered;
    rendered = DoRender(*log, large_target);
    ASSERT_EQ(0u, rendered.size());

    // Add record to the log, then render the log and verify that it contains at
    // least a minimum number of expected bytes, and exactly one occurrence of
    // the test string in the buffer.
    ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, msg));
    rendered = DoRender(*log, large_target);
    ASSERT_GE(rendered.size(), min_rendered_size);
    ASSERT_EQ(1u, CountOccurences(rendered, msg));

    // Attempting to render into an empty target from a log with valid records
    // in should produce no data.
    rendered = DoRender(*log, {});
    ASSERT_EQ(0u, rendered.size());

    // Attempting to render into target with is non-empty, but does not have
    // enough space to hold a rendered record, should also produce no data.
    rendered = DoRender(*log, small_target);
    ASSERT_EQ(0u, rendered.size());

    // Add two more instances of the test message and re-validate
    ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, msg));
    ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, msg));
    rendered = DoRender(*log, large_target);
    ASSERT_GE(rendered.size(), min_rendered_size * 3);
    ASSERT_EQ(3u, CountOccurences(rendered, msg));

    // Figure out how many instances of our message _should_ fit in the dlog
    // buffer, then add that many, forcing the log to wrap in the process.  Note
    // that records are always padded out to a 4 bytes boundary.
    const size_t kRecordSize = (msg.size() + sizeof(dlog_header_t) + 0x3) & ~0x3;
    const size_t kRecordCount = DLOG_SIZE / kRecordSize;
    for (size_t i = 0; i < kRecordCount; ++i) {
      ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, msg));
    }

    // It is not clear exactly how many records should fit into our render
    // buffer at this point, but the number should be more than 3.
    rendered = DoRender(*log, large_target);
    ASSERT_GT(rendered.size(), min_rendered_size * 3);
    ASSERT_GT(CountOccurences(rendered, msg), 3u);

    END_TEST;
  }

  // This is a regression test introduced after discovering https://fxbug.dev/42071359.  It
  // attempt to make sure that the internal methods ReadRecord and
  // ReassembleFromOffset return proper results, even when headers and payload
  // span the break in the ring buffer.
  static bool read_record() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check());

    const char kPayloadData[] = {
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b,
        0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16,
        0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f, 0x20,
    };
    const ktl::string_view kPayload{kPayloadData, ktl::size(kPayloadData)};
    const size_t kPayloadSize = sizeof(kPayloadData);
    const size_t kTotalRecordSize = sizeof(dlog_header_t) + kPayloadSize;

    constexpr uint8_t kTestSeverity = 0xba;
    constexpr uint8_t kTestFlags = 0xad;
    const uint64_t our_tid = Thread::Current::Get()->tid();

    static_assert(
        (kPayloadSize % 4) == 0,
        "Payload data must be a multiple of 4 bytes in order to be able to align its end exactly "
        "with the end of the ring buffer");

    constexpr ktl::array kOffsets = {
        static_cast<size_t>(0),                             // Start of the buffer
        DLOG_SIZE - kTotalRecordSize,                       // Aligned with end
        DLOG_SIZE - kTotalRecordSize + (kPayloadSize / 2),  // Splitting the payload
        DLOG_SIZE - (sizeof(dlog_header_t) / 2),            // Splitting the header
    };

    for (const size_t offset : kOffsets) {
      Guard<MonitoredSpinLock, IrqSave> guard{&log->lock_, SOURCE_TAG};

      // Set up the logs ring buffer so that it looks like it is empty, but
      // ready to write the next record starting at the test vector offset.
      // Also, zero out our entire ring buffer just so we are starting from a
      // known state with each vector.
      ASSERT_TRUE((offset % 4) == 0);  // Records must start on 4 byte boundaries.
      ASSERT_LT(offset, DLOG_SIZE);
      log->head_ = log->tail_ = offset;
      memset(log->data_, 0, sizeof(log->data_));

      // Go ahead and write our record, then confirm that head and tail end up
      // where we expect them to.  Note that these pointers are always absolute
      // pointers, to compute offsets from them, one needs to & with the
      // DLOG_MASK.
      zx_status_t status = ZX_ERR_INTERNAL;

      const zx_instant_boot_t before_ts = current_boot_time();
      guard.CallUnlocked([&log, &status, kPayload]() {
        status = log->Write(kTestSeverity, kTestFlags, kPayload);
      });
      const zx_instant_boot_t after_ts = current_boot_time();

      ASSERT_OK(status);
      EXPECT_EQ(offset, log->tail_);
      EXPECT_EQ(offset + kTotalRecordSize, log->head_);

      // Reassemble the header from the offset and make sure it looks right.
      dlog_header_t hdr;
      memset(&hdr, 0, sizeof(hdr));
      log->ReassembleFromOffset(offset, {reinterpret_cast<uint8_t*>(&hdr), sizeof(hdr)});

      EXPECT_EQ(DLOG_HDR_GET_FIFOLEN(hdr.preamble), kTotalRecordSize);
      EXPECT_EQ(DLOG_HDR_GET_READLEN(hdr.preamble), kTotalRecordSize);
      EXPECT_EQ(kPayloadSize, hdr.datalen);
      EXPECT_GE(hdr.timestamp, before_ts);
      EXPECT_LE(hdr.timestamp, after_ts);
      EXPECT_EQ(kTestSeverity, hdr.severity);
      EXPECT_EQ(kTestFlags, hdr.flags);
      EXPECT_EQ(0u, hdr.pid);  // Kernel PID is always 0
      EXPECT_EQ(our_tid, hdr.tid);
      EXPECT_EQ(log->sequence_count_ - 1u, hdr.sequence);

      // Now attempt to read the record info at the same offset.  We should get
      // the same header base, and regions which exist completely within the
      // ring buffer, and have the test vector pattern in them.
      const zx::result<DLog::Record> maybe_record = log->ReadRecord(offset);
      ASSERT_TRUE(maybe_record.is_ok());
      const DLog::Record& record = maybe_record.value();
      const uintptr_t log_data = reinterpret_cast<uintptr_t>(log->data_);

      EXPECT_BYTES_EQ(reinterpret_cast<const uint8_t*>(&hdr),
                      reinterpret_cast<const uint8_t*>(&record.hdr), sizeof(hdr));
      EXPECT_GT(record.region1.size(), 0u);
      EXPECT_LE(record.region1.size(), DLOG_MAX_RECORD);
      ASSERT_GE(reinterpret_cast<uintptr_t>(record.region1.data()), log_data);
      ASSERT_LE(reinterpret_cast<uintptr_t>(record.region1.data()) + record.region1.size(),
                log_data + DLOG_SIZE);

      if (record.region2.size() > 0u) {
        EXPECT_LE(record.region2.size(), DLOG_MAX_RECORD);
        ASSERT_GE(reinterpret_cast<uintptr_t>(record.region2.data()), log_data);
        ASSERT_LE(reinterpret_cast<uintptr_t>(record.region2.data()) + record.region2.size(),
                  log_data + DLOG_SIZE);
      }

      ASSERT_EQ(kPayloadSize, record.region1.size() + record.region2.size());
      EXPECT_BYTES_EQ(reinterpret_cast<const uint8_t*>(kPayloadData),
                      reinterpret_cast<const uint8_t*>(record.region1.data()),
                      record.region1.size());
      EXPECT_BYTES_EQ(reinterpret_cast<const uint8_t*>(kPayloadData) + record.region1.size(),
                      reinterpret_cast<const uint8_t*>(record.region2.data()),
                      record.region2.size());
      EXPECT_FALSE(record.ends_with_newline);
    }

    END_TEST;
  }

  // Test that write fails with an error once the |DLog| has been shutdown.
  static bool shutdown() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check());

    // Write one message and see that it succeeds.
    char msg[] = "Message!";
    ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)}));

    // Now ask the DLog to shutdown, write another, see that it fails.
    log->Shutdown(0);
    ASSERT_EQ(ZX_ERR_BAD_STATE, log->Write(DEBUGLOG_WARNING, 0, msg));

    // See that there is only one message in the DLog.
    DlogReader reader;
    reader.Initialize(nullptr, nullptr, log.get());
    size_t got;
    dlog_record_t rec{};
    ASSERT_EQ(ZX_OK, reader.Read(0, &rec, &got));
    ASSERT_EQ(sizeof(dlog_header) + sizeof(msg), got);
    ASSERT_EQ(ZX_ERR_SHOULD_WAIT, reader.Read(0, &rec, &got));
    reader.Disconnect();

    END_TEST;
  }

  // Test that we can call write with an active chainlock transaction.
  static bool write_with_active_chainlock_transaction() {
    BEGIN_TEST;

    fbl::AllocChecker ac;
    ktl::unique_ptr<DLog> log = ktl::make_unique<DLog>(&ac);
    ASSERT_TRUE(ac.check());

    char msg[] = "Message!";

    ChainLock lock;
    {
      SingleChainLockGuard guard{IrqSaveOption, lock, CLT_TAG("DebuglogTests")};
      ASSERT_EQ(ZX_OK, log->Write(DEBUGLOG_WARNING, 0, {msg, sizeof(msg)}));
    }

    log->Shutdown(ZX_TIME_INFINITE);

    // See that the message is in the DLog.
    DlogReader reader;
    reader.Initialize(nullptr, nullptr, log.get());
    size_t got;
    dlog_record_t rec{};
    ASSERT_EQ(ZX_OK, reader.Read(0, &rec, &got));
    ASSERT_EQ(sizeof(dlog_header) + sizeof(msg), got);
    ASSERT_EQ(ZX_ERR_SHOULD_WAIT, reader.Read(0, &rec, &got));
    reader.Disconnect();

    END_TEST;
  }
};

#define DEBUGLOG_UNITTEST(fname) UNITTEST(#fname, fname)

UNITTEST_START_TESTCASE(debuglog_tests)
DEBUGLOG_UNITTEST(DebuglogTests::log_format)
DEBUGLOG_UNITTEST(DebuglogTests::log_wrap)
DEBUGLOG_UNITTEST(DebuglogTests::log_reader_read)
DEBUGLOG_UNITTEST(DebuglogTests::log_reader_dataloss)
DEBUGLOG_UNITTEST(DebuglogTests::log_dumper_test)
DEBUGLOG_UNITTEST(DebuglogTests::render_to_crashlog)
DEBUGLOG_UNITTEST(DebuglogTests::read_record)
DEBUGLOG_UNITTEST(DebuglogTests::shutdown)
DEBUGLOG_UNITTEST(DebuglogTests::write_with_active_chainlock_transaction)
UNITTEST_END_TESTCASE(debuglog_tests, "debuglog_tests", "Debuglog test")
