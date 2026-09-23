// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, anyhow};
use argh::{ArgsInfo, FromArgs};
use diagnostics_data::{InspectData, InspectDataBuilder, Timestamp};
use fuchsia_inspect::hierarchy::{ArrayContent, DiagnosticsHierarchy, Property};
use fuchsia_inspect::{ArrayProperty, Inspector, InspectorIntrospectionExt, Node};
use moniker::ExtendedMoniker;
use std::io::Read as _;

/// Analyze the given Inspect JSON from stdin.
/// It should be a JSON5 object with a root node named "root".
/// The minimal valid input is "{{ root: {{}} }}".
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
pub struct AnalyzeCommand {
    #[argh(switch)]
    /// accept the schema output by `ffx --machine json inspect show`
    from_tooling: bool,
}

pub fn main() -> Result<(), Error> {
    let args: AnalyzeCommand = argh::from_env();
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer)?;
    let data: Vec<InspectData> = if args.from_tooling {
        serde_json5::from_str(&buffer).map_err(|e| anyhow!("parsing JSON failed: {e:?}"))?
    } else {
        let h: DiagnosticsHierarchy =
            serde_json5::from_str(&buffer).map_err(|e| anyhow!("parsing JSON failed: {e:?}"))?;
        vec![
            InspectDataBuilder::new(
                "unknown-moniker".try_into().unwrap(),
                "unknown-url",
                Timestamp::from_nanos(0),
            )
            .with_hierarchy(h)
            .build(),
        ]
    };

    let mut results = vec![];
    for datum in data {
        results.push(analyze(datum.moniker, datum.payload.expect("payload must exist"))?);
    }

    println!("{}", AnalyzeResult(results));

    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct AnalyzeResult(Vec<IndividualAnalyzeResult>);

#[derive(Debug, PartialEq)]
pub struct IndividualAnalyzeResult {
    moniker: ExtendedMoniker,
    allocated_blocks: usize,
}

impl std::fmt::Display for AnalyzeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for result in &self.0 {
            writeln!(f, "{result:#?}")?
        }

        Ok(())
    }
}

fn write_property(node: &Node, prop: &Property) {
    match prop {
        Property::String(k, v) => node.record_string(k, v),
        Property::Bytes(k, v) => node.record_bytes(k, v),
        Property::Int(k, v) => node.record_int(k, *v),
        Property::Uint(k, v) => node.record_uint(k, *v),
        Property::Double(k, v) => node.record_double(k, *v),
        Property::Bool(k, v) => node.record_bool(k, *v),
        Property::IntArray(k, v) => {
            let ArrayContent::Values(values) = v else {
                eprintln!("histograms unsupported, skipping {k}");
                return;
            };
            let prop = node.create_int_array(k, values.len());
            for (idx, v) in values.iter().enumerate() {
                prop.set(idx, *v);
            }

            node.record(prop);
        }
        Property::DoubleArray(k, v) => {
            let ArrayContent::Values(values) = v else {
                eprintln!("histograms unsupported, skipping {k}");
                return;
            };
            let prop = node.create_double_array(k, values.len());
            for (idx, v) in values.iter().enumerate() {
                prop.set(idx, *v);
            }

            node.record(prop);
        }
        Property::UintArray(k, v) => {
            let ArrayContent::Values(values) = v else {
                eprintln!("histograms unsupported, skipping {k}");
                return;
            };
            let prop = node.create_uint_array(k, values.len());
            for (idx, v) in values.iter().enumerate() {
                prop.set(idx, *v);
            }

            node.record(prop);
        }
        Property::StringList(k, values) => {
            let prop = node.create_string_array(k, values.len());
            for (idx, v) in values.iter().enumerate() {
                prop.set(idx, v);
            }

            node.record(prop);
        }
    }
}

fn analyze(
    moniker: ExtendedMoniker,
    data: DiagnosticsHierarchy,
) -> Result<IndividualAnalyzeResult, Error> {
    let inspector = Inspector::default();
    write(inspector.root(), &data);
    let stats = inspector.stats().unwrap();

    Ok(IndividualAnalyzeResult { moniker, allocated_blocks: stats.allocated_blocks })
}

fn write(node: &Node, data: &DiagnosticsHierarchy) {
    for property in &data.properties {
        write_property(node, property);
    }

    for child in &data.children {
        let child_node = node.create_child(&child.name);
        write(&child_node, child);
        node.record(child_node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::DiagnosticsHierarchyGetter;
    use diagnostics_hierarchy::hierarchy;

    #[test]
    fn test_analyze() {
        let h = hierarchy! {
            root: {
                foo: 0u64,
            }
        };

        let moniker = ExtendedMoniker::try_from("a/b/c").unwrap();

        // HEADER, "foo", foo property
        assert_eq!(
            analyze(moniker.clone(), h).unwrap(),
            IndividualAnalyzeResult { moniker, allocated_blocks: 3 }
        );
    }

    #[fuchsia::test]
    async fn test_writer() {
        let mut expected = hierarchy! {
            root: {
                foo: 0u64,
                bar: vec![0i64, 1, 2, 3],
                baz: "string".to_string(),
                qux: {
                    foo: 1i64,
                    bar: 1.0f64,
                    baz: false,
                    qux: {
                        foo: vec![0u64, 1, 2],
                        bar: vec![3.0f64, 4.0, 5.0],
                        baz: vec!["hello".to_string(), "world".to_string()],
                    },
                },
            }
        };

        expected.sort();

        let inspector = Inspector::default();
        write(inspector.root(), &expected);
        let mut actual = inspector.get_diagnostics_hierarchy().await.into_owned();
        actual.sort();

        assert_eq!(actual, expected)
    }
}
