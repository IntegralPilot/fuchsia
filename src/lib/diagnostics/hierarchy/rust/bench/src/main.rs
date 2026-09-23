// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_criterion::criterion;
use fuchsia_inspect::Inspector;
use fuchsia_inspect::hierarchy::{DiagnosticsHierarchy, Property as HProp};

fn create_diagnostics_hierarchy() -> DiagnosticsHierarchy {
    DiagnosticsHierarchy::new_root()
}

// This is creating a hierarchy that looks like:
// root: {
//      1: {
//          11: 1,
//          2: {
//              22: 2,
//              3: {...}...
//      },
// }
fn put_data_in_hierarchy(h: &mut DiagnosticsHierarchy, data_range: std::ops::Range<i32>) {
    for num_elements in data_range {
        let path: Vec<String> = (0..num_elements)
            .into_iter()
            .map(|i| if i == 0 { "root".into() } else { i.to_string() })
            .collect();
        let n = h.get_or_add_node(&path);
        n.add_property(HProp::<String>::Int(
            format!("{}{}", num_elements, num_elements),
            num_elements as i64,
        ));
    }
}

fn get_or_add_node_reading_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range.clone());

    b.iter_with_large_drop(|| {
        for num_elements in data_range.clone() {
            let path: Vec<String> = (0..num_elements)
                .into_iter()
                .map(|i| if i == 0 { "root".into() } else { i.to_string() })
                .collect();
            let _ = h.get_or_add_node(&path);
        }
    });
}

fn from_inspector_bench(b: &mut criterion::Bencher<'_>) {
    let mut executor = fuchsia_async::LocalExecutor::default();
    let data_range = 1..100;
    let inspector = Inspector::default();
    let mut node = inspector.root().create_child("n");
    for i in data_range {
        node.record_int(format!("{}{}", i, i), i as i64);
        node = node.create_child(format!("{}", i));
    }

    b.iter_with_large_drop(|| {
        executor.run_singlethreaded(fuchsia_inspect::reader::read(&inspector)).unwrap()
    });
}

fn get_or_add_node_writing_bench(b: &mut criterion::Bencher<'_>) {
    let data_range = 1..100;
    b.iter_with_large_drop(|| {
        let mut h = create_diagnostics_hierarchy();
        put_data_in_hierarchy(&mut h, data_range.clone());
        h
    });
}

fn serialize_pretty_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range);

    b.iter(|| serde_json::to_string_pretty(&h).unwrap());
}

fn serialize_ugly_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range);

    b.iter(|| serde_json::to_string(&h).unwrap());
}

fn deserialize_pretty_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range);
    let json_pretty = serde_json::to_string_pretty(&h).unwrap();

    b.iter(|| {
        let _: DiagnosticsHierarchy = serde_json::from_str(&json_pretty).unwrap();
    });
}

fn deserialize_ugly_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range);
    let json_ugly = serde_json::to_string(&h).unwrap();

    b.iter(|| {
        let _: DiagnosticsHierarchy = serde_json::from_str(&json_ugly).unwrap();
    });
}

fn hierarchy_iteration_bench(b: &mut criterion::Bencher<'_>) {
    let mut h = create_diagnostics_hierarchy();
    let data_range = 1..100;
    put_data_in_hierarchy(&mut h, data_range);

    b.iter(|| {
        for iterator in h.property_iter() {
            match iterator {
                (_, Some(HProp::Int(name, value))) => {
                    assert_eq!(*name, format!("{}{}", value, value))
                }
                (_, wrong_thing) => {
                    panic!(
                        "got {:#?}, expected (_, Some(hierarchy::Poperty::Int(...)))",
                        wrong_thing
                    )
                }
            }
        }
    });
}

fn main() {
    let mut c = fuchsia_inspect_bench_utils::configured_criterion(
        fuchsia_inspect_bench_utils::CriterionConfig::default(),
    );

    let mut group = c.benchmark_group("fuchsia.diagnostics_hierarchy.benchmarks");
    let _ = group.bench_function("DiagnosticsHierarchy/get_or_add_node/reading", move |b| {
        get_or_add_node_reading_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/from_inspector", move |b| {
        from_inspector_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/get_or_add_node/writing", move |b| {
        get_or_add_node_writing_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/serialize/pretty", move |b| {
        serialize_pretty_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/serialize/ugly", move |b| {
        serialize_ugly_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/deserialize/pretty", move |b| {
        deserialize_pretty_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/deserialize/ugly", move |b| {
        deserialize_ugly_bench(b);
    });
    let _ = group.bench_function("DiagnosticsHierarchy/iteration", move |b| {
        hierarchy_iteration_bench(b);
    });

    group.finish();
}
