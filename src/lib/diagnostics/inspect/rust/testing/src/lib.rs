// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::table::*;
use clap::Parser;
use fuchsia_inspect::{
    self as inspect, ArithmeticArrayProperty, ArrayProperty, DoubleArrayProperty,
    DoubleExponentialHistogramProperty, DoubleLinearHistogramProperty, Inspector, IntArrayProperty,
    IntExponentialHistogramProperty, IntLinearHistogramProperty, Node, UintArrayProperty,
    UintExponentialHistogramProperty, UintLinearHistogramProperty,
};
use futures::FutureExt;
use std::ops::AddAssign;
use {
    fidl_fuchsia_inspect as finspect, fidl_fuchsia_inspect_deprecated as fdeprecated,
    fuchsia_async as fasync,
};

pub mod deprecated_fidl_server;
pub mod table;

pub struct PopulateParams<T> {
    pub floor: T,
    pub step: T,
    pub count: usize,
}

pub fn populated<H: inspect::HistogramProperty>(histogram: H, params: PopulateParams<H::Type>) -> H
where
    H::Type: AddAssign + Copy,
{
    let mut value = params.floor;
    for _ in 0..params.count {
        histogram.insert(value);
        value += params.step;
    }
    histogram
}

#[derive(Debug, Parser, Default)]
#[command(
    name = "example",
    about = "Example component to showcase Inspect API objects, including an NxM nested table"
)]
pub struct Options {
    #[arg(long)]
    pub rows: usize,

    #[arg(long)]
    pub columns: usize,

    /// If set, publish a top-level number called "extra_number".
    #[arg(long = "extra-number")]
    pub extra_number: Option<i64>,
}

#[derive(Default)]
pub struct ExampleInspectData {
    int_array: IntArrayProperty,
    uint_array: UintArrayProperty,
    double_array: DoubleArrayProperty,
    int_linear_hist: IntLinearHistogramProperty,
    uint_linear_hist: UintLinearHistogramProperty,
    double_linear_hist: DoubleLinearHistogramProperty,
    int_exp_hist: IntExponentialHistogramProperty,
    uint_exp_hist: UintExponentialHistogramProperty,
    double_exp_hist: DoubleExponentialHistogramProperty,
    table: Option<Table>,
}

impl ExampleInspectData {
    /// Allocates a collection of example inspect values for testing.
    ///
    /// The allocated data remains visible to other components until this object
    /// is dropped.
    pub fn write_to(&mut self, node: &Node) {
        // Changing the order of the function calls below also changes the generated
        // identifiers that are used for each piece of example inspect data.
        reset_unique_names();
        self.table.replace(new_example_table(node));
        self.int_array = new_example_int_array(node);
        self.uint_array = new_example_uint_array(node);
        self.double_array = new_example_double_array(node);
        self.int_linear_hist = new_example_int_linear_hist(node);
        self.uint_linear_hist = new_example_uint_linear_hist(node);
        self.double_linear_hist = new_example_double_linear_hist(node);
        self.int_exp_hist = new_example_int_exp_hist(node);
        self.uint_exp_hist = new_example_uint_exp_hist(node);
        self.double_exp_hist = new_example_double_exp_hist(node);

        // Lazy values are closures registered with the node, and don't need
        // to be held by this struct to avoid being dropped.
        new_example_lazy_double(node);
        new_example_lazy_uint(node);
    }

    /// Returns an example NodeObject that can used to test fuchsia.inspect.deprecated.inspect.
    pub fn has_node_object(&self) -> bool {
        self.table.is_some()
    }

    pub fn get_node_object(&self) -> NodeObject {
        self.table.as_ref().unwrap().get_node_object()
    }
}

pub async fn serve_deprecated_inspect(stream: fdeprecated::InspectRequestStream, node: NodeObject) {
    deprecated_fidl_server::spawn_inspect_server(stream, node);
}

pub async fn serve_inspect_tree(stream: finspect::TreeRequestStream, inspector: &Inspector) {
    let scope = fasync::Scope::new_with_name("serve_inspect_tree");
    let settings = inspect_runtime::TreeServerSendPreference::default();
    inspect_runtime::service::spawn_tree_server_with_stream(
        inspector.clone(),
        settings,
        stream,
        &scope,
    );
    scope.join().await;
}

