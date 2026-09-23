// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use errors::{ffx_bail, ffx_bail_with_code};
use ffx_config::api::ConfigError;
use ffx_config::{
    Config, ConfigLevel, EnvironmentContext, disable_metrics, enable_basic_metrics,
    enable_enhanced_metrics, print_config, show_metrics_status,
};
use ffx_config_plugin_args::{
    AddCommand, AnalyticsCommand, AnalyticsControlCommand, ConfigCommand, EnvAccessCommand,
    EnvCommand, EnvSetCommand, GetCommand, MappingMode, RemoveCommand, SetCommand, SshKeyCommand,
    SubCommand,
};
use ffx_ssh::{SshKeyErrorKind, SshKeyFiles};
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::io::Write;

#[derive(FfxTool)]
#[target(None)]
pub struct ConfigTool {
    #[command]
    config: ConfigCommand,
    ctx: EnvironmentContext,
}

fho::embedded_plugin!(ConfigTool);

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConfigToolMessage {
    Message(String),
    Data(Value),
}

impl std::fmt::Display for ConfigToolMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Message(message) => message.to_owned(),
            Self::Data(data) => format!("{}", data),
        };
        write!(f, "{}", message)
    }
}

#[async_trait::async_trait(?Send)]
impl FfxMain for ConfigTool {
    type Writer = VerifiedMachineWriter<ConfigToolMessage>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        match &self.config.sub {
            SubCommand::CheckSshKeys(check_ssh_cmd) => {
                exec_check_ssh_keys(&self.ctx, check_ssh_cmd, &mut writer).await
            }
            SubCommand::Env(env) => exec_env(&self.ctx, env, writer).await,
            SubCommand::Get(get_cmd) => exec_get(&self.ctx, get_cmd, writer),
            SubCommand::Set(set_cmd) => exec_set(&self.ctx, set_cmd).await,
            SubCommand::Remove(remove_cmd) => exec_remove(&self.ctx, remove_cmd).await,
            SubCommand::Add(add_cmd) => exec_add(&self.ctx, add_cmd).await,
            SubCommand::Analytics(analytics_cmd) => exec_analytics(analytics_cmd).await,
        }
        .map_err(fho::Error::from)
    }
}

fn output<W: Write>(mut writer: W, value: Option<Value>) -> Result<()> {
    match value {
        Some(v) => writeln!(writer, "{}", serde_json::to_string_pretty(&v).unwrap())
            .map_err(|e| anyhow!("{}", e)),
        // Use 2 error code so wrapper scripts don't need check for the string to differentiate
        // errors.
        None => ffx_bail_with_code!(2, "Value not found"),
    }
}

fn output_array<W: Write>(
    mut writer: W,
    values: std::result::Result<Vec<Value>, ConfigError>,
) -> Result<()> {
    match values {
        Ok(v) => {
            if v.len() == 1 {
                writeln!(writer, "{}", serde_json::to_string_pretty(&v[0]).unwrap())
                    .map_err(|e| anyhow!("{}", e))
            } else {
                writeln!(writer, "{}", serde_json::to_string_pretty(&Value::Array(v)).unwrap())
                    .map_err(|e| anyhow!("{}", e))
            }
        }
        // Use 2 error code so wrapper scripts don't need check for the string to differentiate
        // errors.
        Err(_) => ffx_bail_with_code!(2, "Value not found"),
    }
}

fn output_first_element<W: Write>(mut writer: W, value: Option<Value>) -> Result<()> {
    match value {
        Some(Value::Array(vals)) => {
            if !vals.is_empty() {
                writeln!(writer, "{}", serde_json::to_string_pretty(&vals[0]).unwrap())
                    .map_err(|e| anyhow!("{}", e))
            } else {
                ffx_bail_with_code!(2, "Value not found")
            }
        }
        Some(v) => writeln!(writer, "{}", serde_json::to_string_pretty(&v).unwrap())
            .map_err(|e| anyhow!("{}", e)),
        // Use 2 error code so wrapper scripts don't need check for the string to differentiate
        // errors.
        None => ffx_bail_with_code!(2, "Value not found"),
    }
}

