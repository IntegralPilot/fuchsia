// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_AVAILABILITY_H_
#define ZIRCON_AVAILABILITY_H_

// The macros in this file support implementation of Fuchsia API level-aware
// C++ APIs and C++ code that needs to support multiple Fuchsia API levels.
//
// Values passed to level parameters must be a one of:
//  * a positive decimal integer literal no greater than the largest current stable API level
//    * Valid levels are defined in availability_levels.inc.
//    * Support for older retired API levels as parameters may be removed over time.
//  * `NEXT` - for code to be included in the next stable API level.
//  * `HEAD` - for either of the following:
//    * In-development code that is not ready to be exposed in an SDK
//    * Code that should only be in Platform builds.
//  * `PLATFORM` - for code that must behave differently when building the Fuchsia Platform vs.
//     when building code that runs on it. This is useful when removing an API element while
//     continuing to provide runtime support at previous API levels. It should only be used in-tree.

// When targeting the Fuchsia platform, `__Fuchsia_API_level__` must always be
// specified and valid to ensure proper application of Platform Versioning.
// For Clang, which defaults to level 0, the target level should always be
// specified with `-ffuchsia-api-level`.
#if defined(__Fuchsia__)
#if !defined(__Fuchsia_API_level__) || __Fuchsia_API_level__ == 0
// TODO(https://fxbug.dev/327691011): Change to `#error` once the toolchain
// build targets a specific API level.
#warning `__Fuchsia_API_level__` must be set to a non-zero value. For Clang, use `-ffuchsia-api-level`.
#endif  // !defined(__Fuchsia_API_level__) || __Fuchsia_API_level__ == 0
#endif  // defined(__Fuchsia__)

// =============================================================================
// Availability attribute macros.
// =============================================================================

// Determine whether to implement or stub the macros.
#if defined(__Fuchsia__)
#if defined(__clang__)
// Only Clang supports the underlying availability attributes.
#define _ZX_AVAILABLE_IMPLEMENTED() 1
#else
// Targeting Fuchsia with a compiler other than Clang is risky because Platform Versioning is not
// fully supported due to the lack of support for the availability attributes. As a result, it is
// possible to build code that calls APIs that are not available at the target API level. See
// https://fxbug.dev/327686152.
#define _ZX_AVAILABLE_IMPLEMENTED() 0
#endif  // defined(__clang__)
#else   // defined(__Fuchsia__)
// While Clang would not apply the attributes when not targeting Fuchsia, this is cleaner and
// supports other compilers.
#define _ZX_AVAILABLE_IMPLEMENTED() 0
#endif  // defined(__Fuchsia__)

#if _ZX_AVAILABLE_IMPLEMENTED()

// When targeting the Fuchsia platform, Clang compares the level(s) specified in
// the availability attributes below against the target API level, so it is
// important that the level has been specified correctly. Clang does not perform
// availability checks if the API level is unspecified or 0.
// The above check of `__Fuchsia_API_level__` also ensures a non-zero level has
// been specified because Clang uses `-ffuchsia-api-level` for both.

// An API that was added to the platform.
//
// Annotates the API level at which the API was added to the platform. Use
// ZX_DEPRECATED_SINCE if the API is later deprecated.
//
// Example:
//
//   void fdio_spawn(...) ZX_AVAILABLE_SINCE(4);
//
#define ZX_AVAILABLE_SINCE(level_added) \
  __attribute__((availability(fuchsia, strict, introduced = FUCHSIA_API_LEVEL_(level_added))))

// An API that was added the platform and later deprecated.
//
// Annotates the API level at which the API added the platform and the API
// level at which the API was deprecated.
//
// Deprecated API can still be called by clients. The deprecation annotation
// is a warning that the API is likely to be removed in the future. APIs should
// be deprecated for at least one API level before being removed.
//
// Use the `msg` parameter to explain why the API was deprecated and what
// clients should do instead of using the API.
//
// Example:
//
//   void fdio_fork(...) ZX_DEPRECATED_SINCE(1, 4,
//       "Root cause of security vulnerabilities due to implicit handle "
//       "transfer. Use fdio_spawn instead.");
//
#define ZX_DEPRECATED_SINCE(level_added, level_deprecated, msg)                              \
  __attribute__((availability(fuchsia, strict, introduced = FUCHSIA_API_LEVEL_(level_added), \
                              deprecated = FUCHSIA_API_LEVEL_(level_deprecated), message = msg)))

