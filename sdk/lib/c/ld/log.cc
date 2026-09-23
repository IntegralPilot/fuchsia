// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "log.h"

#include <lib/ld/module.h>
#include <lib/symbolizer-markup/line-buffered-sink.h>
#include <lib/symbolizer-markup/writer.h>

#include "ld-abi.h"

namespace LIBC_NAMESPACE_DECL {

void Log::StartupSymbolizerContext() { context_logged_.test_and_set(); }

void Log::SymbolizerContext() {
  if (context_logged_.test_and_set() || !*this) {
    return;
  }

  // The markup writer calls the sink function for each little fragment.
  // Collect whole lines before calling the Log.
  symbolizer_markup::Writer writer{
      symbolizer_markup::LineBuffered<kBufferSize>::Sink{std::ref(*this)}};

  for (const auto& module : ld::AbiLoadedModules(_ld_abi)) {
    ld::ModuleSymbolizerContext(writer, module, zx_system_get_page_size());
  }
}

}  // namespace LIBC_NAMESPACE_DECL
