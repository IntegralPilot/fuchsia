// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/dl-phdr-info.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <zircon/sanitizer.h>

#include "../startup/start-main.h"
#include "ld-abi.h"

// This might be defined by a sanitizer runtime, or might be left undefined.
// NOLINTNEXTLINE(readability-redundant-declaration)
[[gnu::weak]] decltype(__sanitizer_module_loaded) __sanitizer_module_loaded;

namespace LIBC_NAMESPACE_DECL {

// This is called early in startup, before __sanitizer_startup_hook is called.
void StartupSanitizerModuleLoaded() {
  if (!__sanitizer_module_loaded) {
    return;
  }

  // This call can never be re-entered, but once it starts making callbacks
  // it's possible that one of those will use dlopen or will spawn threads that
  // use dlopen.  TODO(https://fxbug.dev/338239201): We can lock around this to
  // exclude other threads using dlopen, but allow dlopen from inside a
  // callback here.  That will use _dlfcn_module_loaded to report new modules
  // before we've reported all the startup modules, but order probably doesn't
  // matter to the callbacks.  If it did, we could keep state of what's been
  // reported so far; and have _dlfcn_module_loaded finish these reports first,
  // updating state so we finish early here.
  for (const auto& module : ld::AbiLoadedModules(_ld_abi)) {
    // As per <zircon/sanitizer.h>, counts and TLS data are omitted.
    const dl_phdr_info info = ld::MakeDlPhdrInfo(module, nullptr, {});
    __sanitizer_module_loaded(&info, sizeof(info));
  }
}

}  // namespace LIBC_NAMESPACE_DECL
