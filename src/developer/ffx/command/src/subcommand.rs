// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{
    CliArgsInfo, FfxCommandLine, FfxContext, FfxToolInfo, FfxToolSource, MetricsSession, Result,
    SubCommandInfo, ToolRunner, ToolSuite,
};
use ffx_command_error::bug;
use ffx_config::{EnvironmentContext, Sdk, SelectMode};
use fho_metadata::FhoToolMetadata;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// The config key for holding subtool search paths.
pub const FFX_SUBTOOL_PATHS_CONFIG: &str = "ffx.subtool-search-paths";

/// The config key for holding the external subtool manifest.
pub const FFX_SUBTOOL_MANIFEST_CONFIG: &str = "ffx.subtool-manifest";

/// Path information about a subtool
#[derive(Clone, Debug)]
struct SubToolLocation {
    source: FfxToolSource,
    name: String,
    tool_path: PathBuf,
    metadata_path: PathBuf,
}

/// A subtool discovered in a user's workspace or sdk
#[derive(Clone, Debug)]
pub struct ExternalSubTool {
    cmd_line: FfxCommandLine,
    context: EnvironmentContext,
    path: PathBuf,
}

impl ExternalSubTool {
    pub fn run_and_capture(self) -> Result<(ExitStatus, String, String)> {
        let mut command = std::process::Command::new(&self.path);
        command
            .env(
                EnvironmentContext::FFX_BIN_ENV,
                self.context.rerun_bin().map_err(ffx_config::macro_deps::anyhow::Error::from)?,
            )
            .args(self.cmd_line.ffx_args_iter().chain(self.cmd_line.subcmd_iter()))
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child = command.spawn().bug_context("Failed to spawn external subtool")?;

        let output = child.wait_with_output().bug_context("Failed to wait for subtool output")?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        Ok((output.status, stdout, stderr))
    }
}
#[derive(Clone, Debug)]
pub struct ExternalSubToolSuite {
    context: EnvironmentContext,
    workspace_tools: HashMap<String, SubToolLocation>,
}

//LINT.IfChange(subtool_manifest)
/// Subtool manifest entry
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SubToolManifestEntry {
    pub(crate) category: String,
    pub(crate) executable: PathBuf,
    pub(crate) executable_metadata: PathBuf,
    pub(crate) name: String,
}
// LINT.ThenChange(//src/developer/ffx/build/ffx_tool.gni:subtool_manifest)

#[async_trait::async_trait(?Send)]
impl ToolRunner for ExternalSubTool {
    async fn run(self: Box<Self>, _metrics: MetricsSession) -> Result<ExitStatus> {
        // fho v0: Run the exact same command, just with the first argument
        // replaced with the 'real' tool location. We will also exec() it so
        // we don't have to do signal management, but later versions of fho
        // will likely need to do more here.
        let exec_err = std::process::Command::new(&self.path)
            .env(
                EnvironmentContext::FFX_BIN_ENV,
                self.context.rerun_bin().map_err(ffx_config::macro_deps::anyhow::Error::from)?,
            )
            .args(self.cmd_line.ffx_args_iter().chain(self.cmd_line.subcmd_iter()))
            .exec();

        // Because we use exec above, we are only ever here if something went
        // wrong with the exec. We will never return Ok() for this function with
        // fho v0.
        Err(exec_err).bug_context("Running external subtool")
        // note: we specifically do not want to report metrics here, as we're running the command externally.
        // The final command is the one that knows how to redact its own args, so it will do it itself.
    }
}

impl ExternalSubToolSuite {
    /// Load subtools from `subtool_paths` and use `context` for the environment context.
    /// This is used both by the main implementation of [`ExternalSubToolSuite::from_env`] and
    /// in tests to redirect to different subtool paths.
    fn with_tools_from(context: EnvironmentContext, subtool_paths: &[impl AsRef<Path>]) -> Self {
        let workspace_tools =
            find_workspace_tools(subtool_paths).map(|tool| (tool.name.to_owned(), tool)).collect();
        Self { context, workspace_tools }
    }

