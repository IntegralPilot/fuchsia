// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_H_
#define ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_H_

#include <lib/debuglog_types.h>
#include <stdint.h>
#include <stdio.h>
#include <zircon/compiler.h>
#include <zircon/syscalls/log.h>
#include <zircon/types.h>

#include <kernel/event.h>
#include <kernel/mutex.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <ktl/type_traits.h>
#include <phys/boot-constants.h>

class DLog;

// DlogReaders drain debuglogs. Owners of DlogReaders are called back as
// messages are pushed through the debuglog, via the Notify callback.
class DlogReader : public fbl::DoublyLinkedListable<DlogReader*> {
 public:
  using NotifyCallback = void(void* cookie);

  constexpr DlogReader() {}
  ~DlogReader();

  // Since DlogReaders typically capture containing objects via |cookie_|, they
  // use 2-phase initialization to avoid races in the contruction of the
  // DlogReader and the containing object.
  // The optional parameter `log` is used mainly for testing purpose. It should be left as default
  // for all other uses, in which case the static global DLOG will be used. If `log` takes the
  // default value i.e static global DLOG, it will outlive the DLogReader and hence there will be no
  // lifetime issues. If `log` is specified, as it would be in case of tests, the test has to
  // ensure that the `log` object outlives any DLogReaders associated to it.
  void Initialize(NotifyCallback* notify, void* cookie, DLog* log = nullptr);

  // Read one record out of the log and store it in |record|.
  //
  // Upon success, returns ZX_OK and sets *|actual| to the record's size.
  zx_status_t Read(uint32_t flags, dlog_record_t* record, size_t* actual);

  void Notify();

  // Similar to Initialize, DlogReaders are be manually stopped via |Disconnect|
  // to avoid reentrency issues in the DlogReader and its containing object.
  //
  // Disconnect must be called before the destructor runs, if Initialize was called.
  void Disconnect();

 private:
  DLog* log_ = nullptr;
  size_t tail_ = 0;

  NotifyCallback* notify_ = nullptr;
  void* cookie_ = nullptr;
};

// Severity Levels
#define DEBUGLOG_TRACE (0x10)
#define DEBUGLOG_DEBUG (0x20)
#define DEBUGLOG_INFO (0x30)
#define DEBUGLOG_WARNING (0x40)
#define DEBUGLOG_ERROR (0x50)
#define DEBUGLOG_FATAL (0x60)

static_assert(sizeof(dlog_record_t) == DLOG_MAX_RECORD, "");

// Returns ZX_ERR_BAD_STATE if the dlog has already started shutting down.
zx_status_t dlog_write(uint32_t severity, uint32_t flags, ktl::string_view str);

// Writes |str| out the serial (debug) port.
//
// This function is a no-op if the dlog has already been shutdown
// (i.e. |dlog_shutdown| has completed).
//
// Used by sys_debug_write().
void dlog_serial_write(ktl::string_view str);

// Allow FILE*-based APIs, like fprintf, to use the same output
// backend as dlog_serial_write.
extern FILE gDlogSerialFile;

// Tell the debuglog that the system has started to panic so that subsequent
// writes won't wake any debuglog threads.  This function should be called
// *very* early in the process of panicking.  In particular, it should be called
// before |dlog_bluescreen_init|.
void dlog_panic_start();

// Print the equivalent of a "blue screen" message.  When called during a panic,
// this function should be called after |dlog_panic_start| to ensure that its
// printfs bypass the normal debuglog queue and drainer thread.
void dlog_bluescreen_init();

// Initialize the debuglog subsystem. Called once at extremely early boot.
void dlog_init_early();

// Shutdown the debuglog subsystem.
//
// Blocks, waiting up to |deadline|, for dlog threads to terminate.  It is safe
// for Shutdown to be called concurrently by multiple threads.  Only one thread
// will "win" and all others will simply wait for it to complete.
//
// Once shutdown has begun, |dlog_write| will fail.  However, calls to
// |dlog_serial_write| will continue to work until shutdown has completed.  At
// which point, they will become no-ops.
//
// On failure, the debuglog subsystem is left in an undefined state.
//
// Returns ZX_OK on success.
zx_status_t dlog_shutdown(zx_instant_mono_t deadline);

// Accessor to quickly determine if the debuglog bypass is enabled.
inline bool dlog_bypass() { return kBootConstants.bypass_debuglog; }

// Renders as many of the recent debug log entries as will fit into the memory
// region specified by |target|.  Returns the number of bytes of target which
// were filled.
size_t dlog_render_to_crashlog(ktl::span<char> target);

// Blocks until the dumper thread has dumped all queued log messages up to the current sequence.
// Must only be called from a thread context where blocking is permitted.
void dlog_sync();

extern "C" {
zx_status_t cpp_dlog_shutdown(zx_instant_mono_t deadline);
zx_status_t cpp_dlog_write(uint32_t severity, uint32_t flags, const char* ptr, size_t len);
void cpp_dlog_reader_init(DlogReader* reader, DlogReader::NotifyCallback* notify, void* cookie);
void cpp_dlog_reader_disconnect(DlogReader* reader);
zx_status_t cpp_dlog_reader_read(DlogReader* reader, uint32_t flags, dlog_record_t* record,
                                 size_t* actual);
void cpp_dlog_serial_write(const char* ptr, size_t len);
void cpp_dlog_sync();
}

#endif  // ZIRCON_KERNEL_LIB_DEBUGLOG_INCLUDE_LIB_DEBUGLOG_H_
