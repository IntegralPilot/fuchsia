// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_DUMP_INCLUDE_LIB_DUMP_DEPTH_PRINTER_H_
#define ZIRCON_KERNEL_LIB_DUMP_INCLUDE_LIB_DUMP_DEPTH_PRINTER_H_

#include <assert.h>
#include <stdarg.h>
#include <stdio.h>
#include <sys/types.h>
#include <zircon/compiler.h>

namespace dump {

// Helper for printing 'dump' style information at different depths (i.e. with space prefixes).
//
// This class is not thread safe.
class DepthPrinter {
 public:
  DepthPrinter(FILE* file, size_t depth) : file_(file), depth_(depth) {}
  explicit DepthPrinter(size_t depth) : DepthPrinter(stdout, depth) {}
  ~DepthPrinter() { DEBUG_ASSERT(!in_list_); }

  // Disallow copying and moving
  DepthPrinter(const DepthPrinter&) = delete;
  DepthPrinter(DepthPrinter&&) = delete;
  DepthPrinter& operator=(const DepthPrinter&) = delete;
  DepthPrinter& operator=(DepthPrinter&&) = delete;

  // Emit a single item it. It will have spaces prefixed based on the depth and a newline added to
  // the end.
  [[gnu::format(printf, 2, 3)]] void Emit(const char* fmt, ...) {
    va_list args;
    va_start(args, fmt);
    VEmit(fmt, args);
    va_end(args);
  }

  // Same as emit, but takes a va_list.
  [[gnu::format(printf, 2, 0)]] void VEmit(const char* fmt, va_list args) {
    if (in_list_) {
      list_emitted_++;
      if (list_emitted_ > list_max_) {
        return;
      }
    }
    PrintDepth();
    vfprintf(file_, fmt, args);
    fprintf(file_, "\n");
  }

  // Indicates a list is about to be emitted. Only at most |max| Emit calls will result in output,
  // with any additional being counted. |EndList| must be called once finished emitting the list.
  void BeginList(size_t max) {
    DEBUG_ASSERT(!in_list_);
    list_max_ = max;
    list_emitted_ = 0;
    in_list_ = true;
  }

  // Indicates that printing of the list is finished and, if relevant, will emit a message
  // indicating how many items were skipped.
  void EndList() {
    DEBUG_ASSERT(in_list_);
    in_list_ = false;
    if (list_emitted_ > list_max_) {
      Emit("[%zu items not emitted]", list_emitted_ - list_max_);
    }
  }

 private:
  void PrintDepth() const {
    for (size_t i = 0; i < depth_; i++) {
      fprintf(file_, "  ");
    }
  }
  FILE* file_;
  const size_t depth_;
  size_t list_max_ = 0;
  size_t list_emitted_ = 0;
  bool in_list_ = false;
};

}  // namespace dump

#endif  // ZIRCON_KERNEL_LIB_DUMP_INCLUDE_LIB_DUMP_DEPTH_PRINTER_H_
