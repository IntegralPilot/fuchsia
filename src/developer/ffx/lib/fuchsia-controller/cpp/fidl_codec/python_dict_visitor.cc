// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "python_dict_visitor.h"

#include <Python.h>

#include <string>

#include <fuchsia_controller_abi/utils.h>

#include "src/lib/fidl_codec/printer.h"
#include "src/lib/fidl_codec/wire_object.h"
#include "src/lib/fidl_codec/wire_types.h"
#include "utils.h"

namespace fuchsia_controller::fidl_codec::python_dict_visitor {

using FuchsiaControllerObject = ::fuchsia_controller::abi::utils::Object;
using ::fuchsia_controller::fidl_codec::utils::NormalizeMemberName;

void PythonDictVisitor::VisitValue(const ::fidl_codec::Value* node,
                                   const ::fidl_codec::Type* for_type) {
  std::stringstream ss;
  ::fidl_codec::PrettyPrinter printer(ss, ::fidl_codec::WithoutColors, false, "", 0, false);
  node->PrettyPrint(for_type, printer);
  result_ = PyUnicode_FromString(ss.str().c_str());
}

void PythonDictVisitor::VisitInvalidValue(const ::fidl_codec::InvalidValue* node,
                                          const ::fidl_codec::Type* for_type) {
  PyErr_Format(PyExc_TypeError, "invalid value for type: %s",
               for_type ? for_type->Name().c_str() : "[unknown]");
}

void PythonDictVisitor::VisitNullValue(const ::fidl_codec::NullValue* node,
                                       const ::fidl_codec::Type* for_type) {
  Py_IncRef(Py_None);
  result_ = Py_None;
}

void PythonDictVisitor::VisitBoolValue(const ::fidl_codec::BoolValue* node,
                                       const ::fidl_codec::Type* for_type) {
  result_ = PyBool_FromLong(node->value());
}

void PythonDictVisitor::VisitStringValue(const ::fidl_codec::StringValue* node,
                                         const ::fidl_codec::Type* for_type) {
  result_ = PyUnicode_FromStringAndSize(node->string().data(),
                                        static_cast<Py_ssize_t>(node->string().length()));
}

void PythonDictVisitor::VisitUnionValue(const ::fidl_codec::UnionValue* node,
                                        const ::fidl_codec::Type* type) {
  auto res = FuchsiaControllerObject(PyDict_New());
  PythonDictVisitor visitor;
  node->value()->Visit(&visitor, node->member().type());
  if (visitor.result() == nullptr) {
    return;
  }
  PyDict_SetItemString(res.get(), NormalizeMemberName(node->member().name()).c_str(),
                       visitor.result());
  result_ = res.take();
}

void PythonDictVisitor::VisitStructValue(const ::fidl_codec::StructValue* node,
                                         const ::fidl_codec::Type* for_type) {
  auto res = FuchsiaControllerObject(PyDict_New());
  for (const auto& member : node->struct_definition().members()) {
    auto it = node->fields().find(member.get());
    if (it == node->fields().end()) {
      continue;
    }
    PythonDictVisitor visitor;
    it->second->Visit(&visitor, member->type());
    if (visitor.result() == nullptr) {
      return;
    }
    PyDict_SetItemString(res.get(), NormalizeMemberName(member->name()).c_str(), visitor.result());
  }
  result_ = res.take();
}

void PythonDictVisitor::VisitVectorValue(const ::fidl_codec::VectorValue* node,
                                         const ::fidl_codec::Type* for_type) {
  if (for_type == nullptr) {
    PyErr_SetString(PyExc_TypeError,
                    "expected vector type in during decoding. Received null value");
    return;
  }
  const auto component_type = for_type->GetComponentType();
  if (component_type == nullptr) {
    PyErr_SetString(PyExc_TypeError, "vector value's type does not contain a component type");
    return;
  }
  auto res = FuchsiaControllerObject(PyList_New(static_cast<Py_ssize_t>(node->values().size())));
  Py_ssize_t values_size = static_cast<Py_ssize_t>(node->values().size());
  const auto& values = node->values();
  for (Py_ssize_t i = 0; i < values_size; ++i) {
    PythonDictVisitor visitor;
    values[i]->Visit(&visitor, component_type);
    if (visitor.result() == nullptr) {
      return;
    }
    PyList_SetItem(res.get(), i, visitor.result());
  }
  result_ = res.take();
}

void PythonDictVisitor::VisitTableValue(const ::fidl_codec::TableValue* node,
                                        const ::fidl_codec::Type* for_type) {
  auto res = FuchsiaControllerObject(PyDict_New());

  for (const auto& member : node->table_definition().members()) {
    if (member != nullptr) {
      auto it = node->members().find(member.get());
      if (it == node->members().end()) {
        Py_IncRef(Py_None);
        PyDict_SetItemString(res.get(), NormalizeMemberName(member->name()).c_str(), Py_None);
        continue;
      }
      if (it->second == nullptr || it->second->IsNull()) {
        Py_IncRef(Py_None);
        PyDict_SetItemString(res.get(), NormalizeMemberName(member->name()).c_str(), Py_None);
        continue;
      }
      PythonDictVisitor visitor;
      it->second->Visit(&visitor, member->type());
      if (visitor.result() == nullptr) {
        return;
      }
      PyDict_SetItemString(res.get(), NormalizeMemberName(member->name()).c_str(),
                           visitor.result());
    }
  }
  result_ = res.take();
}

void PythonDictVisitor::VisitDoubleValue(const ::fidl_codec::DoubleValue* node,
                                         const ::fidl_codec::Type* for_type) {
  double value;
  node->GetDoubleValue(&value);
  result_ = PyFloat_FromDouble(value);
}

void PythonDictVisitor::VisitIntegerValue(const ::fidl_codec::IntegerValue* node,
                                          const ::fidl_codec::Type* for_type) {
  uint64_t value;
  bool negative;
  node->GetIntegerValue(&value, &negative);
  if (negative) {
    // Max possible absolute value for a signed integer (2^63).
    if (value > 1ul << 63) {
      PyErr_SetString(PyExc_OverflowError, "Integer overflow");
      return;
    }
    int64_t res = static_cast<int64_t>(value);
    res *= -1;
    result_ = PyLong_FromLongLong(res);
  } else {
    result_ = PyLong_FromUnsignedLongLong(value);
  }
}

void PythonDictVisitor::VisitHandleValue(const ::fidl_codec::HandleValue* handle,
                                         const ::fidl_codec::Type* for_type) {
  if (handle->handle().handle == 0) {
    Py_INCREF(Py_None);
    result_ = Py_None;
  } else {
    result_ = PyLong_FromUnsignedLongLong(handle->handle().handle);
  }
}

}  // namespace fuchsia_controller::fidl_codec::python_dict_visitor
