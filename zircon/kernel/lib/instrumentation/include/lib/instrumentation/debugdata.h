// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_INSTRUMENTATION_INCLUDE_LIB_INSTRUMENTATION_DEBUGDATA_H_
#define ZIRCON_KERNEL_LIB_INSTRUMENTATION_INCLUDE_LIB_INSTRUMENTATION_DEBUGDATA_H_

#include <zircon/assert.h>
#include <zircon/types.h>

#include <ktl/array.h>
#include <ktl/numeric.h>
#include <ktl/span.h>
#include <ktl/string_view.h>

namespace instrumentation {

using VmoName = ktl::array<char, ZX_MAX_NAME_LEN>;

// Higher level components will search under this root for debug data files from early boot.
constexpr ktl::string_view kRootPath = "i/";

// Debugdata files from early boot under this directory imply that the data is in flux, and
// continuously being updated.
constexpr ktl::string_view kDynamicSubDir = "/d/";

// Debugdata files from early boot under this directory imply that the data is static, and wont
// change.
constexpr ktl::string_view kStaticSubDir = "/s/";

// Debugdata sink used for logs.
constexpr ktl::string_view kLogDebugDataSink = "logs";

// Returns an encoded path that fits within an object name. The encoded path is of the form:
//   "$PATH = $ROOT_DIR/$SINK_NAME/$DATA_TYPE/$MODULE_NAME.$SUFFIX"
//
// Given the length limit of an object's name property, and it's truncated to fit.
//
// It is considered if at least the module name does not fit into the encoded path.
inline VmoName DebugdataVmoName(ktl::string_view sink_name, ktl::string_view module_name,
                                ktl::string_view suffix, bool is_static) {
  VmoName name = {};
  ktl::span<char> buffer(name);
  ktl::string_view data_type = is_static ? kStaticSubDir : kDynamicSubDir;
  ktl::array path{kRootPath, sink_name, data_type, module_name};

  // At least a few characters of the module name must fit in the name.
  constexpr auto accumulate_size = [](size_t total, ktl::string_view str) {
    return total + str.size();
  };
  size_t length = ktl::reduce(path.begin(), path.end(), size_t{}, accumulate_size);
  ptrdiff_t overage = length - ZX_MAX_NAME_LEN;
  ZX_ASSERT_MSG(overage <= 4, "VMO name would be %zu > %zu by %td", length, ZX_MAX_NAME_LEN,
                overage);
  for (ktl::string_view component : path) {
    buffer = buffer.subspan(component.copy(buffer.data(), buffer.size() - 1));
    if (buffer.empty()) {
      break;
    }
  }

  // Either append full suffix or nothing at all.
  if (buffer.size() >= suffix.size() + 1) {
    buffer[0] = '.';
    suffix.copy(buffer.subspan(1).data(), buffer.size() - 1);
  }
  return name;
}

}  // namespace instrumentation

#endif  // ZIRCON_KERNEL_LIB_INSTRUMENTATION_INCLUDE_LIB_INSTRUMENTATION_DEBUGDATA_H_
