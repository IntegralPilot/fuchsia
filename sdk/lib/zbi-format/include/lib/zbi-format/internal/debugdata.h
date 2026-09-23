// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ZBI_FORMAT_INTERNAL_DEBUGDATA_H_
#define LIB_ZBI_FORMAT_INTERNAL_DEBUGDATA_H_

#include <stdint.h>

// ZBI_TYPE_DEBUGDATA
//
// This provides a data dump and associated logging from a boot loader,
// shim, or earlier incarnation that wants its data percolated up by the
// booting Zircon kernel.  This is usually used for instrumented builds.
// It's intended to provide the same information and formats that might be
// published via the FIDL fuchsia.debugdata.Publisher protocol.
//
// The payload is described by a trailer rather than a header, with the
// dump contents themselves first in the payload.  This is so that when the
// whole payload is transferred to a VMO, the contents can be used there
// and sent to fuchsia.debugdata.Publisher in place just by adjusting the
// VMO's ZX_PROP_VMO_CONTENT_SIZE property.
//
// The payload is organized into four chunks, followed by the trailer.  The
// whole payload size is always aligned to ZBI_ALIGNMENT bytes.  The
// zbi_debugdata_t trailer is read from the final bytes of the payload.
// Each chunk has an arbitrary size that might not be aligned, and there is
// no alignment padding between the chunks.  The total size of all the
// chunks together is padded to ZBI_ALIGNMENT, with the trailer at the end.
// The sizes in the zbi_debugdata_t trailer must add up, rounded to alignment,
// to exactly sizeof(zbi_debugdata_t) less than the whole payload size.
//
// Layout:
//  * dump contents
//    This corresponds to the VMO contents in fuchsia.debugdata.Publisher.
//    This is arbitrary binary data, its format known by the ultimate consumer.
//  * sink name
//    This corresponds to the sink name in the fuchsia.debugdata.Publisher
//    FIDL protocol.  It's a UTF-8 string with no NUL terminator.  It's a
//    simple string that identifies what format the dump contents are in.
//    It can be used to route collected dumps to appropriate
//    post-processing.  See the FIDL protocol for details.
//  * VMO name
//    This corresponds to the ZX_PROP_NAME property set on the VMO published.
//    It's usually used like a (shortened) file name for the particular dump,
//    perhaps incorporating a process name and PID or the like.
//  * log text
//    This corresponds to logging that was emitted while producing this
//    data.  It might contain messages relevant to understanding the data,
//    including symbolizer markup with symbolization context.  It's a UTF-8
//    string with no NUL terminator, expected to be log lines (with or
//    without timestamp and other such prefix text) separated by \n characters.
//
// The contents and log text can be of any size, but the sink name is
// presumed to be possibly truncated to a small size such as 255 and the
// VMO name is presumed to be possibly truncated to a very small size such
// as ZX_MAX_NAME_LEN (32).
typedef struct {
  uint32_t content_size;
  uint32_t sink_name_size;
  uint32_t vmo_name_size;
  uint32_t log_size;
} zbi_debugdata_t;

#endif  // LIB_ZBI_FORMAT_INTERNAL_DEBUGDATA_H_
