// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEBUGLOG_DEBUGLOG_INTERNAL_H_
#define ZIRCON_KERNEL_LIB_DEBUGLOG_DEBUGLOG_INTERNAL_H_

#include <lib/debuglog.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>

#include <kernel/event.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <kernel/timer.h>
#include <ktl/algorithm.h>
#include <ktl/limits.h>
#include <ktl/span.h>
#include <ktl/string_view.h>

#define DLOG_SIZE (128u * 1024u)
#define DLOG_MASK (DLOG_SIZE - 1u)

// Make sure that when |dlog_header_t| is in the fifo, it's 4 byte aligned and that the first 4
// bytes (the |preamble| field) don't wrap.
static constexpr size_t kDLogHeaderFifoAlignment = 4;
#define DLOG_ALIGN_TRUNC(n) ((n) & (~3))
#define DLOG_ALIGN(n) DLOG_ALIGN_TRUNC(((n) + 3))

#define DLOG_HDR_SET(fifosize, readsize) ((((readsize) & 0xFFF) << 12) | ((fifosize) & 0xFFF))
#define DLOG_HDR_GET_FIFOLEN(n) ((n) & 0xFFF)
#define DLOG_HDR_GET_READLEN(n) (((n) >> 12) & 0xFFF)

class DlogReader;
struct DebuglogTests;

class DLog {
 public:
  explicit constexpr DLog() {}

  virtual ~DLog() {
    // Be sure to cancel any pending timer to avoid a potential use-after-free.
    deferred_signal_timer_.Cancel();
  }

  void StartThreads() TA_EXCL(lock_);

  // Mark this DLog as shutting down, then shutdown all threads.  Once called, subsequent |write|
  // operations will fail, but already-queued messages will continue to be processed/emitted.
  zx_status_t Shutdown(zx_instant_mono_t deadline) TA_EXCL(lock_);

  // Returns true iff we have finished shutting down this instance.
  bool ShutdownFinished() const {
    return lifecycle_.load(ktl::memory_order_acquire) == Lifecycle::ShutdownFinished;
  }

  // See |dlog_panic_start|.
  void PanicStart() TA_EXCL(lock_);

  // See |dlog_bluescreen_init|.
  void BluescreenInit() TA_EXCL(lock_);

  // See "dlog_render_to_crashlog" in include/lib/debuglog.h
  size_t RenderToCrashlog(ktl::span<char> target) const TA_EXCL(lock_);

  zx_status_t Write(uint32_t severity, uint32_t flags, ktl::string_view str) TA_EXCL(lock_);

  // Blocks until the dumper thread has dumped all queued log messages up to the current sequence.
  // Must only be called from a thread context where blocking is permitted.
  void Sync() TA_EXCL(lock_);

 protected:
  // Methods that can be overridden for tests.
  virtual void OutputLogMessage(ktl::string_view log);

 private:
  friend struct DebuglogTests;
  friend class DlogReader;

  struct ThreadState {
    Thread* thread{nullptr};
    ktl::atomic<bool> shutdown_requested{false};
    AutounsignalEvent event;
  };

  // A small struct which holds a representation of a debuglog record, including:
  // 1) A reconstructed header, contiguous in memory.
  // 2) Two string_view regions representing the data portion of the record as
  //    it exists in the ring buffer.
  // 3) A flag computed during ReadRecord indicating whether or not the data
  //    region of this record ends with a newline or not.
  struct Record {
    dlog_header_t hdr{};
    ktl::string_view region1{};
    ktl::string_view region2{};
    bool ends_with_newline{false};
  };

  static inline constexpr char kDlogNotifierThreadName[] = "debuglog-notifier";
  static inline constexpr char kDlogDumperThreadName[] = "debuglog-dumper";

