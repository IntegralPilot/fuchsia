// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result};
use async_trait::async_trait;
use camino::Utf8Path;
use chrono::DateTime;
use chrono::offset::Utc;
use errors::ffx_bail;
use ffx_config::EnvironmentContext;
use ffx_config::environment::EnvironmentKind;
use ffx_repository_packages_args::{
    ExtractArchiveSubCommand, ListSubCommand, PackagesCommand, PackagesSubCommand, ShowSubCommand,
};
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, bug};
use fuchsia_hash::Hash;
use fuchsia_hyper::new_https_client;
use fuchsia_pkg::PackageArchiveBuilder;
use fuchsia_repo::repo_client::{PackageEntry, RepoClient};
use fuchsia_repo::repository::{PmRepository, RepoProvider, RepositorySpec};
use humansize::{WINDOWS, format_size};
use pkg::repo::repo_spec_to_backend;
use pkg::{PkgServerInstanceInfo as _, PkgServerInstances};
use prettytable::format::{FormatBuilder, TableFormat};
use prettytable::{Row, Table, cell, row};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufWriter, Cursor, Read};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const MAX_HASH: usize = 11;
const REPO_PATH_RELATIVE_TO_BUILD_DIR: &str = "amber-files";

type PackagesWriter = MachineWriter<Vec<PackagesOutput>>;

#[derive(FfxTool)]
#[target(None)]
pub struct PackagesTool {
    #[command]
    cmd: PackagesCommand,
    context: EnvironmentContext,
}

fho::embedded_plugin!(PackagesTool);

#[async_trait(?Send)]
impl FfxMain for PackagesTool {
    type Writer = PackagesWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        packages_impl(self.cmd, self.context, writer).await?;
        Ok(())
    }
}

#[derive(serde::Serialize)]
#[serde(untagged)]
pub enum PackagesOutput {
    Show(PackageEntry),
    List(RepositoryPackage),
}

#[derive(PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
pub struct RepositoryPackage {
    name: String,
    hash: String,
    size: Option<u64>,
    modified: Option<u64>,
    entries: Option<Vec<PackageEntry>>,
}

pub(crate) enum RepoIdArgs {
    Name(String, Option<u16>),
    Path(PathBuf),
    Default,
}

async fn packages_impl(
    cmd: PackagesCommand,
    context: EnvironmentContext,
    mut writer: PackagesWriter,
) -> Result<()> {
    match cmd.subcommand {
        PackagesSubCommand::List(subcmd) => list_impl(subcmd, context, &mut writer).await,
        PackagesSubCommand::Show(subcmd) => show_impl(subcmd, context, &mut writer).await,
        PackagesSubCommand::ExtractArchive(subcmd) => extract_archive_impl(subcmd, context).await,
    }
}

async fn show_impl(
    cmd: ShowSubCommand,
    context: EnvironmentContext,
    writer: &mut PackagesWriter,
) -> Result<()> {
    let repo_id = match (&cmd.repository, &cmd.repo_path) {
        (Some(name), None) => RepoIdArgs::Name(name.clone(), cmd.port.clone()),
        (None, Some(path)) => RepoIdArgs::Path(path.clone()),
        (None, None) => RepoIdArgs::Default,
        _ => anyhow::bail!(
            "Invalid options: Specify either `--repository` or `--repo-path`, not both."
        ),
    };

    let repo = connect_to_repo(repo_id, context).await.context("connect to repo")?;

    let Some(mut blobs) = repo
        .show_package(&cmd.package, cmd.include_subpackages)
        .await
        .with_context(|| format!("showing package {}", cmd.package))?
    else {
        ffx_bail!(
            "repository does not contain package {}. Try checking available packages with `ffx repository package list`.",
            cmd.package
        )
    };

    blobs.sort();

    if writer.is_machine() {
        let blobs = Vec::from_iter(blobs.into_iter().map(PackagesOutput::Show));
        writer.machine(&blobs).context("writing machine representation of blobs")?;
    } else {
        print_blob_table(&cmd, &blobs, writer).context("printing repository table")?
    }

    Ok(())
}

fn table_format() -> TableFormat {
    let padl = 0;
    let padr = 1;
    FormatBuilder::new().padding(padl, padr).build()
}

