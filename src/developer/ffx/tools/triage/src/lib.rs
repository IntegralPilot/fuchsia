// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use errors::FfxError;
use fdomain_fuchsia_feedback::DataProviderProxy;
use ffx_config::EnvironmentContext;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{AvailabilityFlag, Deferred, FfxMain, FfxTool, deferred};
use fuchsia_triage::{
    ActionResultFormatter, ActionTagDirective, TriageOutput, analyze, analyze_structured,
};
use std::env;
use std::io::Write;
use std::path::PathBuf;
use target_holders::toolbox;
use tempfile::tempdir;
use triage_app_lib::file_io::{config_from_files, diagnostics_from_directory};

mod args;
mod config;
mod snapshot;
pub use snapshot::create_snapshot;

use args::TriageCommand;

pub(crate) type Writer = MachineWriter<TriageOutput>;

#[derive(FfxTool)]
#[check(AvailabilityFlag("triage.enabled"))]
pub struct TriageTool {
    #[with(deferred(toolbox()))]
    data_provider_proxy: Deferred<DataProviderProxy>,
    #[command]
    cmd: TriageCommand,
    context: EnvironmentContext,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for TriageTool {
    type Writer = Writer;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        triage(&self.context, self.data_provider_proxy.await.ok(), writer, self.cmd)
            .await
            .map_err(Into::into)
    }
}

pub async fn triage(
    context: &EnvironmentContext,
    data_provider_proxy: Option<DataProviderProxy>,
    writer: Writer,
    cmd: TriageCommand,
) -> Result<()> {
    triage_impl(context, data_provider_proxy, cmd, writer).await.map_err(flatten_error)
}

async fn triage_impl(
    context: &EnvironmentContext,
    data_provider_proxy: Option<DataProviderProxy>,
    cmd: TriageCommand,
    writer: Writer,
) -> Result<()> {
    let TriageCommand { config, data, tags, exclude_tags } = cmd;

    let current_dir = env::current_dir()?;
    let config_files = config::get_or_default_config_files(context, config, current_dir).await?;

    let data_directory = match data {
        Some(d) => PathBuf::from(d),
        None => {
            let snapshot_tempdir =
                tempdir().context("Unable to create temporary snapshot directory.")?;
            let data_provider_proxy =
                data_provider_proxy.ok_or_else(|| anyhow!("Unable to get data provider."))?;
            let _ = snapshot::create_snapshot(data_provider_proxy, snapshot_tempdir.path()).await?;
            snapshot_tempdir.keep()
        }
    };

    analyze_snapshot(config_files, data_directory, tags, exclude_tags, writer)
}

/// Analyze snapshot in the data_directory using config_files.
fn analyze_snapshot(
    config_files: Vec<PathBuf>,
    data_directory: PathBuf,
    tags: Vec<String>,
    exclude_tags: Vec<String>,
    mut writer: Writer,
) -> Result<()> {
    let parse_result =
        config_from_files(&config_files, &ActionTagDirective::from_tags(tags, exclude_tags))
            .context("Unable to parse config files.")?;
    parse_result.validate().context("Unable to validate config files.")?;

    let diagnostic_data = diagnostics_from_directory(&data_directory)
        .context("Unable to process disagnostics data.")?;

    if writer.is_machine() {
        let triage_output = analyze_structured(&diagnostic_data, &parse_result)
            .context("Unable to analyze data.")?;

        writer
            .machine(&triage_output)
            .context("Unable to write structured output to destination.")?;
    } else {
        let action_results =
            analyze(&diagnostic_data, &parse_result).context("Unable to analyze data.")?;

        let results_formatter = ActionResultFormatter::new(&action_results);

        let output = results_formatter.to_string();

        writer
            .write_fmt(format_args!("{}\n", output))
            .context("Unable to write to destination.")?;
    }
    Ok(())
}

/// Flattens the errors attached by context.
fn flatten_error(e: anyhow::Error) -> anyhow::Error {
    FfxError::Error(
        anyhow::anyhow!(
            "Triage encountered an error.{}",
            e.chain()
                .enumerate()
                .map(|(i, e)| format!("\n{:indent$}{}", "", e, indent = i))
                .collect::<Vec<String>>()
                .concat()
        ),
        -1,
    )
    .into()
}