    /// Loads the subtools from the provided manifest. This avoids scanning directories and
    /// reading files that are not expected and will break the hermeticity expectations.
    fn with_tools_manifest(context: EnvironmentContext, subtool_manifest: PathBuf) -> Self {
        let file = fs::File::open(&subtool_manifest).expect("file should open");
        let entries: Vec<SubToolManifestEntry> =
            serde_json::from_reader(file).expect("manifest should be json");

        let workspace_tools: HashMap<String, SubToolLocation> = entries
            .iter()
            .filter_map(|item| {
                SubToolLocation::from_path(
                    FfxToolSource::Workspace,
                    &item.executable,
                    &item.executable_metadata,
                )
            })
            .map(|tool| (tool.name.to_owned(), tool))
            .collect();

        if workspace_tools.is_empty() {
            // This really should not happen, but it is not fatal, so just log an error...
            log::error!(
                "Subtool manifest {subtool_manifest:?} is empty. No workspace subtools defined."
            );
        }
        Self { context, workspace_tools }
    }

    pub fn find_workspace_tool(&self, ffx_cmd: &FfxCommandLine) -> Option<ExternalSubTool> {
        let name = ffx_cmd.global.subcommand.first()?;
        let cmd = match self.workspace_tools.get(name).and_then(SubToolLocation::validate_tool) {
            Some(FfxToolInfo { path: Some(path), .. }) => {
                let context = self.context.clone();
                let cmd_line = ffx_cmd.clone();
                ExternalSubTool { cmd_line, context, path }
            }
            _ => return None,
        };
        Some(cmd)
    }

    pub fn find_sdk_tool(&self, sdk: &Sdk, ffx_cmd: &FfxCommandLine) -> Option<ExternalSubTool> {
        let name = format!("ffx-{}", ffx_cmd.global.subcommand.first()?);
        let ffx_tool = sdk.get_ffx_tool(&name)?;
        let location = SubToolLocation::from_path(
            FfxToolSource::Sdk,
            &ffx_tool.executable,
            &ffx_tool.metadata,
        )?;
        let Some(FfxToolInfo { path: Some(path), .. }) = location.validate_tool() else {
            return None;
        };
        let context = self.context.clone();
        let cmd_line = ffx_cmd.clone();
        Some(ExternalSubTool { cmd_line, context, path })
    }
}

#[async_trait::async_trait(?Send)]
impl ToolSuite for ExternalSubToolSuite {
    fn from_env(env: &EnvironmentContext) -> Result<Self> {
        let subtool_manifest: PathBuf =
            env.query(FFX_SUBTOOL_MANIFEST_CONFIG).build().get_file(env).unwrap_or_default();

        // If the subtool manifest is configured, it use it to load the information for
        // external subtools. Otherwise scan the directories. The manifest file is used when
        // ffx is being run hermetically, and should not scan and read directories.
        if subtool_manifest.exists() {
            log::info!("Initializing ExternalSubToolSuite from {subtool_manifest:?} ");
            Ok(Self::with_tools_manifest(env.clone(), subtool_manifest))
        } else {
            let subtool_config: Vec<Value> = env
                .query(FFX_SUBTOOL_PATHS_CONFIG)
                .select(SelectMode::All)
                .build()
                .get_file(env)
                .unwrap_or_else(|_| vec![]);
            log::info!("Initializing ExternalSubToolSuite from {subtool_config:?}");
            Ok(Self::with_tools_from(env.clone(), &get_subtool_paths(subtool_config)))
        }
    }

