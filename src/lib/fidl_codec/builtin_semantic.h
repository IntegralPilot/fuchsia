// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_FIDL_CODEC_BUILTIN_SEMANTIC_H_
#define SRC_LIB_FIDL_CODEC_BUILTIN_SEMANTIC_H_

#include <string_view>

namespace fidl_codec::semantic {

constexpr std::string_view kBuiltinSemantics =
    "library fuchsia.io {\n"
    "  Openable::Open {\n"
    "    request.object = handle / request.path;\n"
    "    input_field: request.path;\n"
    "    result: request.object;\n"
    "  }\n"
    "  File::Seek {\n"
    "    input_field: request.origin;\n"
    "    input_field: request.offset;\n"
    "  }\n"
    "  File::Write {\n"
    "    input_field: request.data.size ' bytes';\n"
    "  }\n"
    "}\n"
    "library fuchsia.unknown {\n"
    "  Cloneable::Clone {\n"
    "    request.request = handle : 'cloned';\n"
    "  }\n"
    "}\n";

}  // namespace fidl_codec::semantic

#endif  // SRC_LIB_FIDL_CODEC_BUILTIN_SEMANTIC_H_
