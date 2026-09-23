// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mod.h"

#include <fidl/fuchsia.fdomain/cpp/fidl.h>
#include <fidl/fuchsia.fdomain/cpp/natural_ostream.h>
#include <lib/stdcompat/span.h>

#include <sstream>

#include <fuchsia_controller_abi/utils.h>

#include "error.h"

// Defined in fuchsia_controller_py.cc
extern struct PyModuleDef fuchsia_controller_internal;

namespace fc = fuchsia_controller;

namespace {

// The purpose of this wrapper type is to allow for more descriptive error messages for zx_status_t
// types. Currently the formatter cannot differentiate between this type and a regular integer, so
// things like ZX_ERR_PEER_CLOSED would print as "-24" without this wrapper. So far the surface area
// that needs to be handled is relatively small. Only the `TargetError` variant of
// `fuchsia.fdomain.Error` needs to be handled, but in order to ensure all formatting works
// correctly, we also need to wrap other types in this formatter, deconstruct them, and wrap their
// inner errors when encountered.
//
// It does have a decent amount of boilerplate, but that's the price we're currently paying to have
// more clarity around error messages.
template <typename T>
struct DescriptiveFormatter {
  const T &error;
  explicit DescriptiveFormatter(const T &err) : error(err) {}
};
}  // namespace

namespace fidl::ostream {
template <>
struct Formatter<DescriptiveFormatter<fuchsia_fdomain::Error>> {
  static std::ostream &Format(std::ostream &os,
                              const DescriptiveFormatter<fuchsia_fdomain::Error> &f) {
    auto err = f.error;
    switch (err.Which()) {
      case fuchsia_fdomain::Error::Tag::kTargetError:
        os << "fuchsia_fdomain::Error::target_error("
           << error::zx_status_get_string(err.target_error().value()) << ")";
        break;
      case fuchsia_fdomain::Error::Tag::kBadHandleId:
      case fuchsia_fdomain::Error::Tag::kNewHandleIdOutOfRange:
      case fuchsia_fdomain::Error::Tag::kNewHandleIdReused:
      case fuchsia_fdomain::Error::Tag::kWrongHandleType:
      case fuchsia_fdomain::Error::Tag::kStreamingReadInProgress:
      case fuchsia_fdomain::Error::Tag::kNoReadInProgress:
      case fuchsia_fdomain::Error::Tag::kWroteToSelf:
      case fuchsia_fdomain::Error::Tag::kClosedDuringRead:
      case fuchsia_fdomain::Error::Tag::kSignalsUnknown:
      case fuchsia_fdomain::Error::Tag::kRightsUnknown:
      case fuchsia_fdomain::Error::Tag::kSocketDispositionUnknown:
      case fuchsia_fdomain::Error::Tag::kSocketTypeUnknown:
        os << fidl::ostream::Formatted(err);
        break;
      default:
        os << "unhandled variant. This is a bug: " << fidl::ostream::Formatted(err);
        break;
    }
    return os;
  }
};

template <>
struct Formatter<DescriptiveFormatter<fuchsia_fdomain::WriteChannelError>> {
  static std::ostream &Format(std::ostream &os,
                              const DescriptiveFormatter<fuchsia_fdomain::WriteChannelError> &f) {
    auto err = f.error;
    os << "fuchsia_fdomain::WriteChannelError::";
    switch (err.Which()) {
      case fuchsia_fdomain::WriteChannelError::Tag::kError:
        os << "error(" << fidl::ostream::Formatted(DescriptiveFormatter(err.error().value()))
           << ")";
        break;
      case fuchsia_fdomain::WriteChannelError::Tag::kOpErrors:
        os << "op_errors([";
        if (err.op_errors().has_value()) {
          const auto &errors = err.op_errors().value();
          for (auto iter = errors.cbegin(); iter != errors.cend(); iter++) {
            if (iter->has_value()) {
              os << fidl::ostream::Formatted(DescriptiveFormatter(iter->value()));
            } else {
              os << "null";
            }
            if (iter != errors.cend()) {
              os << ", ";
            }
          }
        }
        os << "])";
        break;
    }
    return os;
  }
};

template <>
struct Formatter<DescriptiveFormatter<fuchsia_fdomain::WriteSocketError>> {
  static std::ostream &Format(std::ostream &os,
                              const DescriptiveFormatter<fuchsia_fdomain::WriteSocketError> &f) {
    auto err = f.error;
    os << "fuchsia_fdomain::WriteSocketError { error: "
       << fidl::ostream::Formatted(DescriptiveFormatter(err.error()))
       << ", wrote: " << fidl::ostream::Formatted(err.wrote()) << " }";
    return os;
  }
};
}  // namespace fidl::ostream