fn exec_get<W: Write>(ctx: &EnvironmentContext, get_cmd: &GetCommand, writer: W) -> Result<()> {
    match get_cmd.name.as_ref() {
        Some(_) => match get_cmd.process {
            MappingMode::Raw => {
                let value: Option<Value> = get_cmd.query(ctx).get_raw(ctx)?;
                output(writer, value)
            }
            MappingMode::Substitute => {
                let value: std::result::Result<Vec<Value>, _> = get_cmd.query(ctx).get(ctx);
                output_array(writer, value)
            }
            MappingMode::File => {
                let value = get_cmd.query(ctx).get_file(ctx)?;
                output_first_element(writer, value)
            }
        },
        None => print_config(ctx, writer).map_err(anyhow::Error::from),
    }
}

async fn exec_set(ctx: &EnvironmentContext, set_cmd: &SetCommand) -> Result<()> {
    log::debug!("Set command running...");
    let query = set_cmd.query(ctx);
    let (key, level) = query.validate_write_query()?;
    let mut env = ctx.load()?;
    env.populate_defaults(&level)?;
    let mut config = Config::from_env(&env)?;
    config.set(key, level, set_cmd.value.clone())?;
    config.save()?;
    Ok(())
}

async fn exec_remove(ctx: &EnvironmentContext, remove_cmd: &RemoveCommand) -> Result<()> {
    let query = remove_cmd.query(ctx);
    let (key, level) = query.validate_write_query()?;
    // Check that there is a value before removing it.
    if let Ok(Some(_val)) = query.get_raw::<Option<Value>>(ctx) {
        let env = ctx.load()?;
        let mut config = Config::from_env(&env)?;
        config.remove(key, level)?;
        config.save()?;
        Ok(())
    } else {
        ffx_bail_with_code!(
            2,
            "Configuration key not found. Check your spelling, or try listing available keys with `ffx config get`."
        )
    }
}

async fn exec_add(ctx: &EnvironmentContext, add_cmd: &AddCommand) -> Result<()> {
    let query = add_cmd.query(ctx);
    let (key, level) = query.validate_write_query()?;
    let mut env = ctx.load()?;
    env.populate_defaults(&level)?;
    let mut config = Config::from_env(&env)?;

    let value = Value::String(format!("{}", add_cmd.value));

    config.add(key, level, value)?;

    config.save()?;
    Ok(())
}

async fn exec_env_set<W: Write>(
    env_context: &EnvironmentContext,
    mut writer: W,
    s: &EnvSetCommand,
) -> Result<()> {
    let env_file = env_context.env_file_path().context("Getting ffx environment file path")?;

    if !env_file.exists() {
        writeln!(writer, "\"{}\" does not exist, creating empty json file", env_file.display())?;
        let mut file = File::create(&env_file).context("opening write buffer")?;
        file.write_all(b"{}").context("writing configuration file")?;
        if !env_context.env_kind().is_isolated() {
            file.sync_all().context("syncing configuration file to filesystem")?;
        }
    }

    // Double check read/write permissions and create the file if it doesn't exist.
    let _ = OpenOptions::new().read(true).write(true).create(true).open(&s.file)?;

    let mut env = env_context.load().context("Loading environment file")?;

    match &s.level {
        ConfigLevel::User => env.set_user(Some(&s.file)),
        ConfigLevel::Build => env.set_build(&s.file)?,
        ConfigLevel::Global => env.set_global(Some(&s.file)),
        _ => ffx_bail!("This configuration is not stored in the environment."),
    }
    env.save().map_err(anyhow::Error::from)
}

async fn exec_env<W: Write>(
    ctx: &EnvironmentContext,
    env_command: &EnvCommand,
    mut writer: W,
) -> Result<()> {
    match &env_command.access {
        Some(a) => match a {
            EnvAccessCommand::Set(s) => exec_env_set(ctx, writer, s).await,
            EnvAccessCommand::Get(g) => {
                writeln!(
                    writer,
                    "{}",
                    ctx.load().context("Loading environment file")?.display(&g.level)
                )?;
                Ok(())
            }
        },
        None => {
            writeln!(writer, "{}", ctx.load().context("Loading environment file")?.display(&None))?;
            Ok(())
        }
    }
}

