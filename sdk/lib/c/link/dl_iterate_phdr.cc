// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/link/dl_iterate_phdr.h"

#include <lib/ld/dl-phdr-info.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>

#include "../dlfcn/dlfcn-abi.h"
#include "../ld/ld-abi.h"
#include "../weak.h"
#include "src/__support/common.h"

namespace LIBC_NAMESPACE_DECL {

LLVM_LIBC_FUNCTION(int, dl_iterate_phdr, (__dl_iterate_phdr_callback_t callback, void* arg)) {
  // Use the same sample in all the callbacks, even though dlopen/dlclose might
  // change it during callbacks.  The startup modules won't have changed even
  // if whatever _dlfcn_iterate_phdr may be reporting later might have changed;
  // when _dlfcn_iterate_phdr makes its own callbacks, it can use different
  // counts than these.  All that really matters to the caller is that if any
  // calls to its callback see the same counts as a callback in a _previous_
  // dl_iterate_phdr call, then no actual loading or unloading events had an
  // established "happens before" relationship with starting *this* call--it
  // can safely short-circuit further callbacks and keep using cached data that
  // was accurate as of the last dl_iterate_phdr call completed.  If it hasn't
  // short-circuited and then a dlopen'd module's callback has higher counts
  // than the previous (startup module) callback, it's still collecting the new
  // state of all modules regardless.
  const ld::DlPhdrInfoCounts counts =
      Weak<_dlfcn_phdr_info_counts>::Or{ld::DlPhdrInfoInitialExecCounts(_ld_abi)}();

  // First report all the startup modules.  There's no need for any locking
  // because nothing can ever change.
  for (const auto& module : ld::AbiLoadedModules(_ld_abi)) {
    // If this module has a PT_TLS, find it for the current thread.
    void* tls = ld::TlsInitialExecData(_ld_abi, module.tls_modid);

    // Note the legacy API specification neglected to make the argument const*,
    // so the callback must be allowed to clobber the dl_phdr_info object.
    dl_phdr_info info = ld::MakeDlPhdrInfo(module, tls, counts);
    if (int result = (*callback)(&info, sizeof(info), arg); result != 0) {
      return result;
    }
  }

  // Now let libdl report any additional modules from dlopen calls.  It will
  // handle synchronization vs dlopen/dlclose.  It will recompute its own
  // counts for those reports, which might differ from the unsynchronized
  // sample used for the startup modules.  TODO(https:://fxbug.dev/338239201)
  return Weak<_dlfcn_iterate_phdr>::Or{0}(callback, arg);
}

}  // namespace LIBC_NAMESPACE_DECL
