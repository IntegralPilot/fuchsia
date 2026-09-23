// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_PYTHON_DICT_VISITOR_H_
#define SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_PYTHON_DICT_VISITOR_H_

#include <Python.h>

#include "src/lib/fidl_codec/visitor.h"

namespace fuchsia_controller::fidl_codec::python_dict_visitor {

class PythonDictVisitor : public ::fidl_codec::Visitor {
 public:
  PythonDictVisitor() = default;
  PyObject* result() { return result_; }

 private:
  void VisitValue(const ::fidl_codec::Value* node, const ::fidl_codec::Type* for_type) override;

  void VisitInvalidValue(const ::fidl_codec::InvalidValue* node,
                         const ::fidl_codec::Type* for_type) override;

  void VisitNullValue(const ::fidl_codec::NullValue* node,
                      const ::fidl_codec::Type* for_type) override;

  void VisitBoolValue(const ::fidl_codec::BoolValue* node,
                      const ::fidl_codec::Type* for_type) override;

  void VisitStringValue(const ::fidl_codec::StringValue* node,
                        const ::fidl_codec::Type* for_type) override;

  void VisitUnionValue(const ::fidl_codec::UnionValue* node,
                       const ::fidl_codec::Type* type) override;

  void VisitStructValue(const ::fidl_codec::StructValue* node,
                        const ::fidl_codec::Type* for_type) override;

  void VisitVectorValue(const ::fidl_codec::VectorValue* node,
                        const ::fidl_codec::Type* for_type) override;

  void VisitTableValue(const ::fidl_codec::TableValue* node,
                       const ::fidl_codec::Type* for_type) override;

  void VisitDoubleValue(const ::fidl_codec::DoubleValue* node,
                        const ::fidl_codec::Type* for_type) override;

  void VisitIntegerValue(const ::fidl_codec::IntegerValue* node,
                         const ::fidl_codec::Type* for_type) override;

  void VisitHandleValue(const ::fidl_codec::HandleValue* handle,
                        const ::fidl_codec::Type* for_type) override;

  PyObject* result_{nullptr};
};

}  // namespace fuchsia_controller::fidl_codec::python_dict_visitor
#endif  // SRC_DEVELOPER_FFX_LIB_FUCHSIA_CONTROLLER_CPP_FIDL_CODEC_PYTHON_DICT_VISITOR_H_