namespace {
// Takes a PyTuple and sets the decode error (if we encountered one).
template <typename T, typename V>
PyObject *get_decode_error(const fit::result<T, V> &decode_res) {
  std::ostringstream ss;
  ss << "Unable to decode underlying FIDL error from buffer (this is a bug). Code: "
     << decode_res.error_value();
  auto str = ss.str();
  return PyUnicode_FromStringAndSize(str.data(), static_cast<Py_ssize_t>(str.size()));
}

// Decodes a FIDL error from the scratch memory and turns it into a Python exception.
template <typename T>
PyObject *decode_wire_error_type(mod::FuchsiaControllerState *state, char *err_buf,
                                 uint64_t err_len) {
  fit::result decode_res =
      fidl::Unpersist<T>(cpp20::span(reinterpret_cast<uint8_t *>(err_buf), err_len));
  if (decode_res.is_error()) {
    return get_decode_error(decode_res);
  }
  // For the time being there's not any existing code that handles the various
  // kinds of errors that this could turn into. We're going to turn it into a
  // somewhat readable string, and depending on use-cases we can add
  // easier-to-debug information later. Ideally in the future we can leverage
  // Python bindings and simply use some kind of `unpersist` function for that
  // instead.
  DescriptiveFormatter descriptive_formatter(decode_res.value());
  auto fostream = fidl::ostream::Formatted(descriptive_formatter);
  std::ostringstream ss;
  ss << fostream;
  auto output = ss.str();
  return PyUnicode_FromStringAndSize(output.data(), static_cast<Py_ssize_t>(output.size()));
}

void set_fdomain_exception(mod::FuchsiaControllerState *state, fc_status_t err, char *err_buf,
                           uint64_t err_len) {
  auto tuple = fc::abi::utils::Object(PyTuple_New(2));
  if (tuple == nullptr) {
    std::ostringstream ss;
    ss << "Failed to allocate Tuple in %s" << __func__;
    auto out = ss.str();
    PyErr_SetString(PyExc_RuntimeError, out.c_str());
    return;
  }
  PyTuple_SetItem(tuple.get(), 0, PyLong_FromLong(err));
  PyObject *err_message = nullptr;
  switch (err) {
    case FC_ERR_SOCKET_WRITE:
      err_message =
          decode_wire_error_type<::fuchsia_fdomain::WriteSocketError>(state, err_buf, err_len);
      break;
    case FC_ERR_CHANNEL_WRITE: {
      err_message =
          decode_wire_error_type<::fuchsia_fdomain::WriteChannelError>(state, err_buf, err_len);
      break;
    }
    case FC_ERR_FDOMAIN: {
      err_message = decode_wire_error_type<::fuchsia_fdomain::Error>(state, err_buf, err_len);
      break;
    }
    default:
      std::ostringstream ss;
      // It's a little awkward, but in the event that the caller just sent something wrong we
      // should let the user know they've run into a bug. This would only happen if
      // "set_python_exception" was written incorrectly.
      ss << "Received unrecognized fc_status_t error (" << err << ") in " << __func__
         << ". This is a bug";
      auto out = ss.str();
      PyErr_SetString(PyExc_RuntimeError, out.c_str());
      break;
  }
  if (err_message != nullptr) {
    PyTuple_SetItem(tuple.get(), 1, err_message);
    PyErr_SetObject(reinterpret_cast<PyObject *>(error::FcTransportStatusType), tuple.get());
  }
}
}  // namespace

namespace mod {
FuchsiaControllerState *get_module_state() {
  auto mod = PyState_FindModule(&fuchsia_controller_internal);
  if (mod == nullptr) {
    return nullptr;
  }
  return reinterpret_cast<FuchsiaControllerState *>(PyModule_GetState(mod));
}

// Returns new reference.
namespace {

// Convenience RAII error state handler.
class LastError {
 public:
  explicit LastError(mod::FuchsiaControllerState *state) : state_(state) {
    ffx_get_last_error(state_->ctx, &err_buf_, &err_len_, &err_type_);
  }

  ~LastError() {
    if (err_buf_ != nullptr) {
      ffx_free_error_buffer(err_buf_, err_len_);
    }
  }

  char *err_buf() { return err_buf_; }
  uint64_t err_len() const { return err_len_; }
  fc_err_type_t err_type() const { return err_type_; }

  bool is_fidl() { return err_buf_ != nullptr && err_type_ == FC_ERR_TYPE_FIDL; }
  bool is_string() { return err_buf_ != nullptr && err_type_ == FC_ERR_TYPE_STRING; }