async fn exec_analytics(analytics_cmd: &AnalyticsCommand) -> Result<()> {
    let writer = Box::new(std::io::stdout());
    match &analytics_cmd.sub {
        AnalyticsControlCommand::EnableEnhanced(_) => {
            enable_enhanced_metrics().await.with_context(|| "Failed to enable metrics")?;
            show_metrics_status(writer).await.with_context(|| "Failed to read metrics state")?
        }
        AnalyticsControlCommand::Enable(_) => {
            enable_basic_metrics().await.with_context(|| "Failed to enable metrics")?;
            show_metrics_status(writer).await.with_context(|| "Failed to read metrics state")?
        }
        AnalyticsControlCommand::Disable(_) => {
            disable_metrics().await.with_context(|| "Failed to disable metrics")?;
            show_metrics_status(writer).await.with_context(|| "Failed to read metrics state")?
        }
        AnalyticsControlCommand::Show(_) => {
            show_metrics_status(writer).await.with_context(|| "Failed to read metrics state")?
        }
    }
    Ok(())
}

async fn exec_check_ssh_keys(
    ctx: &EnvironmentContext,
    check_ssh_command: &SshKeyCommand,
    writer: &mut VerifiedMachineWriter<ConfigToolMessage>,
) -> Result<()> {
    match SshKeyFiles::load(&ctx) {
        Ok(ssh_files) => {
            if check_ssh_command.create {
                let _ = ssh_files.create_keys_if_needed(false);
            }
            match ssh_files.check_keys(false) {
                Ok(message) => {
                    writer.item(&ConfigToolMessage::Message(message))?;
                }
                Err(e) => match e.kind {
                    SshKeyErrorKind::BadKeyType => writer.item(&ConfigToolMessage::Message(
                        format!("SSH keys type not supported: {}", e.message),
                    ))?,
                    SshKeyErrorKind::BadConfiguration => writer.item(
                        &ConfigToolMessage::Message(format!("SSH keys configuration problem: {e}")),
                    )?,
                    SshKeyErrorKind::FileNotFound if !check_ssh_command.create => {
                        writer.item(&ConfigToolMessage::Message(format!(
                            "{}. Check configuration or run `ffx config check-ssh-keys --create`.",
                            e.message
                        )))?
                    }
                    _ => writer.item(&ConfigToolMessage::Message(format!(
                        "{}. Check configuration or run `ffx doctor --repair-keys`.",
                        e.message
                    )))?,
                },
            };
        }
        Err(e) => {
            writer.item(&ConfigToolMessage::Message(format!("Could not get SSH key paths {e}")))?;
        }
    };
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use std::{env, fs};

    use super::*;
    use errors::{FfxError, IntoExitCode};
    use ffx_config::{SelectMode, test_env};
    use ffx_writer::{Format, TestBuffers};
    use serde_json::json;

    #[fuchsia::test]
    async fn test_exec_env_set_set_values() -> Result<()> {
        let test_env = ffx_config::test_env().build()?;
        let writer = Vec::<u8>::new();
        let cmd = EnvSetCommand { file: "test.json".into(), level: ConfigLevel::User };
        exec_env_set(&test_env.context, writer, &cmd).await?;
        assert_eq!(cmd.file, test_env.load().get_user().unwrap());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_gey_key() {
        let test_env =
            test_env().user_config("some-key", "a value").build().expect("test env initialized");

        let get_cmd = GetCommand {
            name: Some("some-key".into()),
            process: MappingMode::Substitute,
            select: ffx_config::SelectMode::First,
        };

        let mut writer = Vec::<u8>::new();
        exec_get(&test_env.context, &get_cmd, &mut writer).expect("getting value");
        assert_eq!(String::from_utf8(writer).unwrap(), "\"a value\"\n".to_string());
    }

    #[fuchsia::test]
    async fn test_remove_key() {
        let mut test_env = ffx_config::test_env().build().expect("test env initialized");
        let mut config = Config::from_env(&test_env.load()).unwrap();
        config.set("some-key", ConfigLevel::User, "a value".into()).unwrap();
        config.save().unwrap();

        test_env.reload_context().unwrap();

        let remove_cmd = RemoveCommand { name: "some-key".into() };

        let get_cmd = GetCommand {
            name: Some("some-key".into()),
            process: MappingMode::Substitute,
            select: ffx_config::SelectMode::First,
        };

        exec_remove(&test_env.context, &remove_cmd).await.expect("remove");

        test_env.reload_context().unwrap();

        let mut writer = Vec::<u8>::new();
        match exec_get(&test_env.context, &get_cmd, &mut writer) {
            Ok(_) => panic!("Expected error getting removed key"),
            Err(e) => assert_eq!(e.to_string(), "Value not found"),
        };
    }

    #[fuchsia::test]
    async fn test_remove_nonexistant_key() {
        let test_env = ffx_config::test_env().build().expect("test env initialized");

        let remove_cmd = RemoveCommand { name: "some-key".into() };

        match exec_remove(&test_env.context, &remove_cmd).await {
            Ok(_) => panic!("Expected error getting removed key"),
            Err(e) => {
                if let Some(ffx_err) = e.downcast_ref::<FfxError>() {
                    assert_eq!(
                        ffx_err.to_string(),
                        "Configuration key not found. Check your spelling, or try listing available keys with `ffx config get`."
                    );
                    assert!(ffx_err.exit_code() != 0, "Expected non-zero exit code");
                } else {
                }
            }
        };
    }

    #[fuchsia::test]
    async fn test_list_processed_by_raw() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path1 = isolate_root.join("privatekey1");
        let private_path2 = isolate_root.join("privatekey2");
        fs::write(&private_path1, "path1").expect("key 1 written");
        fs::write(&private_path2, "path2").expect("key 2 written");
        let test_env = builder
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path1.to_string_lossy(),
                    private_path2.to_string_lossy(),
                ]),
            )
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::Raw,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        let want = serde_json::to_string_pretty(&json!([
            "$ENV_PATH_THAT_IS_NOT_SET_2",
            private_path1,
            private_path2
        ]))
        .expect("json output");
        assert_eq!(got, format!("{}\n", want));
    }

    #[fuchsia::test]
    async fn test_list_processed_by_substitute() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path1 = isolate_root.join("privatekey1");
        let private_path2 = isolate_root.join("privatekey2");
        fs::write(&private_path1, "path1").expect("key 1 written");
        fs::write(&private_path2, "path2").expect("key 2 written");
        let test_env = builder
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path1.to_string_lossy(),
                    private_path2.to_string_lossy(),
                ]),
            )
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::Substitute,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        let want = serde_json::to_string_pretty(&json!([private_path1, private_path2]))
            .expect("json output");
        assert_eq!(got, format!("{}\n", want));
    }

    #[fuchsia::test]
    async fn test_list_processed_by_substitute_with_env() {
        // Set the env before the test env
        // assert the key is not already in the environment
        assert!(
            env::var("ENV_SSH_PATH_FOR_TESTING_").is_err(),
            "Expected weird testing env variable to be unset"
        );

        let mut builder = test_env().env_var("ENV_SSH_PATH_FOR_TESTING_", "private_path1");
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path1 = isolate_root.join("privatekey1");
        let private_path2 = isolate_root.join("privatekey2");
        fs::write(&private_path1, "path1").expect("key 1 written");
        fs::write(&private_path2, "path2").expect("key 2 written");

        let test_env = builder
            .user_config(
                "ssh.priv",
                json!(["$ENV_SSH_PATH_FOR_TESTING_", private_path2.to_string_lossy(),]),
            )
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::Substitute,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        let want = serde_json::to_string_pretty(&json!(["private_path1", private_path2]))
            .expect("json output");
        assert_eq!(got, format!("{}\n", want));
    }

    #[fuchsia::test]
    async fn test_list_single_by_file() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path1 = isolate_root.join("privatekey1");
        fs::write(&private_path1, "path1").expect("key 1 written");

        let test_env = builder
            .user_config("ssh.priv", json!([private_path1.to_string_lossy(),]))
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::File,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        assert_eq!(got, format!("{}\n", json!(private_path1)));
    }

    #[fuchsia::test]
    async fn test_list_processed_by_file() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path1 = isolate_root.join("privatekey1");
        let private_path2 = isolate_root.join("privatekey2");
        fs::write(&private_path1, "path1").expect("key 1 written");
        fs::write(&private_path2, "path2").expect("key 2 written");
        let test_env = builder
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path1.to_string_lossy(),
                    private_path2.to_string_lossy(),
                ]),
            )
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::File,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        assert_eq!(got, format!("{}\n", json!(private_path1)));
    }

    #[fuchsia::test]
    async fn test_list_processed_by_file_with_env() {
        let private_path1 = tempfile::NamedTempFile::new().expect("temp file 1");
        // Set the env before the test env
        // assert the key is not already in the environment
        assert!(
            env::var("ENV_SSH_PATH_FOR_TESTING_2").is_err(),
            "Expected weird testing env variable to be unset"
        );

        let mut builder = test_env()
            .env_var("ENV_SSH_PATH_FOR_TESTING_2", private_path1.path().to_str().unwrap());
        let isolate_root = builder.isolate_root();
        let mut writer = Vec::<u8>::new();

        let private_path2 = isolate_root.join("privatekey2");
        fs::write(&private_path2, "path2").expect("key 2 written");

        let test_env = builder
            .user_config(
                "ssh.priv",
                json!(["$ENV_SSH_PATH_FOR_TESTING_2", private_path2.to_string_lossy(),]),
            )
            .build()
            .expect("test env");

        exec_get(
            &test_env.context,
            &GetCommand {
                name: Some("ssh.priv".into()),
                process: MappingMode::File,
                select: SelectMode::First,
            },
            &mut writer,
        )
        .expect("exec_get");
        let got = String::from_utf8_lossy(&writer);
        assert_eq!(got, format!("{}\n", json!(private_path1.path())));
    }

    #[fuchsia::test]
    async fn test_exec_check_mismatched_ssh_keys() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();

        let auth_key_path1 = isolate_root.join("authorized_keys1");
        let private_path1 = isolate_root.join("privatekey1");

        let auth_key_path2 = isolate_root.join("authorized_keys2");
        let private_path2 = isolate_root.join("privatekey2");

        let test_env = builder
            .user_config(
                "ssh.pub",
                json!(["$ENV_PATH_THAT_IS_NOT_SET", auth_key_path2.to_string_lossy(), "someother"]),
            )
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path1.to_string_lossy(),
                    "someother/place"
                ]),
            )
            .build()
            .expect("test env");

        let keys = SshKeyFiles {
            authorized_keys: auth_key_path1.clone(),
            private_key: private_path1.clone(),
        };
        keys.create_keys_if_needed(false).expect("Initializing keys");

        let other_keys = SshKeyFiles {
            authorized_keys: auth_key_path2.clone(),
            private_key: private_path2.clone(),
        };
        other_keys.create_keys_if_needed(false).expect("Initializing other keys");

        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: false }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = buffers.into_stdout_str();

        let expected_message = format!("{}\n",
            serde_json::to_string(
                &ConfigToolMessage::Message(
                    format!("Could not find matching public key for the private key {}. Check configuration or run `ffx doctor --repair-keys`.", private_path1.to_string_lossy())
                )
            ).expect("Should be a string")
        );
        assert_eq!(expected_message, output);

        // Explicitly repair using SshKeyFiles::load(&test_env.context)
        let loaded_keys = SshKeyFiles::load(&test_env.context).expect("load keys");
        loaded_keys.create_keys_if_needed(true).expect("repair keys");

        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: false }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        let output = buffers.into_stdout_str();
        assert!(result.is_ok());

        let expected_message = format!(
            "{}\n",
            serde_json::to_string(&ConfigToolMessage::Message(format!(
                "SSH Public/Private keys match"
            )))
            .expect("Should be a string")
        );

        assert_eq!(expected_message, output);
    }

    #[fuchsia::test]
    async fn test_exec_check_ok_ssh_keys() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();

        let auth_key_path = isolate_root.join("authorized_keys");
        let private_path = isolate_root.join("privatekey");

        let test_env = builder
            .user_config(
                "ssh.pub",
                json!(["$ENV_PATH_THAT_IS_NOT_SET", auth_key_path.to_string_lossy(), "someother"]),
            )
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path.to_string_lossy(),
                    "someother/place"
                ]),
            )
            .build()
            .expect("test env");

        let keys = SshKeyFiles::load(&test_env.context).expect("new ssh keys");

        keys.create_keys_if_needed(false).expect("Initializing keys");

        assert_eq!(
            keys.authorized_keys.display().to_string(),
            auth_key_path.to_string_lossy().to_string()
        );
        assert_eq!(
            keys.private_key.display().to_string(),
            private_path.to_string_lossy().to_string()
        );
        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: false }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());
        let expected_message = format!(
            "{}\n",
            serde_json::to_string(&ConfigToolMessage::Message(
                "SSH Public/Private keys match".to_string()
            ))
            .expect("Should be a string")
        );

        let output = buffers.into_stdout_str();
        assert_eq!(expected_message, output);
    }

    #[fuchsia::test]
    async fn test_exec_check_empty_ssh_keys() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();

        let auth_key_path = isolate_root.join("authorized_keys");
        let private_path = isolate_root.join("privatekey");

        let test_env = builder
            .user_config(
                "ssh.pub",
                json!(["$ENV_PATH_THAT_IS_NOT_SET", auth_key_path.to_string_lossy(), "someother"]),
            )
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path.to_string_lossy(),
                    "someother/place"
                ]),
            )
            .build()
            .expect("test env");

        let keys = SshKeyFiles::load(&test_env.context).expect("new ssh keys");

        assert_eq!(
            keys.authorized_keys.display().to_string(),
            auth_key_path.to_string_lossy().to_string()
        );
        assert_eq!(
            keys.private_key.display().to_string(),
            private_path.to_string_lossy().to_string()
        );

        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: false }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = buffers.into_stdout_str();
        let expected_message = format!(
            "{}\n",
            serde_json::to_string(&ConfigToolMessage::Message(format!(
                "Private key {} does not exist. Check configuration or run `ffx config check-ssh-keys --create`.",
                private_path.to_string_lossy()
            ),))
            .expect("Should be a string")
        );
        assert_eq!(expected_message, output);
    }

    #[fuchsia::test]
    async fn test_exec_check_empty_ssh_keys_with_create() {
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();

        let auth_key_path = isolate_root.join("authorized_keys");
        let private_path = isolate_root.join("privatekey");

        let test_env = builder
            .user_config(
                "ssh.pub",
                json!(["$ENV_PATH_THAT_IS_NOT_SET", auth_key_path.to_string_lossy(), "someother"]),
            )
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path.to_string_lossy(),
                    "someother/place"
                ]),
            )
            .build()
            .expect("test env");

        let keys = SshKeyFiles::load(&test_env.context).expect("new ssh keys");

        assert_eq!(
            keys.authorized_keys.display().to_string(),
            auth_key_path.to_string_lossy().to_string()
        );
        assert_eq!(
            keys.private_key.display().to_string(),
            private_path.to_string_lossy().to_string()
        );

        assert!(!auth_key_path.exists());
        assert!(!private_path.exists());

        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: true }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        assert!(auth_key_path.exists());
        assert!(private_path.exists());

        let output = buffers.into_stdout_str();
        let expected_message = format!(
            "{}\n",
            serde_json::to_string(&ConfigToolMessage::Message(format!(
                "SSH Public/Private keys match"
            ),))
            .expect("Should be a string")
        );
        assert_eq!(expected_message, output);
    }

    #[fuchsia::test]
    async fn test_exec_check_bad_permissions_with_create() {
        use std::os::unix::fs::PermissionsExt;
        let mut builder = test_env();
        let isolate_root = builder.isolate_root();

        let auth_key_path = isolate_root.join("authorized_keys");
        let private_path = isolate_root.join("privatekey");

        // Touch the file with 0o644 permissions
        std::fs::write(&private_path, b"").expect("create file");
        std::fs::set_permissions(&private_path, std::fs::Permissions::from_mode(0o644))
            .expect("set permissions");

        let test_env = builder
            .user_config(
                "ssh.pub",
                json!(["$ENV_PATH_THAT_IS_NOT_SET", auth_key_path.to_string_lossy(), "someother"]),
            )
            .user_config(
                "ssh.priv",
                json!([
                    "$ENV_PATH_THAT_IS_NOT_SET_2",
                    private_path.to_string_lossy(),
                    "someother/place"
                ]),
            )
            .build()
            .expect("test env");

        let cmd = ConfigCommand { sub: SubCommand::CheckSshKeys(SshKeyCommand { create: true }) };
        let tool = ConfigTool { config: cmd, ctx: test_env.context.clone() };
        let buffers = TestBuffers::default();
        let writer = <ConfigTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        let result = tool.main(writer).await;
        assert!(result.is_ok());

        let output = buffers.into_stdout_str();
        let expected_message = format!(
            "{}\n",
            serde_json::to_string(&ConfigToolMessage::Message(format!(
                "Private key {} has the wrong file permissions. SSH requires 0o600, found 0o100644. Check configuration or run `ffx doctor --repair-keys`.",
                private_path.display()
            ),))
            .expect("Should be a string")
        );
        assert_eq!(expected_message, output);
    }
}
