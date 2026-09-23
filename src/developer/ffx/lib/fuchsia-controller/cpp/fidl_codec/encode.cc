// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "encode.h"

#include <Python.h>

#include <string>

#include <fuchsia_controller_abi/utils.h>

#include "mod.h"
#include "object_converter.h"
#include "src/lib/fidl_codec/encoder.h"
#include "src/lib/fidl_codec/wire_types.h"
#include "utils.h"

namespace fuchsia_controller::fidl_codec::encode {

namespace fc = fuchsia_controller;

PyMethodDef encode_fidl_message_py_def = {
    "encode_fidl_message", _PyCFunction_CAST(encode_fidl_message), METH_VARARGS | METH_KEYWORDS,
    "Encodes the FIDL wire format representation of the object. "
    "The only necessary fields are txid and ordinal. Everything else can be set to None. "
    "If the object field is not None, then all parameters are required. "
    "If object is None, other optional parameters will be ignored. Returns a tuple. The first item "
    "is a bytearray, the second is a tuple representing a handle disposition containing, in order "
    " the operation, handle, type, rights, and result all as integers"};

PyMethodDef encode_fidl_object_py_def = {
    "encode_fidl_object", _PyCFunction_CAST(encode_fidl_object), METH_VARARGS | METH_KEYWORDS,
    "Encodes the FIDL wire format representation of the object. "
    "Returns a tuple. The first item in the tuple is a bytearray, the second "
    "is a an array representing the handle dispositions, each of which "
    "contains, in order, the operation, handle, type, rights, and result as "
    "integers."};

struct GetPayloadTypeArgs {
  PyObject *obj;
  PyObject *type_name_obj;
  PyObject *library_obj;
};

namespace {
std::unique_ptr<::fidl_codec::Type> GetPayloadType(GetPayloadTypeArgs args) {
  if (args.obj == Py_None) {
    return std::make_unique<::fidl_codec::EmptyPayloadType>();
  }
  const char *lib_c_str = PyUnicode_AsUTF8AndSize(args.library_obj, nullptr);
  if (lib_c_str == nullptr) {
    return nullptr;
  }
  const char *type_name_c_str = PyUnicode_AsUTF8AndSize(args.type_name_obj, nullptr);
  if (type_name_c_str == nullptr) {
    return nullptr;
  }
  std::string lib_str(lib_c_str);
  std::string type_name_str(type_name_c_str);
  auto library = mod::get_ir_library(lib_str);
  if (library == nullptr) {
    return nullptr;
  }
  auto type = library->TypeFromIdentifier(false, type_name_str);
  if (type == nullptr || !type->IsValid()) {
    PyErr_Format(PyExc_RuntimeError, "Unrecognized type: '%s'", type_name_c_str);
    return nullptr;
  }
  return type;
}
}  // namespace

// NOLINTNEXTLINE: similarly typed parameters are unavoidable in Python.
PyObject *encode_fidl_message(PyObject *self, PyObject *args, PyObject *kwds) {
  static constexpr uint8_t HEADER_MAGIC = 1;
  static constexpr uint8_t AT_REST_FLAGS[2] = {FIDL_MESSAGE_HEADER_AT_REST_FLAGS_0_USE_VERSION_V2,
                                               0};
  static constexpr uint8_t DYNAMIC_FLAGS = 0;

  static const char *kwlist[] = {"object", "library", "type_name", "txid", "ordinal", nullptr};
  PyObject *obj = nullptr;
  PyObject *library_obj = nullptr;
  PyObject *type_name_obj = nullptr;
  PyObject *ordinal_obj = nullptr;
  PyObject *txid_obj = nullptr;
  if (!PyArg_ParseTupleAndKeywords(args, kwds, "OOOOO", const_cast<char **>(kwlist), &obj,
                                   &library_obj, &type_name_obj, &txid_obj, &ordinal_obj)) {
    return nullptr;
  }

  if (ordinal_obj == Py_None || txid_obj == Py_None) {
    PyErr_SetString(PyExc_TypeError, "ordinal and txid must not be None");
    return nullptr;
  }

  auto ordinal = utils::PyLong_AsU64(ordinal_obj);
  if (ordinal == utils::MINUS_ONE_U64 && PyErr_Occurred()) {
    return nullptr;
  }

  auto txid = utils::PyLong_AsU32(txid_obj);
  if (txid == utils::MINUS_ONE_U32 && PyErr_Occurred()) {
    return nullptr;
  }

  auto type = GetPayloadType(GetPayloadTypeArgs{
      .obj = obj,
      .type_name_obj = type_name_obj,
      .library_obj = library_obj,
  });
  if (type == nullptr) {
    return nullptr;
  }
  auto converted = converter::ObjectConverter::Convert(obj, type.get());
  if (converted == nullptr) {
    return nullptr;
  }
  auto msg = ::fidl_codec::Encoder::EncodeMessage(txid, ordinal, AT_REST_FLAGS, DYNAMIC_FLAGS,
                                                  HEADER_MAGIC, converted.get(), type.get());
  auto res = fc::abi::utils::Object(PyTuple_New(2));
  if (res == nullptr) {
    return nullptr;
  }
  auto buf = fc::abi::utils::Object(PyByteArray_FromStringAndSize(
      reinterpret_cast<const char *>(msg.bytes.data()), static_cast<Py_ssize_t>(msg.bytes.size())));
  if (buf == nullptr) {
    return nullptr;
  }
  auto handles_list =
      fc::abi::utils::Object(PyList_New(static_cast<Py_ssize_t>(msg.handles.size())));
  if (handles_list == nullptr) {
    return nullptr;
  }
  PyTuple_SetItem(res.get(), 0, buf.take());
  for (uint64_t i = 0; i < msg.handles.size(); ++i) {
    // This is currently done as a tuple, could also be done as a dict for better readability.
    auto handle_tuple = fc::abi::utils::Object(PyTuple_New(5));
    auto handle_disp = msg.handles[i];
    auto operation = fc::abi::utils::Object(PyLong_FromLong(handle_disp.operation));
    if (operation == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 0, operation.take());
    auto handle_value = fc::abi::utils::Object(PyLong_FromLong(handle_disp.handle));
    if (handle_value == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 1, handle_value.take());
    auto type = fc::abi::utils::Object(PyLong_FromLong(handle_disp.type));
    if (type == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 2, type.take());
    auto rights = fc::abi::utils::Object(PyLong_FromLong(handle_disp.rights));
    if (rights == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 3, rights.take());
    auto result = fc::abi::utils::Object(PyLong_FromLong(handle_disp.result));
    if (result == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 4, result.take());
    PyList_SetItem(handles_list.get(), static_cast<Py_ssize_t>(i), handle_tuple.take());
  }
  PyTuple_SetItem(res.get(), 1, handles_list.take());
  return res.take();
}

// NOLINTNEXTLINE: similarly typed parameters are unavoidable in Python.
PyObject *encode_fidl_object(PyObject *self, PyObject *args, PyObject *kwds) {
  static const char *kwlist[] = {"object", "library_name", "type_name", nullptr};
  PyObject *obj = nullptr;
  PyObject *library_obj = nullptr;
  PyObject *type_name_obj = nullptr;
  if (!PyArg_ParseTupleAndKeywords(args, kwds, "OOO", const_cast<char **>(kwlist), &obj,
                                   &library_obj, &type_name_obj)) {
    return nullptr;
  }
  auto type = GetPayloadType(GetPayloadTypeArgs{
      .obj = obj,
      .type_name_obj = type_name_obj,
      .library_obj = library_obj,
  });
  if (type == nullptr) {
    return nullptr;
  }
  auto converted = converter::ObjectConverter::Convert(obj, type.get());
  if (converted == nullptr) {
    return nullptr;
  }
  auto msg = ::fidl_codec::Encoder::EncodeObject(converted.get(), type.get());
  auto res = fc::abi::utils::Object(PyTuple_New(2));
  if (res == nullptr) {
    return nullptr;
  }
  auto buf = fc::abi::utils::Object(PyByteArray_FromStringAndSize(
      reinterpret_cast<const char *>(msg.bytes.data()), static_cast<Py_ssize_t>(msg.bytes.size())));
  if (buf == nullptr) {
    return nullptr;
  }
  auto handles_list =
      fc::abi::utils::Object(PyList_New(static_cast<Py_ssize_t>(msg.handles.size())));
  if (handles_list == nullptr) {
    return nullptr;
  }
  PyTuple_SetItem(res.get(), 0, buf.take());
  for (uint64_t i = 0; i < msg.handles.size(); ++i) {
    // This is currently done as a tuple, could also be done as a dict for better readability.
    auto handle_tuple = fc::abi::utils::Object(PyTuple_New(5));
    auto handle_disp = msg.handles[i];
    auto operation = fc::abi::utils::Object(PyLong_FromLong(handle_disp.operation));
    if (operation == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 0, operation.take());
    auto handle_value = fc::abi::utils::Object(PyLong_FromLong(handle_disp.handle));
    if (handle_value == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 1, handle_value.take());
    auto type = fc::abi::utils::Object(PyLong_FromLong(handle_disp.type));
    if (type == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 2, type.take());
    auto rights = fc::abi::utils::Object(PyLong_FromLong(handle_disp.rights));
    if (rights == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 3, rights.take());
    auto result = fc::abi::utils::Object(PyLong_FromLong(handle_disp.result));
    if (result == nullptr) {
      return nullptr;
    }
    PyTuple_SetItem(handle_tuple.get(), 4, result.take());
    PyList_SetItem(handles_list.get(), static_cast<Py_ssize_t>(i), handle_tuple.take());
  }
  PyTuple_SetItem(res.get(), 1, handles_list.take());
  return res.take();
}

}  // namespace fuchsia_controller::fidl_codec::encode