    fn global_command_list() -> &'static [&'static argh::CommandInfo] {
        &[]
    }

    async fn get_args_info(&self) -> Result<CliArgsInfo> {
        let mut external_args_info: CliArgsInfo = Default::default();

        // Pass the same command line to each of the external subcommands which
        // prints the json encoded args info.
        let mut argv = vec!["ffx".to_string()];
        let env_args = self
            .context
            .env_args()
            .map_err(|e| bug!("Failed to serialize environment context arguments: {e}"))?;
        argv.extend(env_args);
        argv.extend(["--machine".to_string(), "json".to_string(), "--help".to_string()]);

        let cmdline = FfxCommandLine::from_args_for_help(&argv).bug_context("cmd line for help")?;

        for tool in &self.command_list().await {
            let mut c = std::process::Command::new(
                &tool.path.clone().ok_or(bug!("could not get tool path"))?,
            );
            // Pass FFX_BIN if resolvable so subtools can anchor their environment to the
            // parent ffx binary if needed. In test harnesses or isolated environments where ffx
            // is not built or packaged, rerun_bin() may fail, but subtools only need CLI flags
            // to output their help/schema metadata.
            if let Ok(rerun_bin) = self.context.rerun_bin() {
                c.env(EnvironmentContext::FFX_BIN_ENV, rerun_bin);
            }
            let help_cmd = c.args(cmdline.ffx_args_iter().chain(cmdline.subcmd_iter()));

            let output = help_cmd.output().bug_context("sub tool help")?;
            let outval = String::from_utf8_lossy(&output.stdout);
            let subcmd_args_info: CliArgsInfo = serde_json::from_slice(&output.stdout)
                .bug_context(format!(
                    "json parsing:{outval} {}",
                    String::from_utf8_lossy(&output.stderr)
                ))?;

            external_args_info.commands.push(SubCommandInfo {
                name: subcmd_args_info.name.clone(),
                command: subcmd_args_info,
            });
        }
        Ok(external_args_info)
    }

    async fn command_list(&self) -> Vec<FfxToolInfo> {
        let mut tools: Vec<_> = self.workspace_tools.values().cloned().collect();
        // if we have workspace tools, there is no need to get sdk tools.
        if tools.is_empty() {
            if let Ok(sdk) = self.context.get_sdk() {
                for ffx_tool in sdk.get_ffx_tools() {
                    SubToolLocation::from_path(
                        FfxToolSource::Sdk,
                        &ffx_tool.executable,
                        &ffx_tool.metadata,
                    )
                    .map(|loc| tools.push(loc));
                }
            }
        }
        tools.iter().filter_map(SubToolLocation::validate_tool).collect()
    }

    async fn try_from_args(
        &self,
        ffx_cmd: &FfxCommandLine,
    ) -> Result<Option<Box<dyn ToolRunner + '_>>> {
        // look in the workspace first
        if let Some(cmd) = self.find_workspace_tool(ffx_cmd) {
            log::info!("Found workspace tool {cmd:?}");
            return Ok(Some(Box::new(cmd)));
        }
        // then try the sdk
        let sdk_cmd = self.context.get_sdk().ok().and_then(|sdk| self.find_sdk_tool(&sdk, ffx_cmd));
        if let Some(cmd) = sdk_cmd {
            log::info!("Found SDK tool {cmd:?}");
            return Ok(Some(Box::new(cmd)));
        }
        log::warn!("Did not find external tool for {ffx_cmd:?}");
        // and we're done
        Ok(None)
    }

    async fn try_runner_from_name(
        &self,
        ffx_cmd: &FfxCommandLine,
    ) -> Result<Option<Box<dyn ToolRunner + '_>>> {
        // External commands do not parse the entire command, so it is OK to call
        // try_from_args here.
        self.try_from_args(ffx_cmd).await
    }
}

/// Loads a list of subtool paths from an array of values, flattening them into
/// a list of [`PathBuf`]s.
fn get_subtool_paths(subtools: Vec<Value>) -> Vec<PathBuf> {
    use Value::*;
    subtools
        .into_iter()
        .flat_map(|val| match val {
            Array(arr) => arr.into_iter(),
            other => vec![other].into_iter(),
        })
        .filter_map(|val| val.as_str().map(PathBuf::from))
        .collect()
}

