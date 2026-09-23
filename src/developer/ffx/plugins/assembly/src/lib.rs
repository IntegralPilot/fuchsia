// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use assembly_api::{create_system, product_assembly};
use async_trait::async_trait;
use errors::FfxError;
use ffx_assembly_args::*;
use ffx_config::EnvironmentContext;
use ffx_writer::VerifiedMachineWriter;
use fho::{FfxContext, FfxMain, FfxTool, Result};
mod operations;
mod subpackage_blobs_package;
use assembly_components as _;

#[derive(Debug, PartialEq, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AssemblyResult {
    SizeCheckProduct { fits: bool },
    SizeCheckPackage { fits: bool },
    Ok,
}

#[derive(FfxTool)]
#[target(None)]
pub struct AssemblyTool {
    #[command]
    cmd: AssemblyCommand,

    context: EnvironmentContext,
}

#[async_trait(?Send)]
impl FfxMain for AssemblyTool {
    type Writer = VerifiedMachineWriter<AssemblyResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: VerifiedMachineWriter<AssemblyResult>) -> Result<()> {
        let assembly_result = self.run(&mut writer).await.map_err(flatten_error_sources)?;
        writer.machine(&assembly_result).bug()?;
        Ok(())
    }
}

impl AssemblyTool {
    async fn run(
        self,
        writer: &mut VerifiedMachineWriter<AssemblyResult>,
    ) -> anyhow::Result<AssemblyResult> {
        // Dispatch to the correct operation based on the command.
        // The context() is used to display which operation failed in the event of
        // an error.
        Ok(match self.cmd.op_class {
            OperationClass::CreateSystem(args) => {
                create_system(args.into()).context("Create System")?;
                AssemblyResult::Ok
            }
            OperationClass::CreateUpdate(args) => {
                operations::create_update::create_update(&self.context, args)
                    .context("Create Update Package")?;
                AssemblyResult::Ok
            }
            OperationClass::Product(args) => {
                product_assembly(&self.context, args.into()).context("Product Assembly")?;
                AssemblyResult::Ok
            }
            OperationClass::SizeCheck(args) => match args.op_class {
                SizeCheckOperationClass::Package(args) => {
                    let fits = operations::size_check::package::verify_package_budgets(
                        writer,
                        &self.context,
                        args,
                    )
                    .context("Package size checker")?;
                    AssemblyResult::SizeCheckPackage { fits }
                }
                SizeCheckOperationClass::Product(args) => {
                    // verify_product_budgets() returns a boolean that indicates whether the budget was
                    // exceeded or not. We don't intend to fail the build when budgets are exceeded
                    // at the moment, but we will return it in the machine output.
                    let fits =
                        operations::size_check::product::verify_product_budgets(writer, args)
                            .await
                            .context("Product size checker")?;
                    AssemblyResult::SizeCheckProduct { fits }
                }
            },
        })
    }
}

/// Wrap the anyhow::Error in an FfxError that's into'd an anyhow::Error
/// again.  This removes the "BUG: An internal error occurred" line from the
/// output, creating a multiline display of the context() entries.
///
/// The multi-line format which enumerates each of the intervening Context
/// lines is used so that there is enough information in the error to
/// understand what failed and why, as errors such as "No such file or
/// directory" isn't terribly useful, in an operation that may need to open
/// 100s of files, which may have been listed inside a file, which is listed
/// in a file that's given on the command line.
///
/// The error string constructed here is:
///  ```
///  <First Context> Failed:
///      1.  <Next Context>
///      2.  <Next Context>
///      ...
///      N.  <root error>
///  ```
///
///
///
fn flatten_error_sources(e: anyhow::Error) -> fho::Error {
    FfxError::Error(
        anyhow::anyhow!(
            "{} Failed{}",
            e,
            e.chain()
                .skip(1)
                .enumerate()
                .map(|(i, e)| format!("\n  {: >3}.  {}", i + 1, e))
                .collect::<Vec<String>>()
                .concat()
        ),
        -1,
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn make_flattened_error_display_string(e: anyhow::Error) -> String {
        format!("{}", flatten_error_sources(e))
    }

    #[test]
    fn test_error_source_flatten_no_context() {
        assert_eq!(
            "Some Operation Failed",
            make_flattened_error_display_string(anyhow!("Some Operation"))
        );
    }

    // The order of context's is "in-side-out", the root-most error is
    // created first, and then the context() is attached on all of the
    // returned values, so they are created in the opposite order that they
    // are displayed.

    #[test]
    fn test_error_source_flatten_one_context() {
        let expected = "Some Other Operation Failed\n    1.  some failure";
        let error = anyhow!("some failure");
        let error = error.context("Some Other Operation");
        assert_eq!(expected, make_flattened_error_display_string(error));
    }

    #[test]
    fn test_error_source_flatten_two_contexts() {
        let expected = "Some Operation Failed\n    1.  some context\n    2.  some failure";
        let error = anyhow!("some failure");
        let error = error.context("some context");
        let error = error.context("Some Operation");
        assert_eq!(expected, make_flattened_error_display_string(error));
    }

    #[test]
    fn test_error_source_flatten_three_contexts() {
        let expected = r#"Some Operation Failed
    1.  some context
    2.  more context
    3.  some failure"#;
        let error = anyhow!("some failure")
            .context("more context")
            .context("some context")
            .context("Some Operation");
        assert_eq!(expected, make_flattened_error_display_string(error));
    }

    #[test]
    fn test_assembly_result_serialization() {
        assert_eq!(serde_json::to_string(&AssemblyResult::Ok).unwrap(), r#"{"type":"ok"}"#);
        assert_eq!(
            serde_json::to_string(&AssemblyResult::SizeCheckProduct { fits: true }).unwrap(),
            r#"{"type":"size_check_product","data":{"fits":true}}"#
        );
        assert_eq!(
            serde_json::to_string(&AssemblyResult::SizeCheckProduct { fits: false }).unwrap(),
            r#"{"type":"size_check_product","data":{"fits":false}}"#
        );
        assert_eq!(
            serde_json::to_string(&AssemblyResult::SizeCheckPackage { fits: true }).unwrap(),
            r#"{"type":"size_check_package","data":{"fits":true}}"#
        );
        assert_eq!(
            serde_json::to_string(&AssemblyResult::SizeCheckPackage { fits: false }).unwrap(),
            r#"{"type":"size_check_package","data":{"fits":false}}"#
        );
    }

    #[test]
    fn test_assembly_result_schema() {
        VerifiedMachineWriter::<AssemblyResult>::verify_schema(
            &serde_json::to_value(&AssemblyResult::Ok).unwrap(),
        )
        .unwrap();
        VerifiedMachineWriter::<AssemblyResult>::verify_schema(
            &serde_json::to_value(&AssemblyResult::SizeCheckProduct { fits: true }).unwrap(),
        )
        .unwrap();
        VerifiedMachineWriter::<AssemblyResult>::verify_schema(
            &serde_json::to_value(&AssemblyResult::SizeCheckProduct { fits: false }).unwrap(),
        )
        .unwrap();
        VerifiedMachineWriter::<AssemblyResult>::verify_schema(
            &serde_json::to_value(&AssemblyResult::SizeCheckPackage { fits: true }).unwrap(),
        )
        .unwrap();
        VerifiedMachineWriter::<AssemblyResult>::verify_schema(
            &serde_json::to_value(&AssemblyResult::SizeCheckPackage { fits: false }).unwrap(),
        )
        .unwrap();
    }
}
