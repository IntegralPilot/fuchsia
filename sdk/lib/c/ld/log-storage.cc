// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstddef>

#include "log.h"

namespace LIBC_NAMESPACE_DECL {
namespace {

// Code using gLog may need to run before all normal constructors have run, and
// keep working after all normal destructors have run.  The Log type is
// entirely constinit-compatible.  But it needs a destructor, so even a
// constinit global induces a static constructor to register the destructor.
// Or else it might induce a direct `.fini_array` entry as a static
// registration of the destructor.  In either case, it's hard or impossible to
// ensure that this destructor will run only after all others.  It's important
// not to tear this down until there is no code (including any plain compiled
// code that's instrumented to possibly call into a sanitizer runtime) that
// might ever use gLog.  So instead of a simple gLog global object here, a test
// on the Log type ensures that Log's default constructor is not only constinit
// but truly just makes all the bytes zero.  This means it's sufficient to just
// pretend that the object was formally constructed, without ever actually
// doing it whether via a global defined with type Log or via placement new.
//
// So here gLogStorage is defined just as an array of bytes; but with the
// alignment and size of Log, internal linkage, and a known linkage name.

using LogStorage = std::byte[sizeof(Log)];
[[maybe_unused]] alignas(Log) constinit LogStorage gLogStorage
    LIBC_ASM_LINKAGE_DECLARE(gLogStorage);

}  // namespace

// Now the real LIBC_NAMESPACE::gLog symbol can be defined as just an alias to
// that storage with a different type.  Outside this file, nothing appears
// different from `Log gLog;` here except that no destructor will ever run.
extern constinit Log gLog [[gnu::alias(LIBC_ASM_LINKAGE_STRING(gLogStorage))]];

}  // namespace LIBC_NAMESPACE_DECL
