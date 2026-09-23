// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_PHYS_LOG_H_
#define ZIRCON_KERNEL_PHYS_LOG_H_

#include <stdio.h>

#include <ktl/initializer_list.h>
#include <ktl/string_view.h>
#include <phys/allocation.h>

// The Log is a FILE-compatible object that accumulates output in a contiguous
// buffer of whole pages suitable for later handoff.  It can optionally take
// over as stdout, mirroring everything to the previous stdout before writing
// it to the log.  Page allocation failures cause a panic, which always resets
// to the original stdout first when the Log is currently stdout.
class Log {
 public:
  // This mirrors to the console (first) if SetStdout() has been used,
  // then appends to the log.
  int Write(ktl::string_view str);

  // This only appends to the log without mirroring to the console.
  void AppendToLog(ktl::string_view str);

  // Returns a FILE that calls AppendToLog instead of Write.
  [[nodiscard]] FILE LogOnlyFile();

  // Returns a FILE that acts either like Write or like LogOnlyFile,
  // depending on the kernel.phys.verbose boot option.
  [[nodiscard]] FILE VerboseOnlyFile();

  // Replace the *stdout object with this one, storing the previous
  // stdout object and mirroring the output to that one.
  void SetStdout();

  // If this object was previously installed with SetStdout(), then restore the
  // original stdout that it replaced.  If not, this is a harmless no-op.
  void RestoreStdout();

  // When the object dies, it always detaches from stdout.
  ~Log() { RestoreStdout(); }

  // This can be used to track the current total size of the accumulated log.
  size_t size_bytes() const { return size_; }

  bool empty() const { return size_ == 0; }

  // This takes ownership of the log's contiguous pages.  After this, no other
  // methods can be used.  In particular, size_bytes() should be taken first to
  // get the exact content size rather than the page-rounded buffer size.
  Allocation TakeBuffer() && {
    RestoreStdout();
    size_ = 0;
    return ktl::move(buffer_);
  }

  // This borrows a snapshot of the accumulated log text.  **NOTE!** The view
  // returned here must be used and discarded before anything might append to
  // the log in any fashion, which many paths can do indirectly.
  ktl::string_view BorrowSnapshot() const {
    return {reinterpret_cast<const char*>(buffer_.get()), size_};
  }

 private:
  ktl::span<char> buffer_chars() {
    return {reinterpret_cast<char*>(buffer_.get()), buffer_.size_bytes()};
  }

  ktl::span<char> buffer_chars_left() { return buffer_chars().subspan(size_); }

  Allocation buffer_;
  size_t size_ = 0;
  FILE mirror_;
};

extern Log* gLog;

#endif  // ZIRCON_KERNEL_PHYS_LOG_H_
