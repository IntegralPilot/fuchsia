// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SECTION_SYMBOLS_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SECTION_SYMBOLS_H_

#include <lib/special-sections/special-sections.h>

namespace fxt {

// Forward declarations.
class InternedCategory;
class InternedString;

}  // namespace fxt

// Linker section for InternedString instances.
#define FXT_INTERNED_STRING_SECTION \
  SPECIAL_SECTION("__fxt_interned_string_table", ::fxt::InternedString)

// Linker section for InternedCategory instances.
#define FXT_INTERNED_CATEGORY_SECTION \
  SPECIAL_SECTION("__fxt_interned_category_table", ::fxt::InternedCategory)

#if _KERNEL
#define FXT_WEAK_SECTION_SYMBOL
#else
#define FXT_WEAK_SECTION_SYMBOL [[gnu::weak]]
#endif

extern "C" {

// The following symbols are always provided by the kernel (see //zircon/kernel/kernel.ld),
// regardless of whether the section is empty. In userspace, an empty section does not generate the
// begin/end symbols. Avoid linker errors in userspace by providing weak alternatives.

// Symbols for the bounds of the interned string section.
extern const fxt::InternedString __start___fxt_interned_string_table FXT_WEAK_SECTION_SYMBOL[];
extern const fxt::InternedString __stop___fxt_interned_string_table FXT_WEAK_SECTION_SYMBOL[];

// Symbols for the bounds of the interned category section.
extern const fxt::InternedCategory __start___fxt_interned_category_table FXT_WEAK_SECTION_SYMBOL[];
extern const fxt::InternedCategory __stop___fxt_interned_category_table FXT_WEAK_SECTION_SYMBOL[];

}  // extern "C"

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_SECTION_SYMBOLS_H_
