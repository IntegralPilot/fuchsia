// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZBI_FORMAT_INTERNAL_STORAGE_H_
#define LIB_ZBI_FORMAT_INTERNAL_STORAGE_H_

#include <lib/zbi-format/zbi.h>

// The zbi_header_t.extra field always gives the exact size of the
// original, uncompressed payload.  That equals zbi_header_t.length when
// the payload is not compressed.  If ZBI_FLAGS_STORAGE_COMPRESSED is set in
// zbi_header_t.flags, then the payload is compressed.
//
// **Note:** Magic-number and header bytes at the start of the compressed
// payload indicate the compression algorithm and parameters.  The set of
// compression formats is not a long-term stable ABI.
//  - Zircon userboot (//docs/userboot.md) and core services
//    do the decompression.  A given kernel build's `userboot` will usually
//    only support one particular compression format.
//  - The `zbi` tool will usually retain the ability to compress and
//    decompress for old formats, and can be used to convert between formats.
#define ZBI_FLAGS_STORAGE_COMPRESSED ((zbi_flags_t)(1u << 0))

#endif  // LIB_ZBI_FORMAT_INTERNAL_STORAGE_H_