  // Attempt to format a debuglog record header into the memory location supplied in |target|.  The
  // value returned will depend on what is passed for |target|.
  //
  // 1) If target.data() is nullptr, this is a "measurement" operation.  The
  //    return value will be an indication of the length which _would be needed_
  //    in order to properly render the header into the buffer supplied.
  // 2) If target.data is non-null, then this is a "render" operation.  The
  //    return value will be an indication of the _amount of the target buffer
  //    which was consumed_.  It will never exceed the length of the target
  //    buffer.
  //
  // In either case, errors returned by snprintf will never be propagated up to
  // the user.  If snprintf indicates an error, the result returned to the user
  // will 0, regardless of whether this is a measurement or render operation.
  //
  static size_t FormatHeader(ktl::span<char> target, const dlog_header_t& hdr) {
    // It is illegal to pass a null pointer for target, but have a non-zero
    // length.
    if ((target.data() == nullptr) && (target.size() != 0)) {
      return 0;
    }

    int res = snprintf(target.data(), target.size(), "[%05d.%03d] %05" PRIu64 ":%05" PRIu64 "> ",
                       static_cast<int>(hdr.timestamp / ZX_SEC(1)),
                       static_cast<int>((hdr.timestamp / ZX_MSEC(1)) % 1000ULL), hdr.pid, hdr.tid);
    size_t ret = (res < 0) ? 0 : static_cast<size_t>(res);

    return (target.data() == nullptr) ? ret : ktl::min(target.size(), ret);
  }

  // A small helper alias for FormatHeader which helps to make it a bit more
  // clear at the callsite what operation is being performed.
  static size_t MeasureRenderedHeader(const dlog_header_t& hdr) { return FormatHeader({}, hdr); }

  // Attempt to read |target.size()| bytes from an absolute location in the
  // debuglog buffer given by |offset|, storing the result at |target.data()|
  // and dealing with any wrapping of the ring buffer in the process.  Note that
  // this method is not specific to reading a dlog_header_t, or a record's
  // payload.  It simply attempts to read a contiguous sequence of bytes from
  // the buffer, and automatically deals with the ring buffer wrapping for the
  // user at the same time.
  //
  zx_status_t ReassembleFromOffset(size_t offset, ktl::span<uint8_t> target) const TA_REQ(lock_) {
    // Attempting to read 0 bytes is simple, we are done already.
    if (target.empty()) {
      return ZX_OK;
    }

    // Attempting to read to a null pointer, or to read more data than can exist
    // in the buffer, is considered an error.
    if (!target.data() || (target.size() > DLOG_SIZE)) {
      return ZX_ERR_INVALID_ARGS;
    }

    offset &= DLOG_MASK;
    size_t fifospace = DLOG_SIZE - offset;
    if (target.size() <= fifospace) {
      // The requested region exists contiguously in the circular buffer.
      memcpy(target.data(), data_ + offset, target.size());
    } else {
      // The requested region wraps in the circular buffer, and needs to be
      // copied as 2 chunks.
      memcpy(target.data(), data_ + offset, fifospace);
      memcpy(target.data() + fifospace, data_, target.size() - fifospace);
    }

    return ZX_OK;
  }

  // Attempt to read a Record from the ring buffer located at |offset|,
  // reporting any diagnostic information to |error_file| in the case of
  // trouble.
  zx::result<Record> ReadRecord(size_t offset, FILE* error_file = nullptr) const TA_REQ(lock_) {
    // Attempt to reassemble the header from the specified offset.
    Record ret;
    zx_status_t status =
        ReassembleFromOffset(offset, {reinterpret_cast<uint8_t*>(&ret.hdr), sizeof(ret.hdr)});
    if (status != ZX_OK) {
      if (error_file) {
        fprintf(error_file, "Failed to read header at offset %zu (%d)\n", offset, status);
      }
      return zx::error(status);
    }

    // Perform consistency checks of the lengths.
    const size_t readlen = DLOG_HDR_GET_READLEN(ret.hdr.preamble);
    if ((readlen < sizeof(ret.hdr)) || ((readlen - sizeof(ret.hdr)) != ret.hdr.datalen)) {
      if (error_file) {
        fprintf(error_file, "Bad lengths (pre %zu, hdr_sz %zu, datalen %hu)\n", readlen,
                sizeof(ret.hdr), ret.hdr.datalen);
      }
      return zx::error(ZX_ERR_INTERNAL);
    }

    if (ret.hdr.datalen) {
      // Figure out if the payload portion of the record needs to be represented
      // by one contiguous region, or two.  Start by computing the offset into
      // the ring buffer where the data region begins.  Next compute the
      // distance from this offset to the end of the ring buffer.  This is the
      // total amount of contiguous space we have for the payload.  If the
      // length of the payload is larger than this region, we need to produce
      // two regions, one from the start of the payload to the end of the ring,
      // and the other from the start of the ring to the end of the payload.
      // Otherwise, we can just produce a single contiguous region and be done
      // with it.
      const size_t data_offset = (offset + sizeof(ret.hdr)) & DLOG_MASK;
      const size_t fifospace = DLOG_SIZE - data_offset;
      const char* const char_data = reinterpret_cast<const char*>(data_);
      if (ret.hdr.datalen <= fifospace) {
        ret.region1 = {char_data + data_offset, ret.hdr.datalen};
        ret.ends_with_newline = (ret.region1.back() == '\n');
      } else {
        ret.region1 = {char_data + data_offset, fifospace};
        ret.region2 = {char_data, ret.hdr.datalen - fifospace};
        ret.ends_with_newline = (ret.region2.back() == '\n');
      }
    }

    return zx::ok(ret);
  }

