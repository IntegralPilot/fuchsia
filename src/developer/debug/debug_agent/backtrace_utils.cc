// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/backtrace_utils.h"

#include <stdio.h>  // For fmemopen

#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/developer/debug/debug_agent/thread_handle.h"
#include "zircon/system/ulib/inspector/include/inspector/inspector.h"

namespace debug_agent {

std::string GetBacktraceMarkupForThread(const ProcessHandle &process, const ThreadHandle &thread) {
  // Large binaries with lots of modules and deep stacks can produce quite a lot of markup text. 32k
  // should be more than enough and leaves plenty of room in the rest of the FIDL message. Once the
  // markup has been written to the string, we trim off the unused bytes below.
  constexpr size_t kMarkupBufferMaxSize = 32768;
  std::string backtrace_markup(kMarkupBufferMaxSize, 0);

  FILE *out = fmemopen(backtrace_markup.data(), backtrace_markup.size(), "w");
  fprintf(out, "{{{reset:begin}}}");
  inspector_print_markup_context(out, process.GetNativeHandle().get());
  inspector_print_backtrace_markup(out, process.GetNativeHandle().get(),
                                   thread.GetNativeHandle().get());
  fprintf(out, "{{{reset:end}}}");

  // The current position of the stream will tell us how large the string really needs to be. If
  // we end up needing to pack multiple backtrace markups in a single response message, this will
  // make sure there's no wasted space.
  auto final_pos = ftell(out);

  fclose(out);

  backtrace_markup.resize(final_pos);
  backtrace_markup.push_back('\0');

  return backtrace_markup;
}

}  // namespace debug_agent
