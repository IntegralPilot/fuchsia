// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_diagnostics_validate::{Action, LazyAction, LinkDisposition, ROOT_ID, Value, ValueType};

pub enum Step {
    Actions(Vec<Action>),
    LazyActions(Vec<LazyAction>),
    ActLazyThreadLocalActions(Vec<LazyAction>),
    WithMetrics(Vec<Action>, String),
}

pub struct Trial {
    pub name: String,
    pub steps: Vec<Step>,
}

pub fn real_trials() -> Vec<Trial> {
    vec![
        basic_node(),
        basic_int(),
        basic_uint(),
        basic_double(),
        basic_string(),
        basic_bytes(),
        basic_bool(),
        basic_int_array(),
        basic_string_array(),
        basic_uint_array(),
        basic_double_array(),
        int_histogram_ops_trial(),
        uint_histogram_ops_trial(),
        double_histogram_ops_trial(),
        deletions_trial(),
        lazy_nodes_trial(),
        repeated_names(),
        act_lazy_thread_local_trial(),
    ]
}

fn act_lazy_thread_local_trial() -> Trial {
    Trial {
        name: "ActLazyThreadLocal".into(),
        steps: vec![Step::ActLazyThreadLocalActions(vec![
            // Create sibling node with same name and same content
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes",value: vec!(3u8, 4u8))]
            ),
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 2,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes",value: vec!(3u8, 4u8))]
            ),
            crate::delete_lazy_node!(id: 1),
            crate::delete_lazy_node!(id: 2),
            // Recreate child node with new values
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes_new",value: vec!(1u8, 2u8))]
            ),
            crate::delete_lazy_node!(id: 1),
            // Create child node with inline disposition
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "inline_child",
                disposition: LinkDisposition::Inline,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "inline_child",value: vec!(1u8, 2u8))]
            ),
            crate::delete_lazy_node!(id: 1),
        ])],
    }
}

fn basic_node() -> Trial {
    Trial {
        name: "Basic Node".into(),
        steps: vec![Step::Actions(vec![
            crate::create_node!(parent: ROOT_ID, id: 1, name: "child"),
            crate::create_node!(parent: 1, id: 2, name: "grandchild"),
            crate::delete_node!( id: 2),
            crate::delete_node!( id: 1 ),
            // Verify they can be deleted in either order.
            crate::create_node!(parent: ROOT_ID, id: 1, name: "child"),
            crate::create_node!(parent: 1, id: 2, name: "grandchild"),
            crate::delete_node!( id: 1),
            crate::delete_node!( id: 2 ),
        ])],
    }
}

fn basic_string() -> Trial {
    Trial {
        name: "Basic String".into(),
        steps: vec![Step::Actions(vec![
            crate::create_string_property!(parent: ROOT_ID, id:1, name: "str", value: "foo"),
            crate::set_string!(id: 1, value: "bar"),
            crate::set_string!(id: 1, value: "This Is A Longer String"),
            crate::set_string!(id: 1, value: "."),
            // Make sure it can hold a string bigger than the biggest block (3000 chars > 2040)
            crate::set_string!(id: 1, value: ["1234567890"; 300].to_vec().join("")),
            crate::delete_property!(id: 1),
        ])],
    }
}

fn basic_bytes() -> Trial {
    Trial {
        name: "Basic bytes".into(),
        steps: vec![Step::Actions(vec![
            crate::create_bytes_property!(parent: ROOT_ID, id: 8, name: "bytes", value: vec![1u8, 2u8]),
            crate::set_bytes!(id: 8, value: vec![3u8, 4, 5, 6, 7]),
            crate::set_bytes!(id: 8, value: vec![8u8]),
            crate::delete_property!(id: 8),
        ])],
    }
}

fn basic_bool() -> Trial {
    Trial {
        name: "Basic Bool".into(),
        steps: vec![Step::Actions(vec![
            crate::create_bool_property!(parent: ROOT_ID, id: 1, name: "bool", value: true),
            crate::set_bool!(id: 1, value: false),
            crate::set_bool!(id: 1, value: true),
            crate::delete_property!(id: 1),
        ])],
    }
}

fn basic_int() -> Trial {
    Trial {
        name: "Basic Int".into(),
        steps: vec![Step::Actions(vec![
            crate::create_numeric_property!(parent: ROOT_ID, id: 5, name: "int", value: Value::IntT(10)),
            crate::set_number!(id: 5, value: Value::IntT(i64::MAX)),
            crate::subtract_number!(id: 5, value: Value::IntT(3)),
            crate::set_number!(id: 5, value: Value::IntT(i64::MIN)),
            crate::add_number!(id: 5, value: Value::IntT(2)),
            crate::delete_property!(id: 5),
        ])],
    }
}