fn print_blob_table(
    cmd: &ShowSubCommand,
    blobs: &[PackageEntry],
    writer: &mut PackagesWriter,
) -> Result<()> {
    let mut table = Table::new();

    let mut header = row!("NAME", "SIZE", "HASH", "MODIFIED");
    if cmd.include_subpackages {
        header.insert_cell(1, cell!("SUBPACKAGE"));
    }
    table.set_titles(header);
    table.set_format(table_format());

    let mut rows = vec![];

    for blob in blobs {
        let mut row = row!(
            blob.path,
            blob.size.map(|s| format_size(s, WINDOWS)).unwrap_or_else(|| "<unknown>".to_string()),
            format_hash(&blob.hash.map(|hash| hash.to_string()), cmd.full_hash),
            blob.modified
                .and_then(|m| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(m)))
                .map(|m| DateTime::<Utc>::from(m).to_rfc2822())
                .unwrap_or_else(String::new)
        );
        if cmd.include_subpackages {
            row.insert_cell(1, cell!(blob.subpackage.as_deref().unwrap_or("<root>")));
        }
        rows.push(row);
    }

    for row in rows.into_iter() {
        table.add_row(row);
    }
    table.print(writer)?;

    Ok(())
}

async fn list_impl(
    cmd: ListSubCommand,
    context: EnvironmentContext,
    writer: &mut PackagesWriter,
) -> Result<()> {
    let repo_id = match (&cmd.repository, &cmd.repo_path) {
        (Some(name), None) => RepoIdArgs::Name(name.clone(), cmd.port.clone()),
        (None, Some(path)) => RepoIdArgs::Path(path.clone()),
        (None, None) => RepoIdArgs::Default,
        _ => anyhow::bail!(
            "Invalid options: Specify either `--repository` or `--repo-path`, not both."
        ),
    };
    let repo = connect_to_repo(repo_id, context).await.context("connect to repo")?;

    let mut packages = vec![];
    for package in repo.list_packages().await.context("listing packages")? {
        let mut package = RepositoryPackage {
            name: package.name,
            hash: package.hash.to_string(),
            size: None,
            modified: package.modified,
            entries: None,
        };

        if cmd.include_size || cmd.include_components {
            if let Some(entries) = repo.show_package(&package.name, cmd.include_size).await? {
                if cmd.include_size {
                    // Compute the total size of the unique blobs.
                    let mut blob_sizes = HashMap::new();
                    for entry in &entries {
                        if let (Some(hash), Some(size)) = (entry.hash, entry.size) {
                            blob_sizes.insert(hash, size);
                        }
                    }
                    package.size = Some(blob_sizes.values().sum());
                }

                if cmd.include_components {
                    package.entries = Some(
                        entries
                            .into_iter()
                            .filter(|entry| entry.subpackage.is_none())
                            .filter(|entry| entry.path.ends_with(".cm"))
                            .collect(),
                    );
                }
            }
        }

        packages.push(package);
    }

    packages.sort();

    if writer.is_machine() {
        let packages = Vec::from_iter(packages.into_iter().map(PackagesOutput::List));
        writer.machine(&packages).context("writing machine representation of packages")?;
    } else {
        print_package_table(&cmd, packages, writer).context("printing repository table")?
    }

    Ok(())
}

fn print_package_table(
    cmd: &ListSubCommand,
    packages: Vec<RepositoryPackage>,
    writer: &mut PackagesWriter,
) -> Result<()> {
    let mut table = Table::new();
    let mut header = row!("NAME", "HASH", "MODIFIED");
    if cmd.include_size {
        header.add_cell(cell!("SIZE"));
    }
    if cmd.include_components {
        header.add_cell(cell!("COMPONENTS"));
    }
    table.set_titles(header);
    table.set_format(table_format());

    let mut rows = vec![];

    for pkg in packages {
        let mut row = row!(
            pkg.name,
            format_hash(&Some(pkg.hash), cmd.full_hash),
            pkg.modified
                .and_then(|m| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(m)))
                .map(to_rfc2822)
                .unwrap_or_else(String::new)
        );
        if cmd.include_size {
            row.add_cell(cell!(
                pkg.size
                    .map(|s| format_size(s, WINDOWS))
                    .unwrap_or_else(|| "<unknown>".to_string())
            ));
        }
        if cmd.include_components {
            if let Some(entries) = pkg.entries {
                row.add_cell(cell!(
                    entries.into_iter().map(|entry| entry.path).collect::<Vec<_>>().join("\n")
                ));
            }
        }
        rows.push(row);
    }

    rows.sort_by_key(|r: &Row| r.get_cell(0).unwrap().get_content());
    for row in rows.into_iter() {
        table.add_row(row);
    }
    table.print(writer)?;

    Ok(())
}