pub fn new_example_table(node: &Node) -> Table {
    let table_node_name = unique_name("table");
    Table::new(3, 3, &table_node_name, node.create_child(&table_node_name))
}

pub fn new_example_int_array(node: &Node) -> IntArrayProperty {
    let int_array = node.create_int_array(unique_name("array"), 3);
    int_array.set(0, 1);
    int_array.add(1, 10);
    int_array.subtract(2, 3);
    int_array
}

pub fn new_example_uint_array(node: &Node) -> UintArrayProperty {
    let uint_array = node.create_uint_array(unique_name("array"), 3);
    uint_array.set(0, 1u64);
    uint_array.add(1, 10);
    uint_array.set(2, 3u64);
    uint_array.subtract(2, 1);
    uint_array
}

pub fn new_example_double_array(node: &Node) -> DoubleArrayProperty {
    let double_array = node.create_double_array(unique_name("array"), 3);
    double_array.set(0, 0.25);
    double_array.add(1, 1.25);
    double_array.subtract(2, 0.75);
    double_array
}

pub fn new_example_int_linear_hist(node: &Node) -> IntLinearHistogramProperty {
    populated(
        node.create_int_linear_histogram(
            unique_name("histogram"),
            inspect::LinearHistogramParams { floor: -10, step_size: 5, buckets: 3 },
        ),
        PopulateParams { floor: -20, step: 1, count: 40 },
    )
}

pub fn new_example_uint_linear_hist(node: &Node) -> UintLinearHistogramProperty {
    populated(
        node.create_uint_linear_histogram(
            unique_name("histogram"),
            inspect::LinearHistogramParams { floor: 5, step_size: 5, buckets: 3 },
        ),
        PopulateParams { floor: 0, step: 1, count: 40 },
    )
}

pub fn new_example_double_linear_hist(node: &Node) -> DoubleLinearHistogramProperty {
    populated(
        node.create_double_linear_histogram(
            unique_name("histogram"),
            inspect::LinearHistogramParams { floor: 0.0, step_size: 0.5, buckets: 3 },
        ),
        PopulateParams { floor: -1.0, step: 0.1, count: 40 },
    )
}

pub fn new_example_int_exp_hist(node: &Node) -> IntExponentialHistogramProperty {
    populated(
        node.create_int_exponential_histogram(
            unique_name("histogram"),
            inspect::ExponentialHistogramParams {
                floor: -10,
                initial_step: 5,
                step_multiplier: 2,
                buckets: 3,
            },
        ),
        PopulateParams { floor: -20, step: 1, count: 40 },
    )
}

pub fn new_example_uint_exp_hist(node: &Node) -> UintExponentialHistogramProperty {
    populated(
        node.create_uint_exponential_histogram(
            unique_name("histogram"),
            inspect::ExponentialHistogramParams {
                floor: 0,
                initial_step: 1,
                step_multiplier: 2,
                buckets: 3,
            },
        ),
        PopulateParams { floor: 0, step: 1, count: 40 },
    )
}

pub fn new_example_double_exp_hist(node: &Node) -> DoubleExponentialHistogramProperty {
    populated(
        node.create_double_exponential_histogram(
            unique_name("histogram"),
            inspect::ExponentialHistogramParams {
                floor: 0.0,
                initial_step: 1.25,
                step_multiplier: 3.0,
                buckets: 5,
            },
        ),
        PopulateParams { floor: -1.0, step: 0.1, count: 40 },
    )
}

pub fn new_example_lazy_uint(node: &Node) {
    node.record_lazy_child("lazy-node", move || {
        async move {
            let inspector = inspect::Inspector::default();
            inspector.root().record_uint("uint", 3);
            Ok(inspector)
        }
        .boxed()
    });
}

pub fn new_example_lazy_double(node: &Node) {
    node.record_lazy_values("lazy-values", move || {
        async move {
            let inspector = inspect::Inspector::default();
            inspector.root().record_double("lazy-double", 3.25);
            Ok(inspector)
        }
        .boxed()
    });
}