fn basic_uint() -> Trial {
    Trial {
        name: "Basic Uint".into(),
        steps: vec![Step::Actions(vec![
            crate::create_numeric_property!(parent: ROOT_ID, id: 5, name: "uint", value: Value::UintT(1)),
            crate::set_number!(id: 5, value: Value::UintT(u64::MAX)),
            crate::subtract_number!(id: 5, value: Value::UintT(3)),
            crate::set_number!(id: 5, value: Value::UintT(0)),
            crate::add_number!(id: 5, value: Value::UintT(2)),
            crate::delete_property!(id: 5),
        ])],
    }
}

fn basic_double() -> Trial {
    Trial {
        name: "Basic Double".into(),
        steps: vec![Step::Actions(vec![
            crate::create_numeric_property!(parent: ROOT_ID, id: 5, name: "double",
                                     value: Value::DoubleT(1.0)),
            crate::set_number!(id: 5, value: Value::DoubleT(f64::MAX)),
            crate::subtract_number!(id: 5, value: Value::DoubleT(f64::MAX/10_f64)),
            crate::set_number!(id: 5, value: Value::DoubleT(f64::MIN)),
            crate::add_number!(id: 5, value: Value::DoubleT(f64::MAX / 10_f64)),
            crate::delete_property!(id: 5),
        ])],
    }
}

fn repeated_names() -> Trial {
    let mut actions = vec![crate::create_node!(parent: ROOT_ID, id: 1, name: "measurements")];

    for i in 100..120 {
        actions.push(crate::create_node!(parent: 1, id: i, name: format!("{}", i)));
        actions.push(crate::create_numeric_property!(parent: i, id: i + 1000, name: "count", value: Value::UintT(i as u64 * 2)));
        actions.push(crate::create_numeric_property!(parent: i, id: i + 2000, name: "time_spent", value: Value::UintT(i as u64 * 1000 + 10)));
    }

    Trial {
        name: "Many repeated names".into(),
        steps: vec![Step::WithMetrics(actions, "Many repeated names".into())],
    }
}

fn array_indexes_to_test() -> Vec<u64> {
    let mut ret: Vec<u64> = (0..10).collect();
    ret.push(1000);
    ret.push(10000);
    ret.push(u64::MAX);
    ret
}