// An API that was added to the platform and later removed.
//
// Annotates the API level at which the API added the platform, the API
// level at which the API was deprecated, and the API level at which the API
// was removed.
//
// Clients can no longer call APIs if they are compiled to target an API
// level at, or beyond, the level at which the API was removed. APIs should be
// deprecated for at least one API level before being removed.
//
// Example:
//
//   void fdio_fork(...) ZX_REMOVED_SINCE(1, 4, 8,
//       "Root cause of security vulnerabilities due to implicit handle "
//       "transfer. Use fdio_spawn instead.");
//
#define ZX_REMOVED_SINCE(level_added, level_deprecated, level_removed, msg)                  \
  __attribute__((availability(fuchsia, strict, introduced = FUCHSIA_API_LEVEL_(level_added), \
                              deprecated = FUCHSIA_API_LEVEL_(level_deprecated),             \
                              obsoleted = FUCHSIA_API_LEVEL_(level_removed), message = msg)))

#else

#define ZX_AVAILABLE_SINCE(level_added)
#define ZX_DEPRECATED_SINCE(level_added, level_deprecated, msg)
#define ZX_REMOVED_SINCE(level_added, level_deprecated, level_removed, msg)

#endif  // _ZX_AVAILABLE_IMPLEMENTED()

// =============================================================================
// Macros for conditionally compiling code based on the target API level.
// Prefer the attribute macros above for declarations.
//
// Use to guard code that is added and/or removed at specific API levels.
// =============================================================================

// The target API level is `level` or greater.
#define FUCHSIA_API_LEVEL_AT_LEAST(level) (__Fuchsia_API_level__ >= FUCHSIA_API_LEVEL_(level))

// The target API level is less than `level`.
#define FUCHSIA_API_LEVEL_LESS_THAN(level) (__Fuchsia_API_level__ < FUCHSIA_API_LEVEL_(level))

// The target API level is `level` or less.
#define FUCHSIA_API_LEVEL_AT_MOST(level) (__Fuchsia_API_level__ <= FUCHSIA_API_LEVEL_(level))

// =============================================================================
// Internal implementation details of the macros above.
// =============================================================================

// To avoid mistakenly using a non-existent name or unpublished API level, the levels specified in
// the following macros are converted to calls to a macro containing the specified API level in its
// name. If the macro does not exist, the build will fail. See https://fxbug.dev/349622444.
#define FUCHSIA_API_LEVEL_CAT_INDIRECT_(prefix, level) prefix##level##_
#define FUCHSIA_API_LEVEL_CAT_(prefix, level) FUCHSIA_API_LEVEL_CAT_INDIRECT_(prefix, level)
#define FUCHSIA_API_LEVEL_(level) FUCHSIA_API_LEVEL_CAT_(FUCHSIA_INTERNAL_LEVEL_, level)()

// The macros referenced by the output of `FUCHSIA_API_LEVEL_()` must be defined for each API level.
// They are defined in the following file, which must be included after the macros above because it
// may use those macros.
#include <zircon/availability_levels.inc>

// Obsolete mechanism for determining whether the target API level is HEAD.
// Use one of the macros above instead.
// Rather than not defining this identifier, which would cause the preprocessor
// to silently treat any instances as zero, define it as a string, which will
// cause a compiler error if it is used in a preprocessor comparison.
#define FUCHSIA_HEAD \
  "DEPRECATED: Use one of the FUCHSIA_API_LEVEL_*() macros in availability.h instead."

#endif  // ZIRCON_AVAILABILITY_H_