fn format_hash(hash_value: &Option<String>, full: bool) -> String {
    if let Some(value) = hash_value {
        if full { value.to_string() } else { value[..MAX_HASH].to_string() }
    } else {
        "<unknown>".to_string()
    }
}

fn to_rfc2822(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc2822()
}

async fn connect_to_repo(
    repo_id: RepoIdArgs,
    context: EnvironmentContext,
) -> Result<RepoClient<Box<dyn RepoProvider>>> {
    // If we specified a repository on the CLI, try to look up its repository spec.
    let repo_spec = match repo_id {
        RepoIdArgs::Name(repo_name, repo_port) => {
            if let Some(spec) =
                repo_spec_from_running_server(&context, repo_name.clone(), repo_port)?
            {
                Some(spec)
            } else {
                ffx_bail!("No configuration found for {repo_name}")
            }
        }
        RepoIdArgs::Path(path_buf) => {
            let path_buf = Utf8Path::from_path(&path_buf)
                .with_context(|| format!("converting repo path to UTF-8 {:?}", path_buf))?;
            let metadata = path_buf.join("repository");
            if metadata.exists() {
                // Get the blobs
                let blobs = path_buf.join("blobs");
                if blobs.exists() {
                    Some(RepositorySpec::FileSystem {
                        metadata_repo_path: metadata,
                        blob_repo_path: blobs.into(),
                        aliases: BTreeSet::new(),
                    })
                } else {
                    Some(RepositorySpec::Pm { path: path_buf.into(), aliases: BTreeSet::new() })
                }
            } else {
                ffx_bail!("repo_path {path_buf:?} does not exist")
            }
        }
        RepoIdArgs::Default => {
            if let Some(repo_name) = pkg::config::get_default_repository(&context)? {
                // Otherwise, check if the default repository exists. Don't error out if
                // it doesn't.
                if let Some(spec) = repo_spec_from_running_server(&context, repo_name, None)? {
                    Some(spec)
                } else {
                    None
                }
            } else {
                None
            }
        }
    };

    // If we have a repository spec, create a backend for it. Otherwise we'll
    // see if we are in the fuchsia repository and try to use it.
    let backend = if let Some(repo_spec) = repo_spec {
        repo_spec_to_backend(&repo_spec, new_https_client())
            .with_context(|| format!("Creating a repo backend"))?
    } else if let EnvironmentKind::InTree { build_dir: Some(build_dir), .. } = context.env_kind() {
        let build_dir = Utf8Path::from_path(&build_dir)
            .with_context(|| format!("converting repo path to UTF-8 {:?}", build_dir))?;

        let backend =
            PmRepository::builder(build_dir.join(REPO_PATH_RELATIVE_TO_BUILD_DIR)).build();

        Box::new(backend)
    } else {
        ffx_bail!("No configuration found for the repository")
    };

    let mut repo = RepoClient::from_trusted_remote(backend)
        .await
        .with_context(|| format!("Connecting to repository"))?;

    // Make sure the repository is up to date.
    repo.update().await.with_context(|| format!("Updating repository"))?;

    Ok(repo)
}

fn repo_spec_from_running_server(
    context: &EnvironmentContext,
    repo_name: String,
    repo_port: Option<u16>,
) -> Result<Option<RepositorySpec>> {
    // Read repo instances
    let instance_root =
        context.get("repository.process_dir").map_err(|e: ffx_config::api::ConfigError| bug!(e))?;
    let mgr = PkgServerInstances::new(instance_root);
    if let Some(info) = mgr
        .get_instance(repo_name.clone(), repo_port)
        .with_context(|| format!("Getting server instance for {repo_name}"))?
    {
        Ok(Some(info.repo_spec()))
    } else {
        Ok(None)
    }
}