  size_t RenderToCrashlogLocked(ktl::span<char> target) const TA_REQ(lock_);

  int NotifierThread();
  int DumperThread();

  static void DeferredSignal(Timer* timer, zx_instant_mono_t now, void* arg) {
    reinterpret_cast<DLog*>(arg)->DeferredSignal();
  }
  void DeferredSignal();

  ThreadState notifier_state_;
  ThreadState dumper_state_;

  // Use MonitoredSpinLock to provide lockup detector diagnostics for the critical sections
  // protected by this lock.
  mutable DECLARE_SPINLOCK_WITH_TYPE(DLog, MonitoredSpinLock) lock_;
  DECLARE_LOCK(DLog, Mutex) readers_lock_;

  size_t head_ TA_GUARDED(lock_) = 0;
  size_t tail_ TA_GUARDED(lock_) = 0;

  // Used to signal an event from a "safe" context.  See |Write| for details.
  Timer deferred_signal_timer_ TA_GUARDED(lock_);

  // Indicates there is a pending |deferred_signal_timer_|.  Used to reduce the
  // number of unnecessary timers.
  RelaxedAtomic<bool> pending_deferred_signal_ = false;

  // Indicates that the system has begun to panic.  When true, |write| will
  // immediately return an error.
  //
  // TODO(maniscalco): This field should probably be an atomic since it can be
  // accessed from multiple threads.  When/if it becomes an atomic, think
  // carefully about whether it needs acquire/release semantics.
  bool panic_ = false;

  // The lifecycle state of this |DLog| object.
  enum class Lifecycle : uint32_t {
    Running,
    ShutdownStarted,   // Shutdown has been called, but has not yet completed.  No new messages can
                       // be be queued.  Calls to |Write| will fail.  Already-queued messages may
                       // continue to be processed/emitted.
    ShutdownFinished,  // Shutdown has completed.  |dlog_serial_write| will drop data.
  };
  ktl::atomic<Lifecycle> lifecycle_{Lifecycle::Running};

  // The list of our current readers.
  fbl::DoublyLinkedList<DlogReader*> readers TA_GUARDED(readers_lock_);

  // A counter incremented for each log message that enters the debuglog.
  uint64_t sequence_count_ TA_GUARDED(lock_) = 0;

  // Signaled when shutdown has completed.
  Event shutdown_finished_;

  // The sequence number up to which the dumper thread has dumped.
  ktl::atomic<uint64_t> dumper_sequence_{0};

  // Signaled when the dumper thread has drained all currently available records.
  AutounsignalEvent dumper_drained_event_;

  // This array contains dlog_header_t object so make sure it's properly aligned.
  alignas(kDLogHeaderFifoAlignment) uint8_t data_[DLOG_SIZE]{0};
};

#endif  // ZIRCON_KERNEL_LIB_DEBUGLOG_DEBUGLOG_INTERNAL_H_