/// Searches a set of directories for tools matching the path `ffx-<name>`
/// and returns information about them based on known abis
fn find_workspace_tools<P>(subtool_paths: &[P]) -> impl Iterator<Item = SubToolLocation> + '_
where
    P: AsRef<Path>,
{
    subtool_paths
        .iter()
        .filter_map(|path| {
            Some(std::fs::read_dir(path.as_ref()).ok()?.filter_map(move |entry| {
                let entry = entry.ok()?;
                SubToolLocation::from_path(
                    FfxToolSource::Workspace,
                    &entry.path(),
                    &entry.path().with_extension("json"),
                )
            }))
        })
        .flatten()
}

impl SubToolLocation {
    /// Evaluate the given path for if it looks like a subtool based on filename and the
    /// presence of a metadata file.
    fn from_path(
        source: FfxToolSource,
        tool_path: &Path,
        metadata_path: &Path,
    ) -> Option<SubToolLocation> {
        let file_name = tool_path.file_name()?.to_str()?;
        if let Some(suffix) = file_name.strip_prefix("ffx-") {
            let name = suffix.to_lowercase();
            // require the presence of a metadata file
            if metadata_path.exists() {
                let tool_path = tool_path.to_owned();
                let metadata_path = metadata_path.to_owned();
                return Some(SubToolLocation { source, name, tool_path, metadata_path });
            }
        }
        None
    }

    /// Loads the details of the metadata from the file to validate that it is a runnable
    /// command with the current fho version and obtaining extra metadata from the metadata
    /// file.
    ///
    /// Doing this in two steps avoids reading files unnecessarily until we want to either
    /// run one or list it.
    fn validate_tool(&self) -> Option<FfxToolInfo> {
        // bail early if for whatever reason we can't read the metadata.
        let metadata: FhoToolMetadata =
            File::open(&self.metadata_path).ok().and_then(|f| serde_json::from_reader(f).ok())?;
        // also if it requires an fho version we don't support
        metadata.is_supported()?;
        // ignore the tool if the metadata's name is incorrect
        if metadata.name == self.name {
            let source = self.source;
            let name = metadata.name;
            let description = metadata.description;
            let path = Some(self.tool_path.to_owned());
            Some(FfxToolInfo { source, name, description, path })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ffx;
    use fho_metadata::{FhoDetails, Only};
    use serde_json::json;
    use std::collections::HashSet;
    use std::io::Write;

    enum MockMetadata<'a> {
        Valid(FhoToolMetadata),
        Invalid(&'a str),
        NotThere,
    }
    use MockMetadata::*;

    fn check_ffx_tool(source: FfxToolSource, path: &Path) -> Option<FfxToolInfo> {
        SubToolLocation::from_path(source, path, &path.with_extension("json"))
            .as_ref()
            .and_then(SubToolLocation::validate_tool)
    }

    // Sets up a mock subtool in `dir` with the name `subtool_name` and, adjacent metadata based on the
    // `metadata` argument.
    fn create_mock_subtool(dir: &Path, subtool_name: &str, metadata: MockMetadata<'_>) -> PathBuf {
        let subtool_path = dir.join(subtool_name);
        let metadata_path = subtool_path.with_extension("json");
        File::create(&subtool_path).expect("creating subtool file");
        match metadata {
            Valid(meta) => {
                let file = File::create(&metadata_path).expect("creating subtool metadata");
                serde_json::to_writer(file, &meta).expect("Writing subtool metadata")
            }
            Invalid(s) => {
                let mut file =
                    File::create(&metadata_path).expect("creating invalid subtool metadata");
                write!(file, "{s}").expect("Writing invalid subtool metadata")
            }
            _ => {}
        }
        subtool_path
    }

    #[test]
    fn check_non_existent() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        assert!(
            check_ffx_tool(FfxToolSource::Workspace, &tempdir.path().join("ffx-non-existent"))
                .is_none(),
            "Non-existent subtool should be None"
        );
    }

    #[test]
    fn check_no_metadata() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let name = "ffx-no-metadata";
        let subtool = create_mock_subtool(tempdir.path(), name, NotThere);
        assert!(
            check_ffx_tool(FfxToolSource::Workspace, &subtool).is_none(),
            "Tool with no metadata should be None"
        );
    }

