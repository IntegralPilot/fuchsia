// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "decode.h"

#include <Python.h>

#include <cinttypes>
#include <vector>

#include <fuchsia_controller_abi/utils.h>

#include "mod.h"
#include "python_dict_visitor.h"
#include "src/lib/fidl_codec/wire_parser.h"
#include "utils.h"

namespace fuchsia_controller::fidl_codec::decode {

enum Direction : uint8_t { REQUEST, RESPONSE };

namespace {

std::unique_ptr<zx_handle_disposition_t[]> python_obj_to_handle_disps(PyObject *handles,
                                                                      Py_ssize_t *c_handles_len) {
  if (!PyList_Check(handles)) {
    PyErr_SetString(PyExc_TypeError, "Expected handles to be a list");
    return nullptr;
  }
  *c_handles_len = PyList_Size(handles);
  std::unique_ptr<zx_handle_disposition_t[]> c_handles =
      std::make_unique<zx_handle_disposition_t[]>(*c_handles_len);
  for (Py_ssize_t i = 0; i < *c_handles_len; ++i) {
    auto obj = PyList_GetItem(handles, i);
    if (obj == nullptr) {
      return nullptr;
    }
    zx_handle_t res = utils::PyLong_AsU32(obj);
    if (res == utils::MINUS_ONE_U32 && PyErr_Occurred()) {
      return nullptr;
    }
    c_handles[i] = zx_handle_disposition_t{.handle = res};
  }
  return c_handles;
}

}  // namespace

PyObject *decode_fidl_message(PyObject *self, PyObject *args, PyObject *kwds,  // NOLINT
                              Direction direction) {
  PyObject *handles = nullptr;
  Py_buffer bytes_raw;
  static const char *kwlist[] = {
      "bytes",
      "handles",
      nullptr,
  };
  if (!PyArg_ParseTupleAndKeywords(args, kwds, "y*O", const_cast<char **>(kwlist), &bytes_raw,
                                   &handles)) {
    return nullptr;
  }
  Py_ssize_t c_handles_len;
  auto c_handles = python_obj_to_handle_disps(handles, &c_handles_len);
  if (c_handles == nullptr) {
    return nullptr;
  }
  utils::Buffer bytes(bytes_raw);
  auto header = static_cast<const fidl_message_header_t *>(bytes.buf());
  const std::vector<::fidl_codec::ProtocolMethod *> *methods =
      mod::get_module_state()->loader->GetByOrdinal(header->ordinal);
  if (methods == nullptr || methods->empty()) {
    PyErr_Format(PyExc_LookupError, "Unable to find any methods for method ordinal: %" PRIu64,
                 header->ordinal);
    return nullptr;
  }
  // What is the approach here if there's more than one method?
  const ::fidl_codec::ProtocolMethod *method = (*methods)[0];
  std::unique_ptr<::fidl_codec::Value> object;
  std::ostringstream errors;
  bool successful;
  switch (direction) {
    case Direction::RESPONSE:
      successful = ::fidl_codec::DecodeResponse(
          method, static_cast<const uint8_t *>(bytes.buf()), static_cast<uint64_t>(bytes.len()),
          c_handles.get(), static_cast<uint64_t>(c_handles_len), &object, errors);
      break;
    case Direction::REQUEST:
      successful = ::fidl_codec::DecodeRequest(
          method, static_cast<const uint8_t *>(bytes.buf()), static_cast<uint64_t>(bytes.len()),
          c_handles.get(), static_cast<uint64_t>(c_handles_len), &object, errors);
      break;
  }
  if (!successful) {
    PyErr_SetString(PyExc_IOError, errors.str().c_str());
    return nullptr;
  }
  if (object == nullptr) {
    PyErr_SetString(PyExc_RuntimeError, "Parsed object is null");
    return nullptr;
  }
  python_dict_visitor::PythonDictVisitor visitor;
  object->Visit(&visitor, nullptr);
  return visitor.result();
}

PyObject *decode_standalone(PyObject *self, PyObject *args, PyObject *kwds) {  // NOLINT
  const char *c_type_name = nullptr;
  PyObject *handles = nullptr;
  Py_buffer bytes_raw;
  static const char *kwlist[] = {
      "type_name",
      "bytes",
      "handles",
      nullptr,
  };
  if (!PyArg_ParseTupleAndKeywords(args, kwds, "sy*O", const_cast<char **>(kwlist), &c_type_name,
                                   &bytes_raw, &handles)) {
    return nullptr;
  }
  std::string type_name(c_type_name);
  auto const idx = type_name.find('/');
  if (idx == std::string::npos) {
    PyErr_SetString(PyExc_ValueError,
                    "Protocol not formatted properly, expected {library}/{protocol}");
    return nullptr;
  }
  const std::string library_name(c_type_name, idx);
  utils::Buffer bytes(bytes_raw);
  Py_ssize_t c_handles_len;
  auto c_handles = python_obj_to_handle_disps(handles, &c_handles_len);
  if (c_handles == nullptr) {
    return nullptr;
  }
  auto lib = mod::get_ir_library(library_name);
  if (lib == nullptr) {
    return nullptr;
  }
  auto type = lib->TypeFromIdentifier(false, type_name);
  if (type == nullptr) {
    PyErr_Format(PyExc_RuntimeError, "Invalid value returned for %s in library %s",
                 type_name.c_str(), library_name.c_str());
    return nullptr;
  }
  ::fidl_codec::InvalidType invalid_type;
  if (type->Name() == invalid_type.Name()) {
    PyErr_Format(PyExc_TypeError, "Unable to find type %s in library %s", type_name.c_str(),
                 library_name.c_str());
    return nullptr;
  }
  std::ostringstream errors;
  ::fidl_codec::MessageDecoder decoder(static_cast<uint8_t *>(bytes.buf()), bytes.len(),
                                       c_handles.get(), c_handles_len, errors);
  decoder.SkipObject(type->InlineSize(::fidl_codec::WireVersion::kWireV2));
  auto value = type->Decode(&decoder, 0);
  if (value == nullptr || decoder.HasError()) {
    PyErr_SetString(PyExc_RuntimeError, errors.str().c_str());
    return nullptr;
  }
  python_dict_visitor::PythonDictVisitor visitor;
  value->Visit(&visitor, nullptr);
  return visitor.result();
}

PyObject *decode_fidl_response(PyObject *self, PyObject *args, PyObject *kwds) {  // NOLINT
  return decode_fidl_message(self, args, kwds, Direction::RESPONSE);
}

PyObject *decode_fidl_request(PyObject *self, PyObject *args, PyObject *kwds) {  // NOLINT
  return decode_fidl_message(self, args, kwds, Direction::REQUEST);
}

PyMethodDef decode_standalone_py_def = {
    "decode_standalone", _PyCFunction_CAST(decode_standalone), METH_VARARGS | METH_KEYWORDS,
    "Decodes a standalone fidl response based on its type, bytes, and handles."};

PyMethodDef decode_fidl_response_py_def = {
    "decode_fidl_response", _PyCFunction_CAST(decode_fidl_response), METH_VARARGS | METH_KEYWORDS,
    "Decodes a FIDL response message from bytes and handles."};

PyMethodDef decode_fidl_request_py_def = {
    "decode_fidl_request", _PyCFunction_CAST(decode_fidl_request), METH_VARARGS | METH_KEYWORDS,
    "Decodes a FIDL request message from bytes and handles."};

}  // namespace fuchsia_controller::fidl_codec::decode