fn basic_int_array() -> Trial {
    let mut actions =
        vec![crate::create_array_property!(parent: ROOT_ID, id: 5, name: "int", slots: 5,
                                       type: ValueType::Int)];
    for index in array_indexes_to_test().iter() {
        actions.push(crate::array_add!(id: 5, index: *index, value: Value::IntT(7)));
        actions.push(crate::array_subtract!(id: 5, index: *index, value: Value::IntT(3)));
        actions.push(crate::array_set!(id: 5, index: *index, value: Value::IntT(19)));
    }
    actions.push(crate::delete_property!(id: 5));
    Trial { name: "Int Array Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn basic_string_array() -> Trial {
    const ID: u32 = 5;
    let mut actions = vec![
        crate::create_array_property!(parent: ROOT_ID, id: ID, name: "string", slots: 5, type: ValueType::String),
    ];

    for index in array_indexes_to_test().iter() {
        if (*index).is_multiple_of(2) {
            actions
            .push(crate::array_set!(id: ID, index: *index, value: Value::StringT(format!("string data {}", *index))));
        } else if (*index).is_multiple_of(3) {
            actions.push(
                crate::array_set!(id: ID, index: *index, value: Value::StringT(String::new())),
            );
        } else {
            actions.push(
                crate::array_set!(id: ID, index: *index, value: Value::StringT("string data".into())),
            );
        }
    }

    for index in array_indexes_to_test().iter() {
        if (*index).is_multiple_of(2) {
            actions
                .push(crate::array_set!(id: ID, index: *index, value: Value::StringT("".into())));
        }
    }

    for index in array_indexes_to_test().iter() {
        if (*index).is_multiple_of(4) {
            actions.push(
                crate::array_set!(id: ID, index: *index, value: Value::StringT(format!("{}", *index))),
            );
        }
    }

    actions.push(crate::delete_property!(id: ID));
    Trial { name: "String Array Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn basic_uint_array() -> Trial {
    let mut actions =
        vec![crate::create_array_property!(parent: ROOT_ID, id: 6, name: "uint", slots: 5,
                                       type: ValueType::Uint)];
    for index in array_indexes_to_test().iter() {
        actions.push(crate::array_add!(id: 6, index: *index, value: Value::UintT(11)));
        actions.push(crate::array_subtract!(id: 6, index: *index, value: Value::UintT(3)));
        actions.push(crate::array_set!(id: 6, index: *index, value: Value::UintT(19)));
    }
    actions.push(crate::delete_property!(id: 6));
    Trial { name: "Unt Array Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn basic_double_array() -> Trial {
    let mut actions =
        vec![crate::create_array_property!(parent: ROOT_ID, id: 4, name: "float", slots: 5,
                                       type: ValueType::Double)];
    for index in array_indexes_to_test().iter() {
        actions.push(crate::array_add!(id: 4, index: *index, value: Value::DoubleT(2.0)));
        actions.push(crate::array_subtract!(id: 4, index: *index, value: Value::DoubleT(3.5)));
        actions.push(crate::array_set!(id: 4, index: *index, value: Value::DoubleT(19.0)));
    }
    actions.push(crate::delete_property!(id: 4));
    Trial { name: "Int Array Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn int_histogram_ops_trial() -> Trial {
    fn push_ops(actions: &mut Vec<Action>, value: i64) {
        actions.push(crate::insert!(id: 4, value: Value::IntT(value)));
        actions.push(crate::insert_multiple!(id: 4, value: Value::IntT(value), count: 3));
        actions.push(crate::insert!(id: 5, value: Value::IntT(value)));
        actions.push(crate::insert_multiple!(id: 5, value: Value::IntT(value), count: 3));
    }
    let mut actions = vec![
        crate::create_linear_histogram!(parent: ROOT_ID, id: 4, name: "Lhist", floor: -5,
                                 step_size: 3, buckets: 3, type: IntT),
        crate::create_exponential_histogram!(parent: ROOT_ID, id: 5, name: "Ehist", floor: -5,
                                 initial_step: 2, step_multiplier: 4,
                                 buckets: 3, type: IntT),
    ];
    for value in [i64::MIN, i64::MAX, 0] {
        push_ops(&mut actions, value);
    }
    for value in [-10_i64, -5_i64, 0_i64, 3_i64, 100_i64] {
        push_ops(&mut actions, value);
    }
    actions.push(crate::delete_property!(id: 4));
    actions.push(crate::delete_property!(id: 5));
    Trial { name: "Int Histogram Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn uint_histogram_ops_trial() -> Trial {
    fn push_ops(actions: &mut Vec<Action>, value: u64) {
        actions.push(crate::insert!(id: 4, value: Value::UintT(value)));
        actions.push(crate::insert_multiple!(id: 4, value: Value::UintT(value), count: 3));
        actions.push(crate::insert!(id: 5, value: Value::UintT(value)));
        actions.push(crate::insert_multiple!(id: 5, value: Value::UintT(value), count: 3));
    }
    let mut actions = vec![
        crate::create_linear_histogram!(parent: ROOT_ID, id: 4, name: "Lhist", floor: 5,
                                 step_size: 3, buckets: 3, type: UintT),
        crate::create_exponential_histogram!(parent: ROOT_ID, id: 5, name: "Ehist", floor: 5,
                                 initial_step: 2, step_multiplier: 4,
                                 buckets: 3, type: UintT),
    ];
    for value in [u64::MAX, 0] {
        push_ops(&mut actions, value);
    }
    for value in [0_u64, 5_u64, 8_u64, 20u64, 200_u64] {
        push_ops(&mut actions, value);
    }
    actions.push(crate::delete_property!(id: 4));
    actions.push(crate::delete_property!(id: 5));
    Trial { name: "Uint Histogram Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn double_histogram_ops_trial() -> Trial {
    fn push_ops(actions: &mut Vec<Action>, value: f64) {
        actions.push(crate::insert!(id: 4, value: Value::DoubleT(value)));
        actions.push(crate::insert_multiple!(id: 4, value: Value::DoubleT(value), count: 3));
        actions.push(crate::insert!(id: 5, value: Value::DoubleT(value)));
        actions.push(crate::insert_multiple!(id: 5, value: Value::DoubleT(value), count: 3));
    }
    let mut actions = vec![
        // Create exponential first in this test, so that if histograms aren't supported, both
        // linear and exponential will be reported as unsupported.
        crate::create_exponential_histogram!(parent: ROOT_ID, id: 5, name: "Ehist",
                                floor: std::f64::consts::PI, initial_step: 2.0,
                                step_multiplier: 4.0, buckets: 3, type: DoubleT),
        crate::create_linear_histogram!(parent: ROOT_ID, id: 4, name: "Lhist", floor: 5.0,
                                 step_size: 3.0, buckets: 3, type: DoubleT),
    ];
    for value in &[f64::MIN, f64::MAX, f64::MIN_POSITIVE, 0.0] {
        push_ops(&mut actions, *value);
    }
    for value in [3.0, 3.15, 5.0, 10.0] {
        push_ops(&mut actions, value);
    }
    actions.push(crate::delete_property!(id: 4));
    actions.push(crate::delete_property!(id: 5));
    Trial { name: "Double Histogram Ops".into(), steps: vec![Step::Actions(actions)] }
}

fn deletions_trial() -> Trial {
    // Action, being a FIDL struct, doesn't implement Clone, so we have to build a new
    // Action each time we want to invoke it.
    fn n1() -> Action {
        crate::create_node!(parent: ROOT_ID, id: 1, name: "root_child")
    }
    fn n2() -> Action {
        crate::create_node!(parent: 1, id: 2, name: "parent")
    }
    fn n3() -> Action {
        crate::create_node!(parent: 2, id: 3, name: "child")
    }
    fn p1() -> Action {
        crate::create_numeric_property!(parent: 1, id: 4, name: "root_int", value: Value::IntT(1))
    }
    fn p2() -> Action {
        crate::create_numeric_property!(parent: 2, id: 5, name: "parent_int", value: Value::IntT(2))
    }
    fn p3() -> Action {
        crate::create_numeric_property!(parent: 3, id: 6, name: "child_int", value: Value::IntT(3))
    }
    fn create() -> Vec<Action> {
        vec![n1(), n2(), n3(), p1(), p2(), p3()]
    }
    fn create2() -> Vec<Action> {
        vec![n1(), p1(), n2(), p2(), n3(), p3()]
    }
    fn d1() -> Action {
        crate::delete_node!(id: 1)
    }
    fn d2() -> Action {
        crate::delete_node!(id: 2)
    }
    fn d3() -> Action {
        crate::delete_node!(id: 3)
    }
    fn x1() -> Action {
        crate::delete_property!(id: 4)
    }
    fn x2() -> Action {
        crate::delete_property!(id: 5)
    }
    fn x3() -> Action {
        crate::delete_property!(id: 6)
    }
    let steps = vec![
        Step::Actions(create()),
        Step::Actions(vec![d3(), d2(), d1(), x3(), x2(), x1()]),
        Step::Actions(create2()),
        Step::WithMetrics(vec![d1(), d2()], "Delete Except Grandchild".into()),
        Step::WithMetrics(vec![d3(), x3(), x2(), x1()], "Deleted Grandchild".into()),
        // This list tests all 6 sequences of node deletion.
        // TODO(https://fxbug.dev/42116860): Get the permutohedron crate and test all 720 sequences.
        Step::Actions(create()),
        Step::Actions(vec![d1(), d2(), d3(), x3(), x2(), x1()]),
        Step::Actions(create2()),
        Step::Actions(vec![d1(), x3(), d2(), x1(), d3(), x2()]),
        Step::Actions(create()),
        Step::Actions(vec![d1(), x2(), d3(), x3(), d2(), x1()]),
        Step::Actions(create2()),
        Step::Actions(vec![x1(), x3(), d2(), d1(), d3(), x2()]),
        Step::Actions(create()),
        Step::Actions(vec![d2(), x3(), x2(), x1(), d3(), d1()]),
        Step::Actions(create2()),
        Step::Actions(vec![d3(), x3(), d2(), x1(), d1(), x2()]),
        Step::Actions(create2()),
        Step::Actions(vec![x3(), d3(), d1(), x1(), d2(), x2()]),
        Step::WithMetrics(vec![], "Everything should be gone".into()),
    ];
    Trial { name: "Delete With Metrics".into(), steps }
}

fn lazy_nodes_trial() -> Trial {
    Trial {
        name: "Lazy Nodes".into(),
        steps: vec![Step::LazyActions(vec![
            // Create sibling node with same name and same content
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes",value: vec!(3u8, 4u8))]
            ),
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 2,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes",value: vec!(3u8, 4u8))]
            ),
            crate::delete_lazy_node!(id: 1),
            crate::delete_lazy_node!(id: 2),
            // Recreate child node with new values
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "child",
                disposition: LinkDisposition::Child,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "child_bytes_new",value: vec!(1u8, 2u8))]
            ),
            crate::delete_lazy_node!(id: 1),
            // Create child node with inline disposition
            crate::create_lazy_node!(
                parent: ROOT_ID,
                id: 1,
                name: "inline_child",
                disposition: LinkDisposition::Inline,
                actions: vec![crate::create_bytes_property!(parent: ROOT_ID, id: 1, name: "inline_child",value: vec!(1u8, 2u8))]
            ),
            crate::delete_lazy_node!(id: 1),
        ])],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn trial_with_action(name: &str, action: Action) -> Trial {
        Trial { name: name.into(), steps: vec![Step::Actions(vec![action])] }
    }
}