    #[test]
    fn check_invalid_metadata() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let name = "ffx-bad-metadata";
        let subtool = create_mock_subtool(tempdir.path(), name, Invalid("boom"));
        assert!(
            check_ffx_tool(FfxToolSource::Workspace, &subtool).is_none(),
            "Tool with bad metadata should be None"
        );
    }

    #[test]
    fn check_valid_metadata() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let name = "ffx-valid-metadata";
        let metadata = FhoToolMetadata::new("valid-metadata", "A tool with valid metadata!");
        let subtool = create_mock_subtool(tempdir.path(), name, Valid(metadata.clone()));
        let info = FfxToolInfo {
            source: FfxToolSource::Workspace,
            name: metadata.name.clone(),
            description: metadata.description.clone(),
            path: Some(subtool.clone()),
        };
        assert_eq!(
            check_ffx_tool(FfxToolSource::Workspace, &subtool),
            Some(info),
            "Tool with valid metadata should be what we put in"
        );
    }

    #[test]
    fn check_incorrect_name_metadata() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let name = "ffx-invalid-metadata";
        let metadata = FhoToolMetadata::new("not-the-right-name", "A tool with invalid metadata!");
        let subtool = create_mock_subtool(tempdir.path(), name, Valid(metadata.clone()));
        assert_eq!(
            check_ffx_tool(FfxToolSource::Workspace, &subtool),
            None,
            "Tool with invalid metadata should be None"
        );
    }

    #[test]
    fn check_future_fho_version_required() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let name = "ffx-invalid-metadata";
        let metadata = FhoToolMetadata {
            name: "invalid-metadata".to_owned(),
            description: "A tool with invalid metadata!".to_owned(),
            requires_fho: u16::MAX,
            fho_details: FhoDetails::FhoVersion0 { version: Only },
        };
        let subtool = create_mock_subtool(tempdir.path(), name, Valid(metadata.clone()));
        assert_eq!(
            check_ffx_tool(FfxToolSource::Workspace, &subtool),
            None,
            "Tool with invalid metadata should be None"
        );
    }

    #[fuchsia::test]
    async fn scan_workspace_subtool_directory() {
        let test_env = ffx_config::test_init().expect("test init");

        let tempdir = tempfile::tempdir().expect("tempdir");
        create_mock_subtool(
            tempdir.path(),
            "ffx-something",
            Valid(FhoToolMetadata::new("something", "something something something")),
        );
        create_mock_subtool(
            tempdir.path(),
            "ffx-something-else",
            Valid(FhoToolMetadata::new("something-else", "something something something else")),
        );
        create_mock_subtool(
            tempdir.path(),
            "ffx-whatever",
            Valid(FhoToolMetadata::new("whatever", "whatevs")),
        );
        create_mock_subtool(
            tempdir.path(),
            "ffx-orelse",
            Valid(FhoToolMetadata::new("orelse", "what")),
        );

        let suite =
            ExternalSubToolSuite::with_tools_from(test_env.context.clone(), &[tempdir.path()]);

        assert!(
            ExternalSubToolSuite::global_command_list().is_empty(),
            "no global commands for an external suite"
        );

        let basic_subtool_definition = FfxToolInfo {
            source: FfxToolSource::Workspace,
            name: "".to_string(),
            description: "".to_string(),
            path: None,
        };
        let expected_commands: HashSet<_> = HashSet::from_iter(
            [
                FfxToolInfo {
                    name: "something".to_owned(),
                    description: "something something something".to_owned(),
                    path: Some(tempdir.path().join("ffx-something")),
                    ..basic_subtool_definition
                },
                FfxToolInfo {
                    name: "something-else".to_owned(),
                    description: "something something something else".to_owned(),
                    path: Some(tempdir.path().join("ffx-something-else")),
                    ..basic_subtool_definition
                },
                FfxToolInfo {
                    name: "whatever".to_owned(),
                    description: "whatevs".to_owned(),
                    path: Some(tempdir.path().join("ffx-whatever")),
                    ..basic_subtool_definition
                },
                FfxToolInfo {
                    name: "orelse".to_owned(),
                    description: "what".to_owned(),
                    path: Some(tempdir.path().join("ffx-orelse")),
                    ..basic_subtool_definition
                },
            ]
            .into_iter(),
        );
        let found_commands = HashSet::from_iter(
            find_workspace_tools(&[tempdir.path()]).filter_map(|tool| tool.validate_tool()),
        );
        assert_eq!(found_commands, expected_commands, "subtools we created should exist");

        suite
            .try_from_args(&FfxCommandLine {
                command: vec!["ffx".to_owned()],
                ffx_args: vec![],
                global: Ffx { subcommand: vec!["whatever".to_owned()], ..Default::default() },
            })
            .await
            .expect("should be able to find mock subtool in suite");
    }

    #[fuchsia::test]
    fn test_with_tools_manifest() {
        let test_env = ffx_config::test_init().expect("test init");

        create_mock_subtool(
            test_env.isolate_root.path(),
            "ffx-sample1",
            Valid(FhoToolMetadata::new("sample1", "what")),
        );
        let subtool_manifest = test_env.isolate_root.path().join("subtools.json");
        let metadata = test_env.isolate_root.path().join("ffx-sample1.json");
        let executable = test_env.isolate_root.path().join("ffx-sample1");
        let contents = vec![
            SubToolManifestEntry {
                category: "internal".into(),
                executable: executable.clone(),
                executable_metadata: metadata.clone(),
                name: "ffx-sample1".into(),
            },
            SubToolManifestEntry {
                category: "internal".into(),
                executable: "/path/to/ffx-executable2".into(),
                executable_metadata: metadata.clone(),
                name: "ffx-ex2".into(),
            },
        ];
        fs::write(&subtool_manifest, serde_json::to_string(&contents).expect("serialized data"))
            .expect("subtool manifest written");

        let suite =
            ExternalSubToolSuite::with_tools_manifest(test_env.context.clone(), subtool_manifest);

        let ffx_cmd = FfxCommandLine {
            command: vec!["ffx".to_owned()],
            ffx_args: vec![],
            global: Ffx { subcommand: vec!["sample1".to_owned()], ..Default::default() },
        };
        let cmd = suite.find_workspace_tool(&ffx_cmd);

        assert!(cmd.is_some(), "Expected external command to be found");
        assert_eq!(cmd.unwrap().path, executable)
    }

    #[fuchsia::test]
    async fn test_get_args_info_with_no_environment() {
        use ffx_config::environment::ExecutableKind;
        use std::os::unix::fs::PermissionsExt;

        let test_env = ffx_config::test_init().expect("test init");
        let context =
            EnvironmentContext::no_context(ExecutableKind::Test, Default::default(), None, true)
                .unwrap();

        let subtool_path = test_env.isolate_root.path().join("ffx-sample1");
        let metadata_path = test_env.isolate_root.path().join("ffx-sample1.json");
        let script = r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = "--no-environment" ]; then
        echo '{"name":"sample1","description":"desc","examples":[],"flags":[],"notes":[],"commands":[],"positionals":[],"error_codes":[]}'
        exit 0
    fi