async fn extract_archive_impl(
    cmd: ExtractArchiveSubCommand,
    context: EnvironmentContext,
) -> Result<()> {
    let repo_id = match (&cmd.repository, &cmd.repo_path) {
        (Some(name), None) => RepoIdArgs::Name(name.clone(), cmd.port.clone()),
        (None, Some(path)) => RepoIdArgs::Path(path.clone()),
        (None, None) => RepoIdArgs::Default,
        _ => anyhow::bail!(
            "Invalid options: Specify either `--repository` or `--repo-path`, not both."
        ),
    };
    let repo = connect_to_repo(repo_id, context).await.context("connect to repo")?;

    let Some(entries) = repo
        .show_package(&cmd.package, true)
        .await
        .with_context(|| format!("showing package {}", cmd.package))?
    else {
        ffx_bail!(
            "repository does not contain package {}. Try checking available packages with `ffx repository package list`.",
            cmd.package
        )
    };

    let entry_is_meta_far =
        |entry: &&PackageEntry| entry.path == "meta.far" && entry.subpackage.is_none();

    let Some(meta_far_entry) = entries.iter().find(entry_is_meta_far) else {
        ffx_bail!("no meta.far entry in package {}", cmd.package)
    };

    let fetch_blob = |hash: Hash| {
        let repo = &repo;
        async move {
            let blob = repo
                .read_blob_decompressed(&hash)
                .await
                .with_context(|| format!("reading blob {hash}"))?;
            Ok::<(u64, Box<dyn Read>), anyhow::Error>((
                blob.len().try_into()?,
                Box::new(Cursor::new(blob)),
            ))
        }
    };

    let (size, content) =
        fetch_blob(meta_far_entry.hash.expect("meta.far entry should have hash")).await?;
    let mut archive_builder = PackageArchiveBuilder::with_meta_far(size, content);

    for entry in entries {
        if entry_is_meta_far(&&entry) {
            continue;
        }
        let Some(hash) = entry.hash else {
            // skip entries inside meta.far
            continue;
        };

        let (size, content) = fetch_blob(hash).await?;
        archive_builder.add_blob(hash, size, content);
    }

    let out_file = File::create(&cmd.out)
        .with_context(|| format!("creating package archive file {}", cmd.out.display()))?;
    archive_builder
        .build(BufWriter::new(out_file))
        .with_context(|| format!("writing package archive file {}", cmd.out.display()))?;
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use camino::Utf8PathBuf;

    use ffx_package_archive_utils::{ArchiveEntry, FarArchiveReader, read_file_entries};
    use ffx_writer::TestBuffers;
    use fidl_fuchsia_pkg_ext::{
        RepositoryConfigBuilder, RepositoryRegistrationAliasConflictMode, RepositoryStorageType,
    };
    use fuchsia_repo::test_utils;
    use pkg::PkgServerInfo;
    use pretty_assertions::assert_eq;
    use std::net::SocketAddr;
    use std::path::Path;

    const PKG1_HASH: &str = "2881455493b5870aaea36537d70a2adc635f516ac2092598f4b6056dabc6b25d";
    const PKG2_HASH: &str = "050907f009ff634f9aa57bff541fb9e9c2c62b587c23578e77637cda3bd69458";

    const PKG1_BIN_HASH: &str = "72e1e7a504f32edf4f23e7e8a3542c1d77d12541142261cfe272decfa75f542d";
    const PKG1_LIB_HASH: &str = "8a8a5f07f935a4e8e1fd1a1eda39da09bb2438ec0adfb149679ddd6e7e1fbb4f";

    async fn setup_repo<'a>(instance_name: &'a str, path: &'a Path) -> ffx_config::TestEnv {
        test_utils::make_pm_repo_dir(path).await;

        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let repo_process_dir = isolate_root.join("repo_data");

        let env = builder
            .user_config("repository.process_dir", repo_process_dir.to_string_lossy())
            .build()
            .unwrap();

        // Create a test repo
        let repo_config =
            RepositoryConfigBuilder::new(format!("fuchsia-pkg://{instance_name}").parse().unwrap())
                .build();

        let addr = SocketAddr::new(std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), 8000);
        let s1 = PkgServerInfo {
            name: instance_name.into(),
            address: addr,
            repo_spec: fuchsia_repo::repository::RepositorySpec::Pm {
                path: Utf8PathBuf::from_path_buf(path.into()).expect("utf-8 path"),
                aliases: BTreeSet::new(),
            },
            registration_storage_type: RepositoryStorageType::Ephemeral,
            registration_alias_conflict_mode: RepositoryRegistrationAliasConflictMode::ErrorOut,
            server_mode: pkg::ServerMode::Foreground,
            pid: std::process::id(),
            repo_config,
        };
        let mgr = PkgServerInstances::new(repo_process_dir.into());
        mgr.write_instance(&s1).expect("writing s1");

        env
    }

    async fn run_impl(cmd: ListSubCommand, context: EnvironmentContext) -> TestBuffers {
        let test_buffers = TestBuffers::default();
        let mut writer = PackagesWriter::new_test(None, &test_buffers);
        timeout::timeout(
            std::time::Duration::from_millis(1000),
            list_impl(cmd, context, &mut writer),
        )
        .await
        .unwrap()
        .unwrap();

        test_buffers
    }

    async fn run_impl_for_show_command(
        cmd: ShowSubCommand,
        context: EnvironmentContext,
    ) -> TestBuffers {
        let test_buffers = TestBuffers::default();
        let mut writer = PackagesWriter::new_test(None, &test_buffers);
        timeout::timeout(
            std::time::Duration::from_millis(1000),
            show_impl(cmd, context, &mut writer),
        )
        .await
        .unwrap()
        .unwrap();

        test_buffers
    }

    #[fuchsia::test]
    async fn test_package_list_repo_path() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo1", tmp.path()).await;

        let test_buffers = run_impl(
            ListSubCommand {
                repository: None,
                port: None,
                full_hash: false,
                include_components: false,
                include_size: false,
                repo_path: Some(tmp.path().into()),
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg2_hash = &PKG2_HASH[..MAX_HASH];
        let pkg2_path = blobs_path.join(PKG2_HASH);
        let pkg2_modified = to_rfc2822(std::fs::metadata(pkg2_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME       HASH        MODIFIED \n\
package1/0 {pkg1_hash} {pkg1_modified} \n\
package2/0 {pkg2_hash} {pkg2_modified} \n",
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_package_list_truncated_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo2", tmp.path()).await;

        let test_buffers = run_impl(
            ListSubCommand {
                repository: Some("test-repo2".to_string()),
                port: None,
                full_hash: false,
                include_components: false,
                include_size: false,
                repo_path: None,
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg2_hash = &PKG2_HASH[..MAX_HASH];
        let pkg2_path = blobs_path.join(PKG2_HASH);
        let pkg2_modified = to_rfc2822(std::fs::metadata(pkg2_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME       HASH        MODIFIED \n\
package1/0 {pkg1_hash} {pkg1_modified} \n\
package2/0 {pkg2_hash} {pkg2_modified} \n",
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_package_list_full_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo3", tmp.path()).await;

        let test_buffers = run_impl(
            ListSubCommand {
                repository: Some("test-repo3".to_string()),
                port: None,
                full_hash: true,
                include_components: false,
                include_size: false,
                repo_path: None,
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH;
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg2_hash = &PKG2_HASH;
        let pkg2_path = blobs_path.join(PKG2_HASH);
        let pkg2_modified = to_rfc2822(std::fs::metadata(pkg2_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME       HASH                                                             MODIFIED \n\
package1/0 {pkg1_hash} {pkg1_modified} \n\
package2/0 {pkg2_hash} {pkg2_modified} \n",
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_package_list_including_components() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo4", tmp.path()).await;

        let test_buffers = run_impl(
            ListSubCommand {
                repository: Some("test-repo4".to_string()),
                port: None,
                full_hash: false,
                include_components: true,
                include_size: false,
                repo_path: None,
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg2_hash = &PKG2_HASH[..MAX_HASH];
        let pkg2_path = blobs_path.join(PKG2_HASH);
        let pkg2_modified = to_rfc2822(std::fs::metadata(pkg2_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME       HASH        {:1$} COMPONENTS \n\
package1/0 {pkg1_hash} {pkg1_modified} meta/package1.cm \n\
package2/0 {pkg2_hash} {pkg2_modified} meta/package2.cm \n",
                "MODIFIED",
                pkg1_modified.len().max(pkg2_modified.len())
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_package_list_including_size() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo5", tmp.path()).await;

        let test_buffers = run_impl(
            ListSubCommand {
                repository: Some("test-repo5".to_string()),
                port: None,
                full_hash: false,
                include_components: false,
                include_size: true,
                repo_path: None,
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg2_hash = &PKG2_HASH[..MAX_HASH];
        let pkg2_path = blobs_path.join(PKG2_HASH);
        let pkg2_modified = to_rfc2822(std::fs::metadata(pkg2_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME       HASH        {:1$} SIZE \n\
package1/0 {pkg1_hash} {pkg1_modified} 24.03 kB \n\
package2/0 {pkg2_hash} {pkg2_modified} 24.03 kB \n",
                "MODIFIED",
                pkg1_modified.len().max(pkg2_modified.len())
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_show_package_truncated_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo6", tmp.path()).await;

        let test_buffers = run_impl_for_show_command(
            ShowSubCommand {
                repository: Some("test-repo6".to_string()),
                port: None,
                repo_path: None,
                full_hash: false,
                include_subpackages: false,
                package: "package1/0".to_string(),
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg1_bin_hash = &PKG1_BIN_HASH[..MAX_HASH];
        let pkg1_bin_path = blobs_path.join(PKG1_BIN_HASH);
        let pkg1_bin_modified =
            to_rfc2822(std::fs::metadata(pkg1_bin_path).unwrap().modified().unwrap());

        let pkg1_lib_hash = &PKG1_LIB_HASH[..MAX_HASH];
        let pkg1_lib_path = blobs_path.join(PKG1_LIB_HASH);
        let pkg1_lib_modified =
            to_rfc2822(std::fs::metadata(pkg1_lib_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME                          SIZE  HASH        MODIFIED \n\
bin/package1                  15 B  {pkg1_bin_hash} {pkg1_bin_modified} \n\
lib/package1                  12 B  {pkg1_lib_hash} {pkg1_lib_modified} \n\
meta.far                      24 kB {pkg1_hash} {pkg1_modified} \n\
meta/contents                 156 B <unknown>   {pkg1_modified} \n\
meta/fuchsia.abi/abi-revision 8 B   <unknown>   {pkg1_modified} \n\
meta/package                  33 B  <unknown>   {pkg1_modified} \n\
meta/package1.cm              11 B  <unknown>   {pkg1_modified} \n\
meta/package1.cmx             12 B  <unknown>   {pkg1_modified} \n"
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_show_package_full_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo7", tmp.path()).await;

        let test_buffers = run_impl_for_show_command(
            ShowSubCommand {
                repository: Some("test-repo7".to_string()),
                port: None,
                repo_path: None,
                full_hash: true,
                include_subpackages: false,
                package: "package1/0".to_string(),
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH;
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg1_bin_hash = &PKG1_BIN_HASH;
        let pkg1_bin_path = blobs_path.join(PKG1_BIN_HASH);
        let pkg1_bin_modified =
            to_rfc2822(std::fs::metadata(pkg1_bin_path).unwrap().modified().unwrap());

        let pkg1_lib_hash = &PKG1_LIB_HASH;
        let pkg1_lib_path = blobs_path.join(PKG1_LIB_HASH);
        let pkg1_lib_modified =
            to_rfc2822(std::fs::metadata(pkg1_lib_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();

        let expected = format!("\
NAME                          SIZE  HASH                                                             MODIFIED \n\
bin/package1                  15 B  {pkg1_bin_hash} {pkg1_bin_modified} \n\
lib/package1                  12 B  {pkg1_lib_hash} {pkg1_lib_modified} \n\
meta.far                      24 kB {pkg1_hash} {pkg1_modified} \n\
meta/contents                 156 B <unknown>                                                        {pkg1_modified} \n\
meta/fuchsia.abi/abi-revision 8 B   <unknown>                                                        {pkg1_modified} \n\
meta/package                  33 B  <unknown>                                                        {pkg1_modified} \n\
meta/package1.cm              11 B  <unknown>                                                        {pkg1_modified} \n\
meta/package1.cmx             12 B  <unknown>                                                        {pkg1_modified} \n"
        ).to_owned();
        assert_eq!(stdout, expected);

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_show_package_with_subpackages() {
        let tmp = tempfile::tempdir().unwrap();
        let env = setup_repo("test-repo8", tmp.path()).await;

        let test_buffers = run_impl_for_show_command(
            ShowSubCommand {
                repository: Some("test-repo8".to_string()),
                port: None,
                repo_path: None,
                full_hash: false,
                include_subpackages: true,
                package: "package1/0".to_string(),
            },
            env.context.clone(),
        )
        .await;

        let blobs_path = tmp.path().join("repository/blobs/1");

        let pkg1_hash = &PKG1_HASH[..MAX_HASH];
        let pkg1_path = blobs_path.join(PKG1_HASH);
        let pkg1_modified = to_rfc2822(std::fs::metadata(pkg1_path).unwrap().modified().unwrap());

        let pkg1_bin_hash = &PKG1_BIN_HASH[..MAX_HASH];
        let pkg1_bin_path = blobs_path.join(PKG1_BIN_HASH);
        let pkg1_bin_modified =
            to_rfc2822(std::fs::metadata(pkg1_bin_path).unwrap().modified().unwrap());

        let pkg1_lib_hash = &PKG1_LIB_HASH[..MAX_HASH];
        let pkg1_lib_path = blobs_path.join(PKG1_LIB_HASH);
        let pkg1_lib_modified =
            to_rfc2822(std::fs::metadata(pkg1_lib_path).unwrap().modified().unwrap());

        let (stdout, stderr) = test_buffers.into_strings();
        assert_eq!(
            stdout,
            format!(
                "\
NAME                          SUBPACKAGE SIZE  HASH        MODIFIED \n\
bin/package1                  <root>     15 B  {pkg1_bin_hash} {pkg1_bin_modified} \n\
lib/package1                  <root>     12 B  {pkg1_lib_hash} {pkg1_lib_modified} \n\
meta.far                      <root>     24 kB {pkg1_hash} {pkg1_modified} \n\
meta/contents                 <root>     156 B <unknown>   {pkg1_modified} \n\
meta/fuchsia.abi/abi-revision <root>     8 B   <unknown>   {pkg1_modified} \n\
meta/package                  <root>     33 B  <unknown>   {pkg1_modified} \n\
meta/package1.cm              <root>     11 B  <unknown>   {pkg1_modified} \n\
meta/package1.cmx             <root>     12 B  <unknown>   {pkg1_modified} \n"
            ),
        );

        assert_eq!(stderr, "");
    }

    #[fuchsia::test]
    async fn test_extract_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let repo_path = tmp.path().join("repo");
        let env = setup_repo("test-repo9", &repo_path).await;

        let archive_path = tmp.path().join("archive.far");
        extract_archive_impl(
            ExtractArchiveSubCommand {
                out: archive_path.clone(),
                repository: Some("test-repo9".to_string()),
                port: None,
                repo_path: None,
                package: "package1/0".to_string(),
            },
            env.context.clone(),
        )
        .await
        .unwrap();

        let mut archive_reader = FarArchiveReader::new(&archive_path).unwrap();
        let mut entries = read_file_entries(&mut archive_reader).unwrap();
        entries.sort();

        assert_eq!(
            entries,
            vec![
                ArchiveEntry {
                    name: "bin/package1".to_string(),
                    path: "72e1e7a504f32edf4f23e7e8a3542c1d77d12541142261cfe272decfa75f542d"
                        .to_string(),
                    length: Some(15),
                },
                ArchiveEntry {
                    name: "lib/package1".to_string(),
                    path: "8a8a5f07f935a4e8e1fd1a1eda39da09bb2438ec0adfb149679ddd6e7e1fbb4f"
                        .to_string(),
                    length: Some(12),
                },
                ArchiveEntry {
                    name: "meta.far".to_string(),
                    path: "meta.far".to_string(),
                    length: Some(24576),
                },
                ArchiveEntry {
                    name: "meta/contents".to_string(),
                    path: "meta/contents".to_string(),
                    length: Some(156),
                },
                ArchiveEntry {
                    name: "meta/fuchsia.abi/abi-revision".to_string(),
                    path: "meta/fuchsia.abi/abi-revision".to_string(),
                    length: Some(8),
                },
                ArchiveEntry {
                    name: "meta/package".to_string(),
                    path: "meta/package".to_string(),
                    length: Some(33),
                },
                ArchiveEntry {
                    name: "meta/package1.cm".to_string(),
                    path: "meta/package1.cm".to_string(),
                    length: Some(11),
                },
                ArchiveEntry {
                    name: "meta/package1.cmx".to_string(),
                    path: "meta/package1.cmx".to_string(),
                    length: Some(12),
                },
            ]
        );
    }
}
