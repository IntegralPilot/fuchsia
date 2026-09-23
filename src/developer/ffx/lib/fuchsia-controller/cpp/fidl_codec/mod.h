// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_MOD_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_MOD_H_

#include <Python.h>

#include <sstream>
#include <string>

#include "src/lib/fidl_codec/library_loader.h"

namespace fuchsia_controller::fidl_codec::mod {

static const std::string FIDL_IR_DEPFILE("all_fidl_json.txt");

struct FidlCodecState {
  FidlCodecState() : loader(std::make_unique<::fidl_codec::LibraryLoader>()) {}
  std::unique_ptr<::fidl_codec::LibraryLoader> loader;
};

FidlCodecState *get_module_state();

inline ::fidl_codec::Library *get_ir_library(const std::string &library_name) {
  auto res = get_module_state()->loader->GetLibraryFromName(library_name);
  if (res == nullptr) {
    std::stringstream ss;
    ss << "Unable to find library '" << library_name
       << "' in module. It's possible the library was not loaded via the `add_ir_path` function";
    PyErr_SetString(PyExc_RuntimeError, ss.str().c_str());
    return nullptr;
  }
  return res;
}

}  // namespace fuchsia_controller::fidl_codec::mod

#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_MOD_H_
