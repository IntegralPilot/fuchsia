// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/debuglog.h>
#include <lib/object-constants.h>

static_assert(sizeof(DlogReader) == kDlogReaderStorageSize, "DlogReader size mismatch");
static_assert(alignof(DlogReader) == kDlogReaderStorageAlign, "DlogReader alignment mismatch");

extern "C" {

zx_status_t cpp_dlog_shutdown(zx_instant_mono_t deadline) { return dlog_shutdown(deadline); }

zx_status_t cpp_dlog_write(uint32_t severity, uint32_t flags, const char* ptr, size_t len) {
  return dlog_write(severity, flags, {ptr, len});
}

void cpp_dlog_reader_init(DlogReader* reader, DlogReader::NotifyCallback* notify, void* cookie) {
  reader->Initialize(notify, cookie);
}

void cpp_dlog_reader_disconnect(DlogReader* reader) { reader->Disconnect(); }

zx_status_t cpp_dlog_reader_read(DlogReader* reader, uint32_t flags, dlog_record_t* record,
                                 size_t* actual) {
  return reader->Read(flags, record, actual);
}

void cpp_dlog_serial_write(const char* ptr, size_t len) { dlog_serial_write({ptr, len}); }
void cpp_dlog_sync() { dlog_sync(); }

}  // extern "C"
