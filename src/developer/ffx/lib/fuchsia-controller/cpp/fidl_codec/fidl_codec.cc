// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <Python.h>

#include <fuchsia_controller_abi/utils.h>

#include "decode.h"
#include "encode.h"
#include "ir.h"
#include "mod.h"

extern struct PyModuleDef libfidl_codec;
namespace fuchsia_controller::fidl_codec {
namespace {

namespace fc = fuchsia_controller;

constexpr PyMethodDef SENTINEL = {nullptr, nullptr, 0, nullptr};

PyMethodDef FidlCodecMethods[] = {
    decode::decode_fidl_response_py_def,
    decode::decode_fidl_request_py_def,
    decode::decode_standalone_py_def,
    encode::encode_fidl_message_py_def,
    encode::encode_fidl_object_py_def,
    ir::add_ir_path_py_def,
    ir::add_ir_paths_py_def,
    ir::get_ir_path_py_def,
    ir::get_method_ordinal_py_def,
    SENTINEL,
};

int FidlCodecModule_clear(PyObject *m) {
  auto state = reinterpret_cast<mod::FidlCodecState *>(PyModule_GetState(m));
  state->~FidlCodecState();
  return 0;
}

PyMODINIT_FUNC PyInit_libfidl_codec() {
  auto m = fc::abi::utils::Object(PyModule_Create(&libfidl_codec));
  if (m == nullptr) {
    return nullptr;
  }
  auto state = reinterpret_cast<mod::FidlCodecState *>(PyModule_GetState(m.get()));
  new (state) mod::FidlCodecState();
  return m.take();
}

}  // namespace

}  // namespace fuchsia_controller::fidl_codec

struct PyModuleDef libfidl_codec = {
    .m_base = PyModuleDef_HEAD_INIT,
    .m_name = "fidl_codec",
    .m_doc = nullptr,
    .m_size = sizeof(::fuchsia_controller::fidl_codec::mod::FidlCodecState *),
    .m_methods = ::fuchsia_controller::fidl_codec::FidlCodecMethods,
    .m_clear = ::fuchsia_controller::fidl_codec::FidlCodecModule_clear,
};
