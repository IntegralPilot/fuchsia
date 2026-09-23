// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_DECODE_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_DECODE_H_

#include <Python.h>

namespace fuchsia_controller::fidl_codec::decode {

// Attempts to decode a FIDL response message. The expected arguments are "bytes" which should be a
// bytearray object representing the message to be decoded, and "handles" which should be a list of
// integers that are able to be converted into an unsigned 32 bit integer.
PyObject *decode_fidl_response(PyObject *self, PyObject *args, PyObject *kwds);

// Attempts to decode a FIDL request message. The expected arguments are "bytes" which should be a
// bytearray object representing the message to be decoded, and "handles" which should be a list of
// integers that are able to be converted into an unsigned 32 bit integer.
PyObject *decode_fidl_request(PyObject *self, PyObject *args, PyObject *kwds);

// Attempts to decode a FIDL object based on its type.
//
// The type is supplied as a fully qualified FIDL string name, and the message to be decoded is
// passed in along with it (along with associated handles).
PyObject *decode_standalone(PyObject *self, PyObject *args, PyObject *kwds);

// Represents the signature of the FIDL response decoder method. Used to register the function in a
// Python module.
extern PyMethodDef decode_fidl_response_py_def;

// Represents the signature of the FIDL request decoder method. Used to register the function in a
// Python module.
extern PyMethodDef decode_fidl_request_py_def;

// Represents the signature of the FIDL generalized decoder. Used to decode by type name string.
extern PyMethodDef decode_standalone_py_def;

}  // namespace fuchsia_controller::fidl_codec::decode

#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_DECODE_H_
