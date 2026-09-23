// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_IR_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_IR_H_

#include <Python.h>

namespace fuchsia_controller::fidl_codec::ir {

PyObject *add_ir_path(PyObject *self, PyObject *path_obj);
extern PyMethodDef add_ir_path_py_def;
PyObject *get_method_ordinal(PyObject *self, PyObject *args, PyObject *kwds);
extern PyMethodDef get_method_ordinal_py_def;
PyObject *get_ir_path(PyObject *self, PyObject *library_name);
extern PyMethodDef get_ir_path_py_def;
PyObject *add_ir_paths(PyObject *self, PyObject *path_list);
extern PyMethodDef add_ir_paths_py_def;

}  // namespace fuchsia_controller::fidl_codec::ir

#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_IR_H_
