// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "ir.h"

#include "mod.h"
#include "pyerrors.h"
#include "rapidjson/error/en.h"

namespace fuchsia_controller::fidl_codec::ir {

PyMethodDef add_ir_path_py_def = {
    "add_ir_path", _PyCFunction_CAST(add_ir_path), METH_O,
    "Adds the FIDL IR path to the module for use in encoding/decoding. Throws an exception if the "
    "file cannot be parsed, or if the file does not exist."};

PyMethodDef add_ir_paths_py_def = {"add_ir_paths", _PyCFunction_CAST(add_ir_paths), METH_O,
                                   "Adds the FIDL IR files to the module's IR library in a batch"};
PyMethodDef get_method_ordinal_py_def = {
    "method_ordinal", _PyCFunction_CAST(get_method_ordinal), METH_VARARGS | METH_KEYWORDS,
    "Gets the method ordinal number for the specified method. Method is intended to be a string "
    "formatted as a FIDL fully qualified name, e.g. '<library>/<declaration>.<member>'. For "
    "example: 'fuchsia.developer.ffx/EchoString'. More details can be found at "
    "https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0043_documentation_comment_format#fully-qualified-names"};

PyMethodDef get_ir_path_py_def = {
    "get_ir_path", _PyCFunction_CAST(get_ir_path), METH_O,
    "Attempts to get the path to the IR based on the library name. Raises an exception if it cannot be found"};

namespace {
PyObject *get_error(const ::fidl_codec::LibraryReadError &loader_err) {
  switch (loader_err.value) {
    case ::fidl_codec::LibraryReadError::kIoError: {
      PyErr_Format(PyExc_RuntimeError, "Unable to open fidl file. Error: %s",
                   strerror(loader_err.errno_value));
      return nullptr;
    }
    case ::fidl_codec::LibraryReadError::kParseError:
      PyErr_SetString(PyExc_RuntimeError,
                      rapidjson::GetParseError_En(loader_err.parse_result.Code()));
      return nullptr;
    case ::fidl_codec::LibraryReadError::kOk:
      break;
    default:
      PyErr_SetString(PyExc_RuntimeError, "Unhandled FIDL_codec return result");
      return nullptr;
  };
  Py_RETURN_NONE;
}
}  // namespace

PyObject *get_method_ordinal(PyObject *self, PyObject *args, PyObject *kwds) {  // NOLINT
  static const char *kwlist[] = {"protocol", "method", nullptr};
  const char *c_protocol;
  const char *c_method;
  if (!PyArg_ParseTupleAndKeywords(args, kwds, "ss", const_cast<char **>(kwlist), &c_protocol,
                                   &c_method)) {
    return nullptr;
  }
  std::string_view protocol(c_protocol);
  auto const idx = protocol.find('/');
  if (idx == std::string::npos) {
    PyErr_SetString(PyExc_ValueError,
                    "protocol not formatted properly, expected {library}/{protocol}");
    return nullptr;
  }
  const std::string library(c_protocol, idx);
  std::string_view method(c_method);
  auto lib = mod::get_ir_library(library);
  if (lib == nullptr) {
    return nullptr;
  }
  ::fidl_codec::Protocol *codec_protocol;
  if (!lib->GetProtocolByName(protocol, &codec_protocol)) {
    PyErr_Format(PyExc_RuntimeError, "Unable to find protocol %s under library %s", c_protocol,
                 library.c_str());
    return nullptr;
  }
  auto codec_method = codec_protocol->GetMethodByName(method);
  if (codec_method == nullptr) {
    PyErr_Format(PyExc_RuntimeError, "Unable to find method %s under protocol %s in library %s",
                 method, protocol, library.c_str());
    return nullptr;
  }
  return PyLong_FromUnsignedLongLong(codec_method->ordinal());
}

PyObject *add_ir_path(PyObject *self, PyObject *path_obj) {  // NOLINT
  const char *c_path = PyUnicode_AsUTF8AndSize(path_obj, nullptr);
  if (c_path == nullptr) {
    return nullptr;
  }
  auto path = std::string(c_path);
  ::fidl_codec::LibraryReadError loader_err;
  mod::get_module_state()->loader->AddPath(path, &loader_err);
  return get_error(loader_err);
}

PyObject *get_ir_path(PyObject *self, PyObject *library_name) {  // NOLINT
  const char *c_lib = PyUnicode_AsUTF8AndSize(library_name, nullptr);
  if (c_lib == nullptr) {
    return nullptr;
  }
  auto lib = std::string(c_lib);
  ::fidl_codec::Library *ir_lib = mod::get_ir_library(lib);
  if (ir_lib == nullptr) {
    return nullptr;
  }
  std::string_view ir_lib_source = ir_lib->source();
  if (ir_lib_source.empty()) {
    PyErr_Format(
        PyExc_RuntimeError,
        "Library '%s' is loaded but has no associated source file, implying it was loaded directly"
        "from a string",
        c_lib);
    return nullptr;
  }
  return PyUnicode_FromStringAndSize(ir_lib_source.data(),
                                     static_cast<Py_ssize_t>(ir_lib_source.size()));
}

PyObject *add_ir_paths(PyObject *self, PyObject *path_list) {  // NOLINT
  if (!PyList_Check(path_list)) {
    PyErr_SetString(PyExc_TypeError, "Expected path_list to be a list");
    return nullptr;
  }
  auto len = PyList_Size(path_list);
  std::vector<std::string> paths(len);
  for (Py_ssize_t i = 0; i < len; ++i) {
    auto elmnt = PyList_GetItem(path_list, i);
    if (elmnt == nullptr) {
      return nullptr;
    }
    const char *elmnt_str = PyUnicode_AsUTF8AndSize(elmnt, nullptr);
    if (elmnt_str == nullptr) {
      return nullptr;
    }
    paths[i] = std::string(elmnt_str);
  }
  ::fidl_codec::LibraryReadError loader_err;
  mod::get_module_state()->loader->AddAll(paths, &loader_err);
  return get_error(loader_err);
}

}  // namespace fuchsia_controller::fidl_codec::ir