  fc::abi::utils::Object to_string() {
    if (!is_string()) {
      PyErr_SetString(PyExc_RuntimeError, "Internal error: expected string error from buffer");
      return fc::abi::utils::Object::null();
    }
    auto str_obj = PyUnicode_FromStringAndSize(err_buf_, static_cast<Py_ssize_t>(err_len_));
    // If this is null this will return an internal error about allocating the string.
    if (str_obj == nullptr) {
      PyErr_Clear();
      PyErr_SetString(PyExc_RuntimeError,
                      "Internal error: unable to allocate error string from buffer. This is a bug");
      return fc::abi::utils::Object::null();
    }
    return fc::abi::utils::Object(str_obj);
  }

  void set_fdomain_exception(fc_status_t err) {
    if (!is_fidl()) {
      PyErr_SetString(PyExc_RuntimeError, "Internal error: expected FIDL error from buffer");
      return;
    }
    ::set_fdomain_exception(state_, err, err_buf_, err_len_);
  }

 private:
  mod::FuchsiaControllerState *state_;
  char *err_buf_ = nullptr;
  uint64_t err_len_ = 0;
  fc_err_type_t err_type_ = FC_ERR_TYPE_NONE;
};

}  // namespace

void set_python_exception(fc_status_t err) {
  LastError last_err(get_module_state());
  switch (err) {
    // If for some reason this is passed, some bug has been introduced, as there
    // is no reason to return `FC_OK`.
    case FC_OK:
      PyErr_SetString(PyExc_RuntimeError,
                      "FC_OK was passed to raise an exception. This is likely a bug");
      break;
    // None of these cases contain any internal structs, so setting the error
    // code is sufficient.
    case FC_ERR_INVALID_ARGS:
    case FC_ERR_NOT_SUPPORTED:
    case FC_ERR_NOT_FOUND:
    case FC_ERR_BUFFER_TOO_SMALL:
    case FC_ERR_SHOULD_WAIT:
    case FC_ERR_PROTOCOL_OBJECT_TYPE_INCOMPATIBLE:
    case FC_ERR_PROTOCOL_RIGHTS_INCOMPATIBLE:
    case FC_ERR_PROTOCOL_STREAM_EVENT_INCOMPATIBLE:
    case FC_ERR_CONNECTION_MISMATCH:
    case FC_ERR_STREAMING_ABORTED: {
      auto exception_err = fc::abi::utils::Object(PyLong_FromLong(err));
      PyErr_SetObject(reinterpret_cast<PyObject *>(error::FcTransportStatusType), exception_err.get());
      break;
    }
    // These errors are only surfaced from internal failures, which means the
    // Rust error that created this exception is surfaced.
    case FC_ERR_PROTOCOL:
    case FC_ERR_TRANSPORT:
    case FC_ERR_INTERNAL: {
      auto str_obj = last_err.to_string();
      if (str_obj == nullptr) {
        return;
      }
      switch (err) {
        case FC_ERR_TRANSPORT:
        case FC_ERR_PROTOCOL: {
          auto tuple = fc::abi::utils::Object(PyTuple_New(2));
          if (tuple == nullptr) {
            std::ostringstream ss;
            ss << "Failed to allocate Tuple in %s" << __func__;
            auto out = ss.str();
            PyErr_SetString(PyExc_RuntimeError, out.c_str());
            break;
          }
          PyTuple_SetItem(tuple.get(), 0, PyLong_FromLong(err));
          // Important: use `take` here because this steals the reference.
          // PyErr_SetObject increases the refcount so only requires `get`.
          PyTuple_SetItem(tuple.get(), 1, str_obj.take());
          PyErr_SetObject(reinterpret_cast<PyObject *>(error::FcTransportStatusType), tuple.get());
          break;
        }
        default:
          PyErr_SetObject(PyExc_RuntimeError, str_obj.get());
          break;
      }
      break;
    }
    case FC_ERR_SOCKET_WRITE:
    case FC_ERR_CHANNEL_WRITE:
    case FC_ERR_FDOMAIN: {
      last_err.set_fdomain_exception(err);
      break;
    }
    case FC_ERR_INTERRUPTED: {
      PyErr_SetNone(PyExc_KeyboardInterrupt);
      break;
    }
    default: {
      std::ostringstream ss;
      ss << "Received unrecognized fc_status_t (" << err << ")";
      auto out = ss.str();
      PyErr_SetString(PyExc_RuntimeError, out.c_str());
      break;
    }
  }
}

}  // namespace mod