done
echo '{"type":"user","code":1,"message":"Paths error: cannot find home directory"}'
exit 1
"#;
        fs::write(&subtool_path, script).expect("write mock subtool");
        let mut perms = fs::metadata(&subtool_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&subtool_path, perms).unwrap();

        let metadata = FhoToolMetadata::new("sample1", "desc");
        let file = File::create(&metadata_path).expect("creating subtool metadata");
        serde_json::to_writer(file, &metadata).expect("Writing subtool metadata");

        let subtool_manifest = test_env.isolate_root.path().join("subtools.json");
        let contents = vec![SubToolManifestEntry {
            category: "internal".into(),
            executable: subtool_path.clone(),
            executable_metadata: metadata_path.clone(),
            name: "ffx-sample1".into(),
        }];
        fs::write(&subtool_manifest, serde_json::to_string(&contents).expect("serialized data"))
            .expect("subtool manifest written");

        let suite = ExternalSubToolSuite::with_tools_manifest(context, subtool_manifest);
        let args_info = suite.get_args_info().await.expect("get_args_info should succeed");
        assert_eq!(args_info.commands.len(), 1);
        assert_eq!(args_info.commands[0].name, "sample1");
    }

    #[fuchsia::test]
    async fn test_get_args_info_with_isolate_dir() {
        use std::os::unix::fs::PermissionsExt;

        let test_env = ffx_config::test_init().expect("test init");

        let subtool_path = test_env.isolate_root.path().join("ffx-sample1");
        let metadata_path = test_env.isolate_root.path().join("ffx-sample1.json");
        let script = r#"#!/bin/sh
prev=""
for arg in "$@"; do
    if [ "$prev" = "--isolate-dir" ]; then
        echo '{"name":"sample1","description":"desc","examples":[],"flags":[],"notes":[],"commands":[],"positionals":[],"error_codes":[]}'
        exit 0
    fi
    prev="$arg"
done
echo '{"type":"user","code":1,"message":"Missing --isolate-dir"}'
exit 1
"#;
        fs::write(&subtool_path, script).expect("write mock subtool");
        let mut perms = fs::metadata(&subtool_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&subtool_path, perms).unwrap();

        let metadata = FhoToolMetadata::new("sample1", "desc");
        let file = File::create(&metadata_path).expect("creating subtool metadata");
        serde_json::to_writer(file, &metadata).expect("Writing subtool metadata");

        let subtool_manifest = test_env.isolate_root.path().join("subtools.json");
        let contents = vec![SubToolManifestEntry {
            category: "internal".into(),
            executable: subtool_path.clone(),
            executable_metadata: metadata_path.clone(),
            name: "ffx-sample1".into(),
        }];
        fs::write(&subtool_manifest, serde_json::to_string(&contents).expect("serialized data"))
            .expect("subtool manifest written");

        let suite =
            ExternalSubToolSuite::with_tools_manifest(test_env.context.clone(), subtool_manifest);
        let args_info = suite.get_args_info().await.expect("get_args_info should succeed");
        assert_eq!(args_info.commands.len(), 1);
        assert_eq!(args_info.commands[0].name, "sample1");
    }

    #[fuchsia::test]
    async fn test_get_args_info_with_strict() {
        use ffx_config::environment::ExecutableKind;
        use std::os::unix::fs::PermissionsExt;

        let test_env = ffx_config::test_init().expect("test init");
        let context = EnvironmentContext::strict(ExecutableKind::Test, Default::default()).unwrap();

        let subtool_path = test_env.isolate_root.path().join("ffx-sample1");
        let metadata_path = test_env.isolate_root.path().join("ffx-sample1.json");
        let script = r#"#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = "--strict" ]; then
        echo '{"name":"sample1","description":"desc","examples":[],"flags":[],"notes":[],"commands":[],"positionals":[],"error_codes":[]}'
        exit 0
    fi
done
echo '{"type":"user","code":1,"message":"Missing --strict"}'
exit 1
"#;
        fs::write(&subtool_path, script).expect("write mock subtool");
        let mut perms = fs::metadata(&subtool_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&subtool_path, perms).unwrap();

        let metadata = FhoToolMetadata::new("sample1", "desc");
        let file = File::create(&metadata_path).expect("creating subtool metadata");
        serde_json::to_writer(file, &metadata).expect("Writing subtool metadata");

        let subtool_manifest = test_env.isolate_root.path().join("subtools.json");
        let contents = vec![SubToolManifestEntry {
            category: "internal".into(),
            executable: subtool_path.clone(),
            executable_metadata: metadata_path.clone(),
            name: "ffx-sample1".into(),
        }];
        fs::write(&subtool_manifest, serde_json::to_string(&contents).expect("serialized data"))
            .expect("subtool manifest written");

        let suite = ExternalSubToolSuite::with_tools_manifest(context, subtool_manifest);
        let args_info = suite.get_args_info().await.expect("get_args_info should succeed");
        assert_eq!(args_info.commands.len(), 1);
        assert_eq!(args_info.commands[0].name, "sample1");
    }

    #[fuchsia::test]
    async fn test_get_args_info_with_env() {
        use ffx_config::environment::ExecutableKind;
        use std::os::unix::fs::PermissionsExt;

        let test_env = ffx_config::test_init().expect("test init");
        let env_file = test_env.isolate_root.path().join("custom_env.json");
        let context = EnvironmentContext::no_context(
            ExecutableKind::Test,
            Default::default(),
            Some(env_file.clone()),
            false,
        )
        .unwrap();

        let subtool_path = test_env.isolate_root.path().join("ffx-sample1");
        let metadata_path = test_env.isolate_root.path().join("ffx-sample1.json");
        let script = r#"#!/bin/sh
prev=""
for arg in "$@"; do
    if [ "$prev" = "--env" ]; then
        echo '{"name":"sample1","description":"desc","examples":[],"flags":[],"notes":[],"commands":[],"positionals":[],"error_codes":[]}'
        exit 0
    fi
    prev="$arg"
done
echo '{"type":"user","code":1,"message":"Missing --env"}'
exit 1
"#;
        fs::write(&subtool_path, script).expect("write mock subtool");
        let mut perms = fs::metadata(&subtool_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&subtool_path, perms).unwrap();

        let metadata = FhoToolMetadata::new("sample1", "desc");
        let file = File::create(&metadata_path).expect("creating subtool metadata");
        serde_json::to_writer(file, &metadata).expect("Writing subtool metadata");

        let subtool_manifest = test_env.isolate_root.path().join("subtools.json");
        let contents = vec![SubToolManifestEntry {
            category: "internal".into(),
            executable: subtool_path.clone(),
            executable_metadata: metadata_path.clone(),
            name: "ffx-sample1".into(),
        }];
        fs::write(&subtool_manifest, serde_json::to_string(&contents).expect("serialized data"))
            .expect("subtool manifest written");

        let suite = ExternalSubToolSuite::with_tools_manifest(context, subtool_manifest);
        let args_info = suite.get_args_info().await.expect("get_args_info should succeed");
        assert_eq!(args_info.commands.len(), 1);
        assert_eq!(args_info.commands[0].name, "sample1");
    }

    #[test]
    fn subtool_config_none() {
        assert!(get_subtool_paths(vec![]).is_empty());
    }

    #[test]
    fn subtool_config_one() {
        assert_eq!(get_subtool_paths(vec![json!("boom")]), vec![PathBuf::from("boom")]);
    }

    #[test]
    fn subtool_config_multiple() {
        assert_eq!(
            get_subtool_paths(vec![json!("boom"), json!("zoom")]),
            vec![PathBuf::from("boom"), PathBuf::from("zoom")]
        );
    }

    #[test]
    fn subtool_config_listlist() {
        assert_eq!(
            get_subtool_paths(vec![json!(["boom", "zoom"])]),
            vec![PathBuf::from("boom"), PathBuf::from("zoom")]
        );
    }

    #[test]
    fn subtool_config_multiple_listlist() {
        assert_eq!(
            get_subtool_paths(vec![json!(["boom", "zoom"]), json!(["doom", "loom"])]),
            vec![
                PathBuf::from("boom"),
                PathBuf::from("zoom"),
                PathBuf::from("doom"),
                PathBuf::from("loom")
            ]
        );
    }

    #[test]
    fn subtool_config_multiple_different() {
        assert_eq!(
            get_subtool_paths(vec![json!("boom"), json!(["doom", "loom"])]),
            vec![PathBuf::from("boom"), PathBuf::from("doom"), PathBuf::from("loom")]
        );
        assert_eq!(
            get_subtool_paths(vec![json!(["boom", "zoom"]), json!("loom")]),
            vec![PathBuf::from("boom"), PathBuf::from("zoom"), PathBuf::from("loom")]
        );
    }
}