#[allow(unused_macros)]
#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use super::*;
    use fdomain_fuchsia_feedback::{DataProviderRequest, Snapshot};
    use ffx_writer::{Format, TestBuffers};
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use std::sync::LazyLock;
    use target_holders::fake_proxy;

    macro_rules! test_file {
        (config $filename:expr) => {
            concat!("../../../../../../src/diagnostics/triage/test_data/config/", $filename)
        };
        (snapshot $filename:expr) => {
            concat!("../../../../../../src/diagnostics/triage/test_data/snapshot/", $filename)
        };
    }

    macro_rules! test_data {
        ($(configs: $($config: literal), + $(,)?)?
         $(snapshots: $($snapshot: literal), + $(,)?)?) => {
            {
                let mut m = HashMap::new();
                $($(m.insert($config, include_str!(test_file!(config $config)));)+)?
                $($(m.insert($snapshot, include_str!(test_file!(snapshot $snapshot)));)+)?
                m
            }
        }
    }

    macro_rules! structured_output {
        ($filename: literal) => {
            include_str!(concat!("../test_data/structured/", $filename, ".golden.json"))
        };
    }

    static TEST_DATA_MAP: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
        test_data! {
            // Test config files
            configs:
                "annotation_tests.triage", "error_rate.triage", "log_tests.triage",
                "map_fold.triage","other.triage", "sample.triage",
                "sample_bundle.json", "sample_bundle_file_type_error.json",
                "sample_bundle_files_error.json", "sample_tags.triage",
            // Test snapshot files
            snapshots:
                "inspect.json", "annotations.json"
        }
    });

    fn setup_fake_data_provider_server() -> DataProviderProxy {
        let client = fdomain_local::local_client_empty();
        fake_proxy(client, move |req| match req {
            DataProviderRequest::GetSnapshot { params, responder } => {
                let _channel = params.response_channel.unwrap();

                let snapshot = Snapshot::default();
                responder.send(snapshot).unwrap();
            }
            _ => unreachable!(),
        })
    }

    fn create_test_snapshot(snapshot_dir: &Path) {
        ["inspect.json", "annotations.json"].iter().for_each(|file_name| {
            let snapshot_file = snapshot_dir.join(file_name);
            let snapshot_data = TEST_DATA_MAP.get(file_name).expect("Unknown test snapshot data.");
            fs::write(&snapshot_file, snapshot_data).expect("Unable to write test snapshot data.");
        });
    }

    fn match_structured_output(output: &str, expected: &str) {
        let expected_output: serde_json::Value =
            serde_json::from_str(expected).expect("Unable to deserialize expected output.");
        let actual_output: serde_json::Value =
            serde_json::from_str(output).expect("Unable to deserialize actual output.");

        pretty_assertions::assert_eq!(
            actual_output,
            expected_output,
            "Structured output does not match golden."
        );
    }

    async fn run_triage_test(
        cmd: TriageCommand,
        buffers: &mut TestBuffers,
        format: Option<Format>,
    ) {
        let env = ffx_config::test_init().unwrap();
        let writer = Writer::new_test(format, &buffers);
        let data_provider_proxy = setup_fake_data_provider_server();
        let result = triage_impl(&env.context, Some(data_provider_proxy), cmd, writer)
            .await
            .map_err(flatten_error);

        if let Err(e) = result {
            buffers
                .stdout
                .write_fmt(format_args!("{}", e))
                .expect("Failed to write to destination.")
        }
    }

    macro_rules! triage_integration_test {
        (@internal
         $name: ident,
         $(configs: $($config: literal), + $(,)?)?
         $(tags: $($tag: literal), + $(,)?)?
         $(exclude_tags: $($exclude_tag: literal), + $(,)?)?
         substring: $substring: literal,
         $(golden_structured_outfile: $golden_structured_outfile: literal)?
         should_contain: $should_contain: expr
         ) => {
            #[fuchsia::test]
            async fn $name() -> Result<()> {
                let config = vec![$($($config),+)?];
                let config_tempdir = tempdir()?;

                let config_files: Result<Vec<String>> = config
                    .iter()
                    .map(|config| {
                        let config_data =
                            TEST_DATA_MAP.get(config).expect("Unknown test config data.");
                        let config_file_path = config_tempdir.path().join(config);
                        fs::write(&config_file_path, config_data)?;
                        Ok(config_file_path.to_string_lossy().into())
                    })
                    .collect();
                let config_files = config_files?;

                let snapshot_tempdir = tempdir()?;
                create_test_snapshot(snapshot_tempdir.path());

                let tags = vec![$($($tag.into()),+)?];
                let exclude_tags =vec![$($($exclude_tag.into()),+)?];

                // Test unstructured output.
                let mut test_buffers = TestBuffers::default();
                run_triage_test(TriageCommand {
                    config: config_files.clone(),
                    data: Some(snapshot_tempdir.path().to_string_lossy().into()),
                    tags: tags.clone(),
                    exclude_tags: exclude_tags.clone(),
                },&mut test_buffers, None).await;

                let output = test_buffers.into_stdout_str();

                pretty_assertions::assert_eq!(
                    output.contains($substring),
                    $should_contain,
                    "{} does not contain: {}", output, $substring);

                $(
                // Test structured output only if golden_structured_outfile is provided.
                let mut test_buffers = TestBuffers::default();
                run_triage_test(TriageCommand {
                    config: config_files,
                    data: Some(snapshot_tempdir.path().to_string_lossy().into()),
                    tags: tags.clone(),
                    exclude_tags: exclude_tags.clone(),
                },&mut test_buffers, Some(Format::Json)).await;

                let output = test_buffers.into_stdout_str();

                match_structured_output(&output, structured_output!($golden_structured_outfile));
                )?

                Ok(())
            }
        };
        ($name: ident,
         $(configs: $($config: literal), + $(,)?)?
         $(tags: $($tag: literal), + $(,)?)?
         $(exclude_tags: $($exclude_tag: literal), + $(,)?)?
         substring: $substring: literal,
         $(golden_structured_outfile: $golden_structured_outfile: literal)?
         ) => {
            triage_integration_test!(
                @internal
                $name,
                $(configs: $($config),+)?
                $(tags: $($tag),+)?
                $(exclude_tags: $($exclude_tag),+)?
                substring: $substring,
                $(golden_structured_outfile: $golden_structured_outfile)?
                should_contain: true);
        };
        ($name: ident,
         $(configs: $($config: literal), + $(,)?)?
         $(tags: $($tag: literal), + $(,)?)?
         $(exclude_tags: $($exclude_tag: literal), + $(,)?)?
         substring: not_contains $substring: literal,
         $(golden_structured_outfile: $golden_structured_outfile: literal)?
        )=> {
            triage_integration_test!(
                @internal
                $name,
                $(configs: $($config),+)?
                $(tags: $($tag),+)?
                $(exclude_tags: $($exclude_tag),+)?
                substring: $substring,
                $(golden_structured_outfile: $golden_structured_outfile)?
                should_contain: false);
        };
    }

    triage_integration_test!(
        successfully_read_correct_files,
        configs: "other.triage", "sample.triage",
        substring: not_contains "Couldn't",
        golden_structured_outfile: "successfully_read_correct_files"
    );

    triage_integration_test!(
        use_namespace_in_actions,
        configs: "other.triage", "sample.triage",
        substring: "[WARNING] yes on A!",
        golden_structured_outfile: "use_namespace_in_actions"
    );

    triage_integration_test!(
        use_namespace_in_metrics,
        configs: "other.triage", "sample.triage",
        substring: "[WARNING] Used some of disk",
        golden_structured_outfile: "use_namespace_in_metrics"
    );

    triage_integration_test!(
        fail_on_missing_namespace,
        configs: "sample.triage",
        substring: "Bad namespace",
        golden_structured_outfile: "fail_on_missing_namespace"
    );

    triage_integration_test!(
        include_tagged_actions,
        configs: "sample_tags.triage",
        tags: "foo",
        substring: "[WARNING] trigger foo tag",
        golden_structured_outfile: "include_tagged_actions"
    );

    triage_integration_test!(
        only_runs_included_actions,
        configs: "sample_tags.triage",
        tags: "not_included",
        substring: "",
        golden_structured_outfile: "only_runs_included_actions"
    );

    triage_integration_test!(
        included_tags_override_excludes,
        configs: "sample_tags.triage",
        tags: "foo",
        exclude_tags: "foo",
        substring: "[WARNING] trigger foo tag",
        golden_structured_outfile: "included_tags_override_excludes"
    );

    triage_integration_test!(
        exclude_actions_with_excluded_tags,
        configs: "sample_tags.triage",
        exclude_tags: "foo",
        substring: "",
        golden_structured_outfile: "exclude_actions_with_excluded_tags"
    );

    triage_integration_test!(
        error_rate_with_moniker_payload,
        configs: "error_rate.triage",
        substring: "[WARNING] Error rate for app.cm is too high",
        golden_structured_outfile: "error_rate_with_moniker_payload"
    );

    triage_integration_test!(
        annotation_test,
        configs: "annotation_tests.triage",
        substring: "[WARNING] Running on a chromebook",
        golden_structured_outfile: "annotation_test"
    );

    triage_integration_test!(
        annotation_test2,
        configs: "annotation_tests.triage",
        substring: not_contains "[WARNING] Not using a chromebook",
        golden_structured_outfile: "annotation_test2"
    );

    triage_integration_test!(
        map_fold_test,
        configs: "map_fold.triage",
        substring: "Everything worked as expected",
        golden_structured_outfile: "map_fold_test"
    );

    triage_integration_test!(
        log_tests,
        configs: "log_tests.triage",
        substring: "",
        golden_structured_outfile: "log_tests"
    );

    triage_integration_test!(
        bundle_test,
        configs: "sample_bundle.json",
        substring: "gauge: 120",
        golden_structured_outfile: "bundle_test"
    );

    triage_integration_test!(
        bundle_files_error_test,
        configs: "sample_bundle_files_error.json",
        substring: "looks like a bundle, but key 'files' is not an object",
    );

    triage_integration_test!(
        bundle_file_type_error_test,
        configs: "sample_bundle_file_type_error.json",
        substring: "looks like a bundle, but key file2 must contain a string",
    );
}
