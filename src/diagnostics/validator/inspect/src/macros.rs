// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_export]
macro_rules! create_node {
    (parent: $parent:expr, id: $id:expr, name: $name:expr) => {
        fidl_diagnostics_validate::Action::CreateNode(fidl_diagnostics_validate::CreateNode {
            parent: $parent,
            id: $id,
            name: $name.into(),
        })
    };
}

#[macro_export]
macro_rules! delete_node {
    (id: $id:expr) => {
        fidl_diagnostics_validate::Action::DeleteNode(fidl_diagnostics_validate::DeleteNode {
            id: $id,
        })
    };
}

#[macro_export]
macro_rules! create_numeric_property {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::CreateNumericProperty(
            fidl_diagnostics_validate::CreateNumericProperty {
                parent: $parent,
                id: $id,
                name: $name.into(),
                value: $value,
            },
        )
    };
}

#[macro_export]
macro_rules! create_bytes_property {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::CreateBytesProperty(
            fidl_diagnostics_validate::CreateBytesProperty {
                parent: $parent,
                id: $id,
                name: $name.into(),
                value: $value.into(),
            },
        )
    };
}

#[macro_export]
macro_rules! create_string_property {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::CreateStringProperty(
            fidl_diagnostics_validate::CreateStringProperty {
                parent: $parent,
                id: $id,
                name: $name.into(),
                value: $value.into(),
            },
        )
    };
}

#[macro_export]
macro_rules! create_bool_property {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::CreateBoolProperty(
            fidl_diagnostics_validate::CreateBoolProperty {
                parent: $parent,
                id: $id,
                name: $name.into(),
                value: $value.into(),
            },
        )
    };
}

#[macro_export]
macro_rules! set_string {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::SetString(fidl_diagnostics_validate::SetString {
            id: $id,
            value: $value.into(),
        })
    };
}

#[macro_export]
macro_rules! set_bytes {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::SetBytes(fidl_diagnostics_validate::SetBytes {
            id: $id,
            value: $value.into(),
        })
    };
}

#[macro_export]
macro_rules! set_number {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::SetNumber(fidl_diagnostics_validate::SetNumber {
            id: $id,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! set_bool {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::SetBool(fidl_diagnostics_validate::SetBool {
            id: $id,
            value: $value.into(),
        })
    };
}

#[macro_export]
macro_rules! add_number {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::AddNumber(fidl_diagnostics_validate::AddNumber {
            id: $id,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! subtract_number {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::SubtractNumber(
            fidl_diagnostics_validate::SubtractNumber { id: $id, value: $value },
        )
    };
}

#[macro_export]
macro_rules! delete_property {
    (id: $id:expr) => {
        fidl_diagnostics_validate::Action::DeleteProperty(
            fidl_diagnostics_validate::DeleteProperty { id: $id },
        )
    };
}

#[macro_export]
macro_rules! apply_no_op {
    () => {
        fidl_diagnostics_validate::Action::ApplyNoOp(fidl_diagnostics_validate::ApplyNoOp {})
    };
}

#[macro_export]
macro_rules! create_array_property {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, slots: $slots:expr, type: $type:expr) => {
        fidl_diagnostics_validate::Action::CreateArrayProperty(
            fidl_diagnostics_validate::CreateArrayProperty {
                parent: $parent,
                id: $id,
                name: $name.into(),
                slots: $slots,
                value_type: $type,
            },
        )
    };
}

#[macro_export]
macro_rules! array_set {
    (id: $id:expr, index: $index:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::ArraySet(fidl_diagnostics_validate::ArraySet {
            id: $id,
            index: $index,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! array_add {
    (id: $id:expr, index: $index:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::ArrayAdd(fidl_diagnostics_validate::ArrayAdd {
            id: $id,
            index: $index,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! array_subtract {
    (id: $id:expr, index: $index:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::ArraySubtract(fidl_diagnostics_validate::ArraySubtract {
            id: $id,
            index: $index,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! create_linear_histogram {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, floor: $floor:expr,
        step_size: $step_size:expr, buckets: $buckets:expr, type: $type:ident) => {
        fidl_diagnostics_validate::Action::CreateLinearHistogram(
            fidl_diagnostics_validate::CreateLinearHistogram {
                parent: $parent,
                id: $id,
                name: $name.into(),
                floor: Value::$type($floor),
                step_size: Value::$type($step_size),
                buckets: $buckets,
            },
        )
    };
}

#[macro_export]
macro_rules! create_exponential_histogram {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, floor: $floor:expr,
        initial_step: $initial_step:expr, step_multiplier: $step_multiplier:expr,
        buckets: $buckets:expr, type: $type:ident) => {
        fidl_diagnostics_validate::Action::CreateExponentialHistogram(
            fidl_diagnostics_validate::CreateExponentialHistogram {
                parent: $parent,
                id: $id,
                name: $name.into(),
                floor: Value::$type($floor),
                initial_step: Value::$type($initial_step),
                step_multiplier: Value::$type($step_multiplier),
                buckets: $buckets,
            },
        )
    };
}

#[macro_export]
macro_rules! insert {
    (id: $id:expr, value: $value:expr) => {
        fidl_diagnostics_validate::Action::Insert(fidl_diagnostics_validate::Insert {
            id: $id,
            value: $value,
        })
    };
}

#[macro_export]
macro_rules! insert_multiple {
    (id: $id:expr, value: $value:expr, count: $count:expr) => {
        fidl_diagnostics_validate::Action::InsertMultiple(
            fidl_diagnostics_validate::InsertMultiple { id: $id, value: $value, count: $count },
        )
    };
}

#[macro_export]
macro_rules! create_lazy_node {
    (parent: $parent:expr, id: $id:expr, name: $name:expr, disposition: $disposition:expr, actions: $actions:expr) => {
        fidl_diagnostics_validate::LazyAction::CreateLazyNode(
            fidl_diagnostics_validate::CreateLazyNode {
                parent: $parent,
                id: $id,
                name: $name.into(),
                disposition: $disposition,
                actions: $actions,
            },
        )
    };
}

#[macro_export]
macro_rules! delete_lazy_node {
    (id: $id:expr) => {
        fidl_diagnostics_validate::LazyAction::DeleteLazyNode(
            fidl_diagnostics_validate::DeleteLazyNode { id: $id },
        )
    };
}
