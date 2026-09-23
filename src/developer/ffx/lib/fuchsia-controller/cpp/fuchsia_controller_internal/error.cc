// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "error.h"

#include <Python.h>
#include <structmember.h>  // PyMemberDef.

#include <sstream>
#include <string>

#include <fuchsia_controller_abi/utils.h>

#include "fuchsia_controller.h"

namespace error {

namespace fc = fuchsia_controller;

std::string zx_status_get_string(zx_status_t status) {
  switch (status) {
    case ZX_OK:
      return "ZX_OK";
    case ZX_ERR_INTERNAL:
      return "ZX_ERR_INTERNAL";
    case ZX_ERR_NOT_SUPPORTED:
      return "ZX_ERR_NOT_SUPPORTED";
    case ZX_ERR_NO_RESOURCES:
      return "ZX_ERR_NO_RESOURCES";
    case ZX_ERR_NO_MEMORY:
      return "ZX_ERR_NO_MEMORY";
    case ZX_ERR_INTERNAL_INTR_RETRY:
      return "ZX_ERR_INTERNAL_INTR_RETRY";
    case ZX_ERR_INVALID_ARGS:
      return "ZX_ERR_INVALID_ARGS";
    case ZX_ERR_BAD_HANDLE:
      return "ZX_ERR_BAD_HANDLE";
    case ZX_ERR_WRONG_TYPE:
      return "ZX_ERR_WRONG_TYPE";
    case ZX_ERR_BAD_SYSCALL:
      return "ZX_ERR_BAD_SYSCALL";
    case ZX_ERR_OUT_OF_RANGE:
      return "ZX_ERR_OUT_OF_RANGE";
    case ZX_ERR_BUFFER_TOO_SMALL:
      return "ZX_ERR_BUFFER_TOO_SMALL";
    case ZX_ERR_BAD_STATE:
      return "ZX_ERR_BAD_STATE";
    case ZX_ERR_TIMED_OUT:
      return "ZX_ERR_TIMED_OUT";
    case ZX_ERR_SHOULD_WAIT:
      return "ZX_ERR_SHOULD_WAIT";
    case ZX_ERR_CANCELED:
      return "ZX_ERR_CANCELED";
    case ZX_ERR_PEER_CLOSED:
      return "ZX_ERR_PEER_CLOSED";
    case ZX_ERR_NOT_FOUND:
      return "ZX_ERR_NOT_FOUND";
    case ZX_ERR_ALREADY_EXISTS:
      return "ZX_ERR_ALREADY_EXISTS";
    case ZX_ERR_ALREADY_BOUND:
      return "ZX_ERR_ALREADY_BOUND";
    case ZX_ERR_UNAVAILABLE:
      return "ZX_ERR_UNAVAILABLE";
    case ZX_ERR_ACCESS_DENIED:
      return "ZX_ERR_ACCESS_DENIED";
    case ZX_ERR_IO:
      return "ZX_ERR_IO";
    case ZX_ERR_IO_REFUSED:
      return "ZX_ERR_IO_REFUSED";
    case ZX_ERR_IO_DATA_INTEGRITY:
      return "ZX_ERR_IO_DATA_INTEGRITY";
    case ZX_ERR_IO_DATA_LOSS:
      return "ZX_ERR_IO_DATA_LOSS";
    case ZX_ERR_IO_NOT_PRESENT:
      return "ZX_ERR_IO_NOT_PRESENT";
    case ZX_ERR_IO_OVERRUN:
      return "ZX_ERR_IO_OVERRUN";
    case ZX_ERR_IO_MISSED_DEADLINE:
      return "ZX_ERR_IO_MISSED_DEADLINE";
    case ZX_ERR_IO_INVALID:
      return "ZX_ERR_IO_INVALID";
    case ZX_ERR_BAD_PATH:
      return "ZX_ERR_BAD_PATH";
    case ZX_ERR_NOT_DIR:
      return "ZX_ERR_NOT_DIR";
    case ZX_ERR_NOT_FILE:
      return "ZX_ERR_NOT_FILE";
    case ZX_ERR_FILE_BIG:
      return "ZX_ERR_FILE_BIG";
    case ZX_ERR_NO_SPACE:
      return "ZX_ERR_NO_SPACE";
    case ZX_ERR_NOT_EMPTY:
      return "ZX_ERR_NOT_EMPTY";
    case ZX_ERR_STOP:
      return "ZX_ERR_STOP";
    case ZX_ERR_NEXT:
      return "ZX_ERR_NEXT";
    case ZX_ERR_ASYNC:
      return "ZX_ERR_ASYNC";
    case ZX_ERR_PROTOCOL_NOT_SUPPORTED:
      return "ZX_ERR_PROTOCOL_NOT_SUPPORTED";
    case ZX_ERR_ADDRESS_UNREACHABLE:
      return "ZX_ERR_ADDRESS_UNREACHABLE";
    case ZX_ERR_ADDRESS_IN_USE:
      return "ZX_ERR_ADDRESS_IN_USE";
    case ZX_ERR_NOT_CONNECTED:
      return "ZX_ERR_NOT_CONNECTED";
    case ZX_ERR_CONNECTION_REFUSED:
      return "ZX_ERR_CONNECTION_REFUSED";
    case ZX_ERR_CONNECTION_RESET:
      return "ZX_ERR_CONNECTION_RESET";
    case ZX_ERR_CONNECTION_ABORTED:
      return "ZX_ERR_CONNECTION_ABORTED";
    default:
      // In the event that this has bottomed out, add some debug info with the
      // raw error code.
      std::ostringstream ss;
      ss << "(UNKNOWN: " << status << ")";
      return ss.str();
  }
}

namespace {

std::string fc_status_get_string(fc_status_t status) {
  switch (status) {
    case FC_OK:
      return "FC_OK";
    case FC_ERR_INVALID_ARGS:
      return "FC_ERR_INVALID_ARGS";
    case FC_ERR_NOT_SUPPORTED:
      return "FC_ERR_NOT_SUPPORTED";
    case FC_ERR_NOT_FOUND:
      return "FC_ERR_NOT_FOUND";
    case FC_ERR_BUFFER_TOO_SMALL:
      return "FC_ERR_BUFFER_TOO_SMALL";
    case FC_ERR_SHOULD_WAIT:
      return "FC_ERR_SHOULD_WAIT";
    case FC_ERR_INTERNAL:
      return "FC_ERR_INTERNAL";
    case FC_ERR_SOCKET_WRITE:
      return "FC_ERR_SOCKET_WRITE";
    case FC_ERR_CHANNEL_WRITE:
      return "FC_ERR_CHANNEL_WRITE";
    case FC_ERR_FDOMAIN:
      return "FC_ERR_FDOMAIN";
    case FC_ERR_PROTOCOL:
      return "FC_ERR_PROTOCOL";
    case FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE:
      return "FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE";
    case FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE:
      return "FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE";
    case FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE:
      return "FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE";
    case FC_ERR_TRANSPORT:
      return "FC_ERR_TRANSPORT";
    case FC_ERR_CONNECTION_MISMATCH:
      return "FC_ERR_CONNECTION_MISMATCH";
    case FC_ERR_STREAMING_ABORTED:
      return "FC_ERR_STREAMING_ABORTED";
    default:
      // In the event that this has bottomed out, add some debug info.
      std::ostringstream ss;
      ss << "(UNKNOWN: " << status
         << ". As ZxStatus this is: " << zx_status_get_string(static_cast<zx_status_t>(status))
         << ")";
      return ss.str();
  }
}

// This was copied and macro'd from fuchsia_controller.h
PyObject *FcTransportStatus_make_constants() {
  auto dict = fc::abi::utils::Object(PyDict_New());
  if (dict == nullptr) {
    return nullptr;
  }
  if (PyDict_SetItemString(dict.get(), "FC_OK", PyLong_FromLong(FC_OK)) < 0) {
    return nullptr;
  }
  PyDict_SetItemString(dict.get(), "FC_OK", PyLong_FromLong(FC_OK));
  PyDict_SetItemString(dict.get(), "FC_ERR_INVALID_ARGS", PyLong_FromLong(FC_ERR_INVALID_ARGS));
  PyDict_SetItemString(dict.get(), "FC_ERR_NOT_SUPPORTED", PyLong_FromLong(FC_ERR_NOT_SUPPORTED));
  PyDict_SetItemString(dict.get(), "FC_ERR_NOT_FOUND", PyLong_FromLong(FC_ERR_NOT_FOUND));
  PyDict_SetItemString(dict.get(), "FC_ERR_BUFFER_TOO_SMALL",
                       PyLong_FromLong(FC_ERR_BUFFER_TOO_SMALL));
  PyDict_SetItemString(dict.get(), "FC_ERR_SHOULD_WAIT", PyLong_FromLong(FC_ERR_SHOULD_WAIT));
  PyDict_SetItemString(dict.get(), "FC_ERR_INTERNAL", PyLong_FromLong(FC_ERR_INTERNAL));
  PyDict_SetItemString(dict.get(), "FC_ERR_SOCKET_WRITE", PyLong_FromLong(FC_ERR_SOCKET_WRITE));
  PyDict_SetItemString(dict.get(), "FC_ERR_CHANNEL_WRITE", PyLong_FromLong(FC_ERR_CHANNEL_WRITE));
  PyDict_SetItemString(dict.get(), "FC_ERR_FDOMAIN", PyLong_FromLong(FC_ERR_FDOMAIN));
  PyDict_SetItemString(dict.get(), "FC_ERR_PROTOCOL", PyLong_FromLong(FC_ERR_PROTOCOL));
  PyDict_SetItemString(dict.get(), "FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE",
                       PyLong_FromLong(FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE));
  PyDict_SetItemString(dict.get(), "FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE",
                       PyLong_FromLong(FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE));
  PyDict_SetItemString(dict.get(), "FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE",
                       PyLong_FromLong(FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE));
  PyDict_SetItemString(dict.get(), "FC_ERR_TRANSPORT", PyLong_FromLong(FC_ERR_TRANSPORT));
  PyDict_SetItemString(dict.get(), "FC_ERR_CONNECTION_MISMATCH",
                       PyLong_FromLong(FC_ERR_CONNECTION_MISMATCH));
  PyDict_SetItemString(dict.get(), "FC_ERR_STREAMING_ABORTED",
                       PyLong_FromLong(FC_ERR_STREAMING_ABORTED));
  return dict.take();
}

std::string FcTransportStatus_reprstr_helper(PyObject *self) {
  if (!PyObject_HasAttrString(self, "args")) {
    return "unknown: object has no args";
  }
  auto args = fc::abi::utils::Object(PyObject_GetAttrString(self, "args"));
  if (args == nullptr || PyTuple_Size(args.get()) < 1) {
    return "unknown: no args set";
  }
  std::stringstream ss;
  PyObject *i = PyTuple_GetItem(args.get(), 0);
  if (!PyLong_Check(i)) {
    return "unknown: non-int error code in arg[0]";
  }
  ss << fc_status_get_string(static_cast<fc_status_t>(PyLong_AsLong(i)));
  PyObject *str = PyTuple_GetItem(args.get(), 1);
  // In the event that there are some types being created manually, e.g. there
  // are no string args, return the string we have so far.
  if (str == nullptr) {
    PyErr_Clear();
    return ss.str();
  }
  if (PyUnicode_Check(str)) {
    const char *val = PyUnicode_AsUTF8AndSize(str, nullptr);
    if (val != nullptr) {
      auto view = std::string_view(val);
      ss << ": " << view;
    }
  }
  return ss.str();
}

PyObject *FcTransportStatus_repr(PyObject *self, PyTypeObject *defining_class,
                                 PyObject *const *args, Py_ssize_t nargs, PyObject *kwnames) {
  return PyUnicode_FromString(FcTransportStatus_reprstr_helper(self).c_str());
}

PyObject *FcTransportStatus_str(PyObject *self) {
  std::stringstream ss;
  ss << FcTransportStatus_reprstr_helper(self);
  return PyUnicode_FromString(ss.str().c_str());
}

PyMethodDef FcTransportStatus_repr_def = {
    "__repr__",
    _PyCFunction_CAST(FcTransportStatus_repr),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

PyMethodDef FcTransportStatus_str_def = {
    "__str__",
    _PyCFunction_CAST(FcTransportStatus_str),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

PyObject *ZxStatus_raw(PyObject *self) {
  auto args = fc::abi::utils::Object(PyObject_GetAttrString(self, "args"));
  if (args == nullptr) {
    return nullptr;
  }
  if (PyTuple_Size(args.get()) < 1) {
    PyErr_Format(PyExc_RuntimeError, "class does not have any arguments set. This is a BUG.");
    return nullptr;
  }
  PyObject *i = PyTuple_GetItem(args.get(), 0);
  if (!PyLong_Check(i)) {
    PyErr_Format(PyExc_RuntimeError, "class does not have an integer argument set. This is a BUG");
    return nullptr;
  }
  // This is a BORROWED reference. Need to increment it so it doesn't get garbage collected by
  // accident.
  Py_INCREF(i);
  return i;
}

PyObject *FcTransportStatus_desc(PyObject *self) {
  auto args = fc::abi::utils::Object(PyObject_GetAttrString(self, "args"));
  if (args == nullptr) {
    return nullptr;
  }
  // Want to make sure there's at least SOMETHING set.
  if (PyTuple_Size(args.get()) < 1) {
    PyErr_Format(PyExc_RuntimeError, "class does not have any arguments set. This is a BUG.");
    return nullptr;
  }
  // If we only have a code, we have no description, so just return None.
  if (PyTuple_Size(args.get()) < 2) {
    Py_RETURN_NONE;
  }
  PyObject *i = PyTuple_GetItem(args.get(), 1);
  if (!PyUnicode_Check(i)) {
    PyErr_Format(PyExc_RuntimeError,
                 "class does not have a string argument set for the description. This is a BUG");
    return nullptr;
  }
  // This is a BORROWED reference, so we need to increment it to prevent accidental garbage
  // collection.
  Py_INCREF(i);
  return i;
}

PyMethodDef FcTransportStatus_desc_def = {
    "desc",
    _PyCFunction_CAST(FcTransportStatus_desc),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

PyMethodDef FcTransportStatus_code_def = {
    "code",
    _PyCFunction_CAST(ZxStatus_raw),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

std::string ZxStatus_reprstr_helper(PyObject *self) {
  if (!PyObject_HasAttrString(self, "args")) {
    return "";
  }
  auto args = fc::abi::utils::Object(PyObject_GetAttrString(self, "args"));
  if (args == nullptr || PyTuple_Size(args.get()) < 1) {
    return "unknown";
  }
  std::stringstream ss;
  PyObject *i = PyTuple_GetItem(args.get(), 0);
  if (!PyLong_Check(i)) {
    return "unknown";
  }
  ss << zx_status_get_string(static_cast<zx_status_t>(PyLong_AsLong(i)));
  return ss.str();
}

PyObject *ZxStatus_repr(PyObject *self, PyTypeObject *defining_class, PyObject *const *args,
                        Py_ssize_t nargs, PyObject *kwnames) {
  return PyUnicode_FromString(ZxStatus_reprstr_helper(self).c_str());
}

PyObject *ZxStatus_str(PyObject *self) {
  std::stringstream ss;
  ss << "FIDL status: " << ZxStatus_reprstr_helper(self);
  return PyUnicode_FromString(ss.str().c_str());
}

PyMethodDef ZxStatus_repr_def = {
    "__repr__",
    _PyCFunction_CAST(ZxStatus_repr),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

PyMethodDef ZxStatus_str_def = {
    "__str__",
    _PyCFunction_CAST(ZxStatus_str),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

PyMethodDef ZxStatus_raw_def = {
    "raw",
    _PyCFunction_CAST(ZxStatus_raw),
    METH_METHOD | METH_FASTCALL | METH_KEYWORDS,
    nullptr,
};

// This was copied and macro'd from the rust zx_status files.
PyObject *ZxStatus_make_constants() {
  auto dict = fc::abi::utils::Object(PyDict_New());
  if (dict == nullptr) {
    return nullptr;
  }
  if (PyDict_SetItemString(dict.get(), "ZX_OK", PyLong_FromLong(ZX_OK)) < 0) {
    return nullptr;
  }
  PyDict_SetItemString(dict.get(), "ZX_ERR_INTERNAL", PyLong_FromLong(ZX_ERR_INTERNAL));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_SUPPORTED", PyLong_FromLong(ZX_ERR_NOT_SUPPORTED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NO_RESOURCES", PyLong_FromLong(ZX_ERR_NO_RESOURCES));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NO_MEMORY", PyLong_FromLong(ZX_ERR_NO_MEMORY));
  PyDict_SetItemString(dict.get(), "ZX_ERR_INVALID_ARGS", PyLong_FromLong(ZX_ERR_INVALID_ARGS));
  PyDict_SetItemString(dict.get(), "ZX_ERR_BAD_HANDLE", PyLong_FromLong(ZX_ERR_BAD_HANDLE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_WRONG_TYPE", PyLong_FromLong(ZX_ERR_WRONG_TYPE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_BAD_SYSCALL", PyLong_FromLong(ZX_ERR_BAD_SYSCALL));
  PyDict_SetItemString(dict.get(), "ZX_ERR_OUT_OF_RANGE", PyLong_FromLong(ZX_ERR_OUT_OF_RANGE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_BUFFER_TOO_SMALL",
                       PyLong_FromLong(ZX_ERR_BUFFER_TOO_SMALL));
  PyDict_SetItemString(dict.get(), "ZX_ERR_BAD_STATE", PyLong_FromLong(ZX_ERR_BAD_STATE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_TIMED_OUT", PyLong_FromLong(ZX_ERR_TIMED_OUT));
  PyDict_SetItemString(dict.get(), "ZX_ERR_SHOULD_WAIT", PyLong_FromLong(ZX_ERR_SHOULD_WAIT));
  PyDict_SetItemString(dict.get(), "ZX_ERR_CANCELED", PyLong_FromLong(ZX_ERR_CANCELED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_PEER_CLOSED", PyLong_FromLong(ZX_ERR_PEER_CLOSED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_FOUND", PyLong_FromLong(ZX_ERR_NOT_FOUND));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ALREADY_EXISTS", PyLong_FromLong(ZX_ERR_ALREADY_EXISTS));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ALREADY_BOUND", PyLong_FromLong(ZX_ERR_ALREADY_BOUND));
  PyDict_SetItemString(dict.get(), "ZX_ERR_UNAVAILABLE", PyLong_FromLong(ZX_ERR_UNAVAILABLE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ACCESS_DENIED", PyLong_FromLong(ZX_ERR_ACCESS_DENIED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO", PyLong_FromLong(ZX_ERR_IO));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_REFUSED", PyLong_FromLong(ZX_ERR_IO_REFUSED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_DATA_INTEGRITY",
                       PyLong_FromLong(ZX_ERR_IO_DATA_INTEGRITY));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_DATA_LOSS", PyLong_FromLong(ZX_ERR_IO_DATA_LOSS));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_NOT_PRESENT", PyLong_FromLong(ZX_ERR_IO_NOT_PRESENT));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_OVERRUN", PyLong_FromLong(ZX_ERR_IO_OVERRUN));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_MISSED_DEADLINE",
                       PyLong_FromLong(ZX_ERR_IO_MISSED_DEADLINE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_IO_INVALID", PyLong_FromLong(ZX_ERR_IO_INVALID));
  PyDict_SetItemString(dict.get(), "ZX_ERR_BAD_PATH", PyLong_FromLong(ZX_ERR_BAD_PATH));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_DIR", PyLong_FromLong(ZX_ERR_NOT_DIR));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_FILE", PyLong_FromLong(ZX_ERR_NOT_FILE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_FILE_BIG", PyLong_FromLong(ZX_ERR_FILE_BIG));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NO_SPACE", PyLong_FromLong(ZX_ERR_NO_SPACE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_EMPTY", PyLong_FromLong(ZX_ERR_NOT_EMPTY));
  PyDict_SetItemString(dict.get(), "ZX_ERR_STOP", PyLong_FromLong(ZX_ERR_STOP));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NEXT", PyLong_FromLong(ZX_ERR_NEXT));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ASYNC", PyLong_FromLong(ZX_ERR_ASYNC));
  PyDict_SetItemString(dict.get(), "ZX_ERR_PROTOCOL_NOT_SUPPORTED",
                       PyLong_FromLong(ZX_ERR_PROTOCOL_NOT_SUPPORTED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ADDRESS_UNREACHABLE",
                       PyLong_FromLong(ZX_ERR_ADDRESS_UNREACHABLE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_ADDRESS_IN_USE", PyLong_FromLong(ZX_ERR_ADDRESS_IN_USE));
  PyDict_SetItemString(dict.get(), "ZX_ERR_NOT_CONNECTED", PyLong_FromLong(ZX_ERR_NOT_CONNECTED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_CONNECTION_REFUSED",
                       PyLong_FromLong(ZX_ERR_CONNECTION_REFUSED));
  PyDict_SetItemString(dict.get(), "ZX_ERR_CONNECTION_RESET",
                       PyLong_FromLong(ZX_ERR_CONNECTION_RESET));
  PyDict_SetItemString(dict.get(), "ZX_ERR_CONNECTION_ABORTED",
                       PyLong_FromLong(ZX_ERR_CONNECTION_ABORTED));
  return dict.take();
}

}  // namespace

PyTypeObject *FcTransportStatusType = nullptr;
PyTypeObject *ZxStatusType = nullptr;

// This is necessary as Python exception extensions in C are weird. Python exceptions can't be
// wholesale subclassed from the base exception class, as they are expected to have an args
// attribute from which they are constructed. Attempting to make any sort of non-trivial subclassing
// of the Python error subclass will result in segfaulting.
PyTypeObject *ZxStatusType_Create() {
  assert(ZxStatusType == nullptr);
  auto constants = fc::abi::utils::Object(ZxStatus_make_constants());
  if (constants == nullptr) {
    return nullptr;
  }
  auto res = reinterpret_cast<PyTypeObject *>(
      PyErr_NewException("fuchsia_controller_internal.ZxStatus", PyExc_Exception, constants.get()));
  if (res == nullptr) {
    return nullptr;
  }
  auto repr = fc::abi::utils::Object(PyDescr_NewMethod(res, &ZxStatus_repr_def));
  if (repr == nullptr) {
    return nullptr;
  }
  if (PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "__repr__", repr.get()) < 0) {
    return nullptr;
  }
  auto raw = fc::abi::utils::Object(PyDescr_NewMethod(res, &ZxStatus_raw_def));
  if (raw == nullptr ||
      PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "raw", raw.get()) < 0) {
    return nullptr;
  }
  auto str = fc::abi::utils::Object(PyDescr_NewMethod(res, &ZxStatus_str_def));
  if (str == nullptr) {
    return nullptr;
  }
  if (PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "__str__", str.get()) < 0) {
    return nullptr;
  }
  Py_IncRef(reinterpret_cast<PyObject *>(res));
  ZxStatusType = res;
  return res;
}

PyTypeObject *FcTransportStatusType_Create() {
  assert(FcTransportStatusType == nullptr);
  auto constants = fc::abi::utils::Object(FcTransportStatus_make_constants());
  if (constants == nullptr) {
    return nullptr;
  }
  auto res = reinterpret_cast<PyTypeObject *>(PyErr_NewException(
      "fuchsia_controller_internal.FcTransportStatus", PyExc_Exception, constants.get()));
  if (res == nullptr) {
    return nullptr;
  }
  auto repr = fc::abi::utils::Object(PyDescr_NewMethod(res, &FcTransportStatus_repr_def));
  if (repr == nullptr) {
    return nullptr;
  }
  if (PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "__repr__", repr.get()) < 0) {
    return nullptr;
  }
  auto desc = fc::abi::utils::Object(PyDescr_NewMethod(res, &FcTransportStatus_desc_def));
  if (desc == nullptr ||
      PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "desc", desc.get()) < 0) {
    return nullptr;
  }
  auto code = fc::abi::utils::Object(PyDescr_NewMethod(res, &FcTransportStatus_code_def));
  if (code == nullptr ||
      PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "code", code.get()) < 0) {
    return nullptr;
  }
  auto str = fc::abi::utils::Object(PyDescr_NewMethod(res, &FcTransportStatus_str_def));
  if (str == nullptr) {
    return nullptr;
  }
  if (PyObject_SetAttrString(reinterpret_cast<PyObject *>(res), "__str__", str.get()) < 0) {
    return nullptr;
  }
  Py_IncRef(reinterpret_cast<PyObject *>(res));
  FcTransportStatusType = res;
  return res;
}

}  // namespace error
