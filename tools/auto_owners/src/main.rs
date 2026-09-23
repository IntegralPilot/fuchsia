// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, anyhow, bail};
use argh::FromArgs;
use camino::{Utf8Path, Utf8PathBuf};
use gnaw_lib::CrateOutputMetadata;
use simplelog::{Config, LevelFilter, SimpleLogger};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use walkdir::WalkDir;
use xml::reader::{EventReader, XmlEvent};

/// update OWNERS files for external Rust code
///
/// This tool relies on GN metadata produced from a maximal "kitchen sink" build. When run
/// outside the context of `fx update-3p-owners`, it also relies on being run after
/// `fx update-rustc-third-party`.
#[derive(FromArgs)]
struct Options {
    /// path to the JSON metadata produced by cargo-gnaw
    #[argh(option)]
    rust_metadata: Option<Utf8PathBuf>,

    /// path to the 3P integration manifest
    #[argh(option)]
    integration_manifest: Option<Utf8PathBuf>,

    /// file that contains newline-separated list of OWNERS files that can be
    /// added to generated OWNERS.
    #[argh(option)]
    filter: Option<Utf8PathBuf>,

    /// path to the ownership overrides config file
    #[argh(option)]
    overrides: Utf8PathBuf,

    /// path to the source directory
    #[argh(option)]
    fuchsia_dir: Utf8PathBuf,

    /// path to the project.json file created by `gn gen --ide=json`
    #[argh(option)]
    project_json: Utf8PathBuf,

    /// generate OWNERS only for the given projects path. Can be repeated.
    #[argh(option, long = "path")]
    project_paths: Vec<String>,

    /// don't updated existing OWNERS files
    #[argh(switch)]
    skip_existing: bool,

    /// print the generated content without updating OWNERS files
    #[argh(switch)]
    dry_run: bool,

    #[argh(switch)]
    /// automatically generate OWNERS files from reverse dependencies.
    generate_owners_from_reverse_deps: bool,
}

fn main() -> Result<()> {
    SimpleLogger::init(LevelFilter::Info, Config::default()).unwrap_or_else(|err| {
        eprintln!("WARN: failed to initialize logger: {err:#?}");
    });
    let options = argh::from_env();

    OwnersDb::new(options)?.update_all_files()
}

#[derive(Debug, PartialEq)]
struct ProjectMetadata {
    /// name of the project
    pub name: String,

    /// filesystem path to the project
    pub path: Utf8PathBuf,

    /// list of GN targets for depending on the project
    pub targets: Vec<String>,
}

#[derive(PartialEq)]
enum UpdateStrategy {
    /// update all the OWNERS files
    AllFiles,

    /// only add OWNERS files where missing, leaving the existing OWNERS files unchanged
    OnlyMissing,
}

struct OwnersDb {
    /// metadata about projects
    projects: Vec<ProjectMetadata>,

    /// cache of OWNERS file paths, indexed by target name. Holds the targets for which the
    /// corresponding OWNERS file path is known; cached to avoid probing the filesystem
    owners_path_cache: BTreeMap<String, Utf8PathBuf>,

    /// path to the JSON metadata produced by cargo-gnaw
    rust_metadata: Option<Utf8PathBuf>,

    /// explicit lists of OWNERS files to include instead of inferring, indexed by project name
    overrides: BTreeMap<String, Vec<Utf8PathBuf>>,

    /// if set, print update results instead of updating OWNERS file
    dry_run: bool,

    /// automatically generate OWNERS files from reverse dependencies.
    generate_owners_from_reverse_deps: bool,

    update_strategy: UpdateStrategy,

    fuchsia_dir: Utf8PathBuf,

    gn_graph: gn_graph::Graph,

    /// The set of all potential OWNERS files we may include in any generated
    /// OWNERS file.
    filter: Option<HashSet<Utf8PathBuf>>,

    /// Create a map from source file to GN target.
    source_deps: HashMap<String, String>,
}

impl OwnersDb {
    fn new(options: Options) -> Result<Self> {
        let Options {
            rust_metadata,
            integration_manifest,
            overrides,
            fuchsia_dir,
            project_json,
            project_paths,
            filter,
            skip_existing,
            dry_run,
            generate_owners_from_reverse_deps,
        } = options;

        let update_strategy = match skip_existing {
            true => UpdateStrategy::OnlyMissing,
            false => UpdateStrategy::AllFiles,
        };

        let rust_crates = if let Some(rust_metadata) = &rust_metadata {
            let mut rust_crates: Vec<CrateOutputMetadata> = serde_json::from_reader(
                File::open(rust_metadata).with_context(|| format!("opening {}", rust_metadata))?,
            )
            .with_context(|| format!("parsing {}", rust_metadata))?;

            for metadata in &mut rust_crates {
                // Make the path relative to the fuchsia directory.
                if let Ok(path) = metadata.path.strip_prefix(&fuchsia_dir) {
                    metadata.path = path.into();
                }
            }

            rust_crates
        } else {
            vec![]
        };

        let gn_targets = gn_json::parse_file(project_json)?;
        let gn_graph = gn_graph::Graph::create_from(gn_targets)?;

        // OWNERS path is currently only cached for rust projects.
        let mut owners_path_cache = rust_crates
            .iter()
            .map(|metadata| (metadata.canonical_target.clone(), metadata.path.clone()))
            .collect::<BTreeMap<_, _>>();

        owners_path_cache.extend(rust_crates.iter().filter_map(|metadata| {
            metadata.shortcut_target.as_ref().map(|t| (t.clone(), metadata.path.clone()))
        }));

        let rust_projects = rust_crates.into_iter().map(|metadata| ProjectMetadata {
            name: metadata.name,
            path: metadata.path,
            targets: toolchain_suffixed_targets(
                &metadata.canonical_target,
                metadata.shortcut_target.as_ref().map(String::as_str),
            ),
        });

        let integration_projects = integration_manifest
            .map(|manifest| {
                parse_integration_manifest(&gn_graph, &manifest)
                    .with_context(|| format!("parsing {}", manifest))
            })
            .transpose()?
            .unwrap_or_default();

        let path_projects =
            project_paths.iter().map(|path| parse_path(&gn_graph, path)).collect::<Vec<_>>();

        let projects =
            rust_projects.chain(integration_projects).chain(path_projects).collect::<Vec<_>>();

        let overrides: BTreeMap<String, Vec<Utf8PathBuf>> = toml::de::from_str(
            &std::fs::read_to_string(&overrides)
                .with_context(|| format!("reading {}", overrides))?,
        )
        .with_context(|| format!("parsing {}", overrides))?;

        let filter = if let Some(filter) = &filter { Some(parse_filter(filter)?) } else { None };

        // The gn graph doesn't track source dependencies, so build up our own
        // lookup table.
        let source_deps = gn_graph
            .targets()
            .iter()
            .map(|(label, desc)| {
                desc.description.sources.iter().map(|source| (source.clone(), label.clone()))
            })
            .flatten()
            .collect::<HashMap<_, _>>();

        Ok(Self {
            projects,
            owners_path_cache,
            rust_metadata,
            overrides,
            update_strategy,
            dry_run,
            fuchsia_dir,
            gn_graph,
            filter,
            source_deps,
            generate_owners_from_reverse_deps,
        })
    }

    /// Update all OWNERS files for all projects.
    fn update_all_files(&self) -> Result<()> {
        log::info!("Updating OWNERS files...");

        for metadata in &self.projects {
            if !metadata.path.starts_with("third_party/rust_crates/mirrors") {
                self.update_owners_file(&metadata, &mut std::io::stdout())
                    .with_context(|| format!("updating {:?}", metadata))?;
            }
        }

        log::info!("Done!");

        Ok(())
    }

    /// Update the OWNERS file for a single 3p project.
    fn update_owners_file<W: Write>(
        &self,
        metadata: &ProjectMetadata,
        output_buffer: &mut W,
    ) -> Result<()> {
        if self.update_strategy == UpdateStrategy::OnlyMissing {
            if let Some(owners_path) =
                find_owners_including_secondary(&self.fuchsia_dir, &metadata.path)
            {
                log::info!("{} has OWNERS file at {}, skipping", metadata.path, owners_path);
                return Ok(());
            }
        }

        if let Some(file) = self
            .compute_owners_file(metadata)
            .with_context(|| format!("computing owners for {}", metadata.path))?
        {
            let owners_path = self.fuchsia_dir.join(&metadata.path).join("OWNERS");

            if self.dry_run {
                log::info!("Dry-run: generated {} with content:\n", owners_path);
                output_buffer.write_all(file.to_string().as_bytes())?;
            } else {
                // We need to write every OWNERS file, even if it would be empty,
                // because the other OWNERS files may include the empty ones.
                std::fs::write(&owners_path, file.to_string().as_bytes())
                    .with_context(|| format!("writing {owners_path}"))?;
            }
        }

        Ok(())
    }

    fn compute_owners_file(&self, metadata: &ProjectMetadata) -> Result<Option<OwnersFile>> {
        let override_key = if metadata.path.starts_with("third_party/rust_crates") {
            // Trims something like -0.24.11, otherwise the override list would have to be updated
            // every time the version is bumped.
            metadata
                .path
                .to_owned()
                .into_string()
                .trim_end_matches(char::is_numeric)
                .trim_end_matches('.')
                .trim_end_matches(char::is_numeric)
                .trim_end_matches('.')
                .trim_end_matches(char::is_numeric)
                .trim_end_matches('-')
                .to_string()
        } else {
            metadata.path.to_owned().into_string()
        };

        if let Some(owners_overrides) = self.overrides.get(&override_key) {
            Ok(Some(OwnersFile {
                path: metadata.path.join("OWNERS"),
                includes: owners_overrides.iter().map(Clone::clone).collect(),
                source: OwnersSource::Override,
            }))
        } else if self.generate_owners_from_reverse_deps {
            Ok(Some(self.owners_files_from_reverse_deps(&metadata)?))
        } else {
            Ok(None)
        }
    }

    /// Run `gn desc` for the project's GN target(s) and find the OWNERS files that correspond to its
    /// reverse deps.
    ///
    /// For Rust projects, cargo-gnaw metadata encodes version-unambiguous GN targets like
    /// `//third_party/rust_crates:foo-v1_0_0` but we discourage the use of those targets
    /// throughout the tree. To find dependencies from in-house code we need to also get reverse
    /// deps for the equivalent target without the version, e.g. `//third_party/rust_crates:foo`.
    fn owners_files_from_reverse_deps(&self, metadata: &ProjectMetadata) -> Result<OwnersFile> {
        let mut deps = metadata
            .targets
            .iter()
            .map(|target| self.reverse_deps(target))
            .flatten()
            .collect::<BTreeSet<String>>();

        // non-rust projects are sometimes referenced by file. If no deps were found, search for
        // references to any of the files in the project.
        if deps.is_empty() && !metadata.path.starts_with("third_party/rust_crates") {
            log::info!(
                "{} has no target references, searching for all file references",
                metadata.path
            );

            let mut targets = vec![];
            for entry in WalkDir::new(&self.fuchsia_dir.join(&metadata.path)) {
                let path = Utf8PathBuf::try_from(entry?.into_path())?;

                if path.is_file() {
                    let path = if let Ok(path) = path.strip_prefix(&self.fuchsia_dir) {
                        path.into()
                    } else {
                        path
                    };

                    targets.push(path);
                }
            }

            deps = targets
                .iter()
                .map(|target| self.source_deps.get(&format!("//{target}")))
                .flatten()
                .cloned()
                .collect::<BTreeSet<String>>();
        }

        let mut includes = BTreeSet::new();
        for dep in &deps {
            if let Some(included) = self.owners_file_for_gn_target(&*dep)? {
                if should_include(&included) {
                    includes.insert(included);
                }
            }
        }

        let includes =
            includes.into_iter().filter(|i| !metadata.path.starts_with(i.parent().unwrap()));

        // Optionally remove files from `includes` if they are not in the filter list.
        let includes = if let Some(filter) = &self.filter {
            includes.filter(|i| filter.contains(i)).collect()
        } else {
            includes.collect()
        };

        Ok(OwnersFile {
            path: metadata.path.join("OWNERS"),
            includes,
            source: OwnersSource::ReverseDependencies { targets: metadata.targets.clone(), deps },
        })
    }

    /// Find all the dependencies for the GN target `target` and return a list of GN targets which
    /// depend on the target.
    fn reverse_deps(&self, target: &str) -> BTreeSet<String> {
        if let Some(refs) = self.gn_graph.targets_dependent_on(target) {
            refs.into_iter().map(|s| s.to_owned()).collect()
        } else {
            // the target exists in the filesystem but isn't in the existing build graph
            BTreeSet::new()
        }
    }

    /// Given a GN target, find the most likely path for its corresponding OWNERS file.
    fn owners_file_for_gn_target(&self, target: &str) -> Result<Option<Utf8PathBuf>> {
        // none of the metadata we have emits toolchain suffices, so remove them. the target
        // toolchain is the default toolchain so we don't encounter an targets suffixed that way
        let target = if let Some(idx) = target.find(GN_TOOLCHAIN_SUFFIX_PREFIX) {
            target.split_at(idx).0
        } else {
            target
        };

        if target.starts_with(RUST_EXTERNAL_TARGET_PREFIX) && self.rust_metadata.is_some() {
            // if the target is for a 3p crate it might not have an owners file yet, so we don't
            // want to rely on probing the filesystem. instead we'll construct a path *a priori*
            return Ok(if let Some(path) = self.owners_path_cache.get(target) {
                Some(path.join("OWNERS"))
            } else {
                log::warn!(
                    "{} not in {}",
                    target,
                    self.rust_metadata.as_ref().expect("metadata is set")
                );
                None
            });
        }

        // the target is outside of the 3p directory, so we need to probe for the closest file
        let no_slashes =
            target.strip_prefix("//").expect("GN targets from refs should be absolute");

        // remove the target name after the colon
        let path_portion = no_slashes.rsplitn(2, ":").skip(1).next().unwrap();

        // Search up the directories for an OWNERS file. All paths are
        // relative to `fuchsia_dir`, so we need to prepend it in order to
        // see if it exists.
        for dir in Utf8Path::new(path_portion).ancestors() {
            let path = dir.join("OWNERS");
            if self.fuchsia_dir.join(&path).exists() {
                return Ok(Some(path));
            }
        }

        panic!("we will always find an OWNERS file in the source tree");
    }
}

fn parse_filter(filter_path: &Utf8Path) -> Result<HashSet<Utf8PathBuf>> {
    let filter_file = File::open(filter_path).with_context(|| format!("opening {filter_path}"))?;

    let mut files = HashSet::new();
    for line in BufReader::new(filter_file).lines() {
        let line = line?;
        if line.ends_with("OWNERS") {
            files.insert(line.into());
        } else {
            bail!("each line must end with `OWNERS`, not {line}")
        }
    }

    Ok(files)
}

const THIRD_PARTY: &str = "third_party";
const BUILD_SECONDARY_THIRD_PARTY: &str = "build/secondary/third_party";

fn find_owners_including_secondary(
    fuchsia_dir: &Utf8Path,
    project_path: &Utf8PathBuf,
) -> Option<Utf8PathBuf> {
    let owners_path = project_path.join("OWNERS");
    if fuchsia_dir.join(&owners_path).exists() {
        return Some(owners_path);
    }

    // Rust projects have a well established directory structure, so if no owners were found in the
    // project path, don't look for owners up the path.
    if project_path.starts_with("third_party/rust_crates") {
        return None;
    }

    // Some third-party projects share owners up the path. Look for owners up to one level down
    // from //third_party, i.e //third_party/<project-root>.
    if project_path.starts_with(THIRD_PARTY) {
        let owners_path = find_owners_up_to_dir_with_parent(fuchsia_dir, project_path, THIRD_PARTY);

        if owners_path.is_some() {
            return owners_path;
        }
        // Some third-party projects have their OWNERS files placed in //build/secondary/.
        let build_secondary_path = Utf8Path::new("build/secondary").join(project_path);
        let owners_path = find_owners_up_to_dir_with_parent(
            fuchsia_dir,
            &build_secondary_path,
            BUILD_SECONDARY_THIRD_PARTY,
        );

        if owners_path.is_some() {
            return owners_path;
        }
    }
    // Some third-party projects are in //build/secondary.
    if project_path.starts_with(BUILD_SECONDARY_THIRD_PARTY) {
        return find_owners_up_to_dir_with_parent(
            fuchsia_dir,
            project_path,
            BUILD_SECONDARY_THIRD_PARTY,
        );
    }

    None
}

// Search for an OWNERS file up the directory structure until reaching the directory whose parent
// is `parent_path`.
fn find_owners_up_to_dir_with_parent(
    fuchsia_dir: &Utf8Path,
    project_path: &Utf8Path,
    parent_path: &str,
) -> Option<Utf8PathBuf> {
    if !project_path.starts_with(parent_path) {
        return None;
    }

    let owners_path = project_path.join("OWNERS");
    if fuchsia_dir.join(&owners_path).exists() {
        return Some(owners_path);
    }

    if let Some(parent) = project_path.parent() {
        if parent == parent_path {
            // If the parent is `parent_path`, stop searching.
            return None;
        }
        return find_owners_up_to_dir_with_parent(fuchsia_dir, &parent.to_path_buf(), parent_path);
    }
    None
}

/// Fully qualified GN targets have a toolchain suffix like `//foo:bar(//path/to/toolchain:target)`.
/// We need to remove these suffices from targets when looking them up in our JSON metadata because
/// cargo-gnaw doesn't emit toolchains in its metadata.
///
/// Fuchsia's toolchains are by convention all currently defined under `//build/toolchain`.
const GN_TOOLCHAIN_SUFFIX_PREFIX: &str = "(//build/toolchain";

/// Prefix found on all generated GN targets for 3p crates.
const RUST_EXTERNAL_TARGET_PREFIX: &str = "//third_party/rust_crates:";

fn toolchain_suffixed_targets(versioned: &str, top_level: Option<&str>) -> Vec<String> {
    let mut targets = vec![];
    add_all_toolchain_suffices(versioned, &mut targets);
    top_level.map(|t| add_all_toolchain_suffices(t, &mut targets));
    targets
}

fn add_all_toolchain_suffices(target: &str, targets: &mut Vec<String>) {
    // TODO(https://fxbug.dev/42153062) support querying explicitly for both linux and mac
    // TODO(https://fxbug.dev/42150695) support querying explicitly for both x64 and arm64
    #[cfg(target_arch = "x86_64")]
    const HOST_ARCH_SUFFIX: &str = "x64";
    #[cfg(target_arch = "aarch64")]
    const HOST_ARCH_SUFFIX: &str = "arm64";

    // without a suffix, default toolchain is target
    targets.push(target.to_string());
    // we can only query for linux on a linux host and for mac on a mac
    targets.push(format!("{}(//build/toolchain:host_{})", target, HOST_ARCH_SUFFIX));
    targets.push(format!("{}(//build/toolchain:unknown_wasm32)", target));
}

#[derive(Debug)]
enum OwnersSource {
    /// file is computed from reverse deps and they are listed here
    ReverseDependencies {
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        targets: Vec<String>,
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        deps: BTreeSet<String>,
    },
    /// file is computed from overrides in //third_party/owners.toml
    Override,
}

impl OwnersSource {
    fn is_computed(&self) -> bool {
        matches!(self, OwnersSource::ReverseDependencies { .. })
    }
}

#[derive(Debug)]
struct OwnersFile {
    // TODO(https://fxbug.dev/42165549)
    #[allow(unused)]
    path: Utf8PathBuf,
    includes: BTreeSet<Utf8PathBuf>,
    source: OwnersSource,
}

const AUTOGENERATED_HEADER: &str = "# AUTOGENERATED FROM DEPENDENT BUILD TARGETS.";
const HEADER: &str = "\
# TO MAKE CHANGES HERE, UPDATE //third_party/owners.toml.
# DOCS: https://fuchsia.dev/reference/tools/fx/cmd/update-3p-owners
";

impl std::fmt::Display for OwnersFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.source.is_computed() {
            writeln!(f, "{}", AUTOGENERATED_HEADER)?;
        }

        writeln!(f, "{}", HEADER)?;
        for to_include in &self.includes {
            write!(f, "include /{}\n", to_include)?;
        }
        Ok(())
    }
}

fn should_include(owners_file: &Utf8Path) -> bool {
    let owners_file = owners_file.as_os_str().to_str().unwrap();
    // many of these repos aren't open
    !owners_file.starts_with("vendor") &&
    // we don't ever need to include the root OWNERS file
    owners_file != "OWNERS"
}

fn parse_integration_manifest(
    gn_graph: &gn_graph::Graph,
    manifest_path: &Utf8Path,
) -> Result<Vec<ProjectMetadata>> {
    let parser = EventReader::new(BufReader::new(
        File::open(manifest_path).with_context(|| format!("opening {manifest_path}"))?,
    ));

    parser
        .into_iter()
        .filter(|e| {
            matches!(
                e,
                Ok(XmlEvent::StartElement { name, .. })
                    if name.local_name == *"project"
            )
        })
        .map(|e| match e {
            Ok(XmlEvent::StartElement { attributes, .. }) => {
                let name = &attributes
                    .iter()
                    .find(|&a| a.name.local_name == "name")
                    .ok_or_else(|| anyhow!("no name attribute"))?
                    .value;

                let path = &attributes
                    .iter()
                    .find(|&a| a.name.local_name == "path")
                    .ok_or_else(|| anyhow!("no path attribute"))?
                    .value
                    .trim_end_matches("/src");

                Ok(ProjectMetadata {
                    name: name.to_string(),
                    path: Utf8PathBuf::from(path),
                    // the project can be referred by any of its inner targets.
                    targets: get_targets_in_directory(gn_graph, path),
                })
            }
            _ => bail!("unreachable"),
        })
        .collect()
}

fn parse_path(gn_graph: &gn_graph::Graph, path: &str) -> ProjectMetadata {
    let path = path.trim_end_matches('/');

    ProjectMetadata {
        name: path.to_string(),
        path: Utf8PathBuf::from(path.to_string()),
        targets: get_targets_in_directory(gn_graph, path),
    }
}

fn get_targets_in_directory(gn_graph: &gn_graph::Graph, path: &str) -> Vec<String> {
    let prefixes = [format!("//{path}/"), format!("//{path}:")];

    let mut targets = vec![];
    for (target, _) in gn_graph.targets().range(format!("//{path}")..) {
        if prefixes.iter().any(|prefix| target.starts_with(prefix)) {
            targets.push(target.clone());
        } else {
            break;
        }
    }

    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use pretty_assertions::assert_eq;
    use serial_test::serial;
    use std::process::{Command, Stdio};

    fn read_owners(dir: &Utf8Path) -> BTreeMap<String, String> {
        let mut owners: BTreeMap<String, String> = BTreeMap::new();
        for entry in WalkDir::new(dir) {
            let entry = entry.unwrap();
            if entry.path().ends_with("OWNERS") {
                let contents = std::fs::read_to_string(entry.path()).unwrap();
                let path = entry.path().strip_prefix(&dir).unwrap().to_str().unwrap();
                owners.insert(path.to_string(), contents);
            }
        }

        owners
    }

    #[test]
    fn parse_integration_manifest_projects() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        let project_json = test_dir_path.join("out/project.json");
        let gn_targets = gn_json::parse_file(project_json).unwrap();
        let gn_graph = gn_graph::Graph::create_from(gn_targets).unwrap();

        let projects =
            parse_integration_manifest(&gn_graph, &test_dir_path.join("manifest")).unwrap();

        assert_eq!(
            projects
                .into_iter()
                .map(|ProjectMetadata { name: _, path, targets: _ }| path
                    .as_os_str()
                    .to_str()
                    .unwrap()
                    .to_string())
                .collect::<Vec<String>>(),
            ["third_party/foo", "third_party/bar"]
        );
    }

    #[test]
    fn parse_path_projects() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        let project_json = test_dir_path.join("out/project.json");
        let gn_targets = gn_json::parse_file(project_json).unwrap();
        let gn_graph = gn_graph::Graph::create_from(gn_targets).unwrap();

        assert_eq!(
            parse_path(&gn_graph, "third_party/bar/"),
            ProjectMetadata {
                name: "third_party/bar".into(),
                path: "third_party/bar".into(),
                targets: vec!["//third_party/bar:bar".into()],
            }
        );
    }

    #[test]
    fn override_owners() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));
        assert!(!expected_owner_files.contains_key("third_party/rust_crates/foo/OWNERS"));
        assert!(
            !expected_owner_files
                .contains_key("third_party/rust_crates/with-dashes-and-version-0.1.0/OWNERS")
        );

        assert!(
            OwnersDb::new(Options {
                rust_metadata: Some(PATHS.test_base_dir.join("owners/rust_metadata.json")),
                integration_manifest: None,
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: false,
                project_paths: vec![],
                dry_run: false,
                generate_owners_from_reverse_deps: true,
            })
            .expect("valid OwnersDb")
            .update_all_files()
            .is_ok()
        );

        // Only these paths should be created.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/bar/OWNERS".into(),
            format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/foo/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/with-dashes-and-version-0.1.0/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // check that the OWNERS file was generated.
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    #[test]
    fn update_outdated_owners() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));
        assert!(!expected_owner_files.contains_key("third_party/rust_crates/foo/OWNERS"));
        assert!(
            !expected_owner_files
                .contains_key("third_party/rust_crates/with-dashes-and-version-0.1.0/OWNERS")
        );

        assert!(
            OwnersDb::new(Options {
                rust_metadata: Some(PATHS.test_base_dir.join("owners/rust_metadata.json")),
                integration_manifest: None,
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: false,
                project_paths: vec![],
                dry_run: false,
                generate_owners_from_reverse_deps: true,
            })
            .expect("valid OwnersDb")
            .update_all_files()
            .is_ok()
        );

        // Only these paths should be created.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/bar/OWNERS".into(),
            format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/foo/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/with-dashes-and-version-0.1.0/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // check that the OWNERS file was generated
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    #[test]
    fn generate_missing() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));
        assert!(!expected_owner_files.contains_key("third_party/rust_crates/foo/OWNERS"));
        assert!(
            !expected_owner_files
                .contains_key("third_party/rust_crates/with-dashes-and-version-0.1.0/OWNERS")
        );

        assert_matches!(
            OwnersDb::new(Options {
                rust_metadata: Some(PATHS.test_base_dir.join("owners/rust_metadata.json")),
                integration_manifest: Some(PATHS.test_base_dir.join("owners/manifest")),
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: true,
                project_paths: vec![],
                dry_run: false,
                generate_owners_from_reverse_deps: true,
            })
            .expect("valid OwnersDb")
            .update_all_files(),
            Ok(())
        );

        // Only these paths should be created.
        expected_owner_files.insert(
            "third_party/bar/OWNERS".into(),
            format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/foo/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/with-dashes-and-version-0.1.0/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // check that the OWNERS file was generated
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    #[test]
    #[serial] // these tests mutate the current process' working directory
    fn dry_run() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // check that the project doesn't have an OWNERS file
        assert!(!test_dir_path.join("third_party/bar/OWNERS").exists());

        let owners_db = OwnersDb::new(Options {
            rust_metadata: None,
            integration_manifest: Some(PATHS.test_base_dir.join("owners/manifest")),
            filter: Some(test_dir_path.join("filter-files")),
            overrides: PATHS.test_base_dir.join("owners/owners.toml"),
            project_json: test_dir_path.join("out/project.json"),
            fuchsia_dir: test_dir_path.to_path_buf(),
            skip_existing: false,
            project_paths: vec![],
            dry_run: true,
            generate_owners_from_reverse_deps: true,
        })
        .expect("OwnersDb is valid");

        let unowned_metadata = owners_db
            .projects
            .iter()
            .find(|m| m.name.contains("bar"))
            .expect("should contain project metadata");

        let mut output_buffer = vec![];
        assert!(owners_db.update_owners_file(&unowned_metadata, &mut output_buffer).is_ok());

        // check that an OWNERS file was not created for the project
        assert!(!test_dir_path.join("third_party/bar/OWNERS").exists());

        // check that the dry-run output is correct
        let output = String::from_utf8(output_buffer).unwrap();
        assert_eq!(output, format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"));
    }

    #[test]
    fn update_owners_for_path() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));
        assert!(!expected_owner_files.contains_key("third_party/rust_crates/foo/OWNERS"));

        assert_matches!(
            OwnersDb::new(Options {
                rust_metadata: None,
                integration_manifest: None,
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: false,
                project_paths: vec!["third_party/bar/".to_string()],
                dry_run: false,
                generate_owners_from_reverse_deps: true,
            })
            .expect("valid OwnersDb")
            .update_all_files(),
            Ok(())
        );

        // Only this path should be created.
        expected_owner_files.insert(
            "third_party/bar/OWNERS".into(),
            format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"),
        );

        // check that the OWNERS file was generated.
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    #[test]
    fn search_for_file_refs() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));

        // the 'baz' project is depended on by file, not gn target.
        assert!(
            OwnersDb::new(Options {
                rust_metadata: None,
                integration_manifest: None,
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: false,
                project_paths: vec!["third_party/baz/".to_string()],
                dry_run: false,
                generate_owners_from_reverse_deps: true,
            })
            .expect("valid OwnersDb")
            .update_all_files()
            .is_ok()
        );

        // Only these paths should be created.
        expected_owner_files.insert(
            "third_party/baz/OWNERS".into(),
            format!("{AUTOGENERATED_HEADER}\n{HEADER}\ninclude /dep/OWNERS\n"),
        );

        // check that the OWNERS file was generated.
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    #[test]
    #[serial] // these tests mutate the current process' working directory
    fn find_owners() {
        let test_dir = setup_test_dir("owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // The OWNERS file for the child project is in its parent directory.
        assert_eq!(
            Some(Utf8Path::new("third_party/foo/OWNERS").to_path_buf()),
            find_owners_including_secondary(
                &test_dir_path,
                &Utf8Path::new("third_party/foo/child").to_path_buf()
            ),
        );
        // The OWNERS file for this project is in the alternative location //build/secondary.
        assert_eq!(
            Some(Utf8Path::new("build/secondary/third_party/baz/foo/OWNERS").to_path_buf()),
            find_owners_including_secondary(
                &test_dir_path,
                &Utf8Path::new("third_party/baz/foo").to_path_buf()
            ),
        );
    }

    fn setup_test_dir(test_subdir: &str) -> tempfile::TempDir {
        let original_test_dir = PATHS.test_base_dir.join(test_subdir);
        let test_dir = tempfile::tempdir().unwrap();
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();
        let out_dir = test_dir_path.join("out");

        copy_contents(&original_test_dir, &test_dir_path);
        copy_contents(&PATHS.test_base_dir.join("common"), &test_dir_path);

        // Write all the files into `filter-files`.
        let mut filter_file = File::create(test_dir_path.join("filter-files")).unwrap();
        for entry in walkdir::WalkDir::new(&test_dir_path) {
            let entry = entry.unwrap();
            let path =
                Utf8Path::from_path(entry.path().strip_prefix(&test_dir_path).unwrap()).unwrap();
            if path.ends_with("OWNERS") {
                writeln!(filter_file, "{}", path).unwrap();
            }
        }
        drop(filter_file);

        // generate a gn out directory
        assert!(
            Command::new(&PATHS.gn_binary_path)
                .current_dir(&test_dir_path)
                .arg("gen")
                .arg(&out_dir)
                .arg("--ide=json")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("generating out directory")
                .success()
        );

        test_dir
    }

    #[test]
    fn no_auto_owners() {
        let test_dir = setup_test_dir("no_auto_owners");
        let test_dir_path = Utf8Path::from_path(test_dir.path()).unwrap();

        // an OWNERS file should be generated for projects missing owners
        let mut expected_owner_files = read_owners(&test_dir_path);
        assert!(!expected_owner_files.contains_key("third_party/bar/OWNERS"));
        assert!(!expected_owner_files.contains_key("third_party/rust_crates/foo/OWNERS"));
        assert!(
            !expected_owner_files
                .contains_key("third_party/rust_crates/with-dashes-and-version-0.1.0/OWNERS")
        );

        assert!(
            OwnersDb::new(Options {
                rust_metadata: Some(PATHS.test_base_dir.join("owners/rust_metadata.json")),
                integration_manifest: None,
                filter: Some(test_dir_path.join("filter-files")),
                overrides: PATHS.test_base_dir.join("owners/owners.toml"),
                project_json: test_dir_path.join("out/project.json"),
                fuchsia_dir: test_dir_path.to_path_buf(),
                skip_existing: false,
                project_paths: vec![],
                dry_run: false,
                generate_owners_from_reverse_deps: false,
            })
            .expect("valid OwnersDb")
            .update_all_files()
            .is_ok()
        );

        // Only these paths should be created.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/bar/OWNERS".into(),
            format!("include /tools/auto_owners/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/foo/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // The file should not contain the 'autogenerated' header because of the override.
        expected_owner_files.insert(
            "third_party/rust_crates/vendor/with-dashes-and-version-0.1.0/OWNERS".into(),
            format!("{HEADER}\ninclude /third_party/foo/OWNERS\n"),
        );

        // check that the OWNERS file was generated.
        let actual_owner_files = read_owners(&test_dir_path);
        assert_eq!(expected_owner_files, actual_owner_files);
    }

    fn copy_contents(original_test_dir: &Utf8Path, test_dir_path: &Utf8Path) {
        // copy the contents of original test dir to test_dir
        for entry in walkdir::WalkDir::new(&original_test_dir) {
            let entry = entry.expect("walking original test directory to copy files to /tmp");
            if !entry.file_type().is_file() {
                continue;
            }
            let to_copy = Utf8Path::from_path(entry.path()).unwrap();
            let destination = test_dir_path.join(to_copy.strip_prefix(&original_test_dir).unwrap());
            std::fs::create_dir_all(destination.parent().unwrap())
                .expect("making parent of file to copy");
            std::fs::copy(to_copy, destination).expect("copying file");
        }
        log::trace!("done copying files");
    }

    /// All the paths to runfiles and tools which are used in this test.
    ///
    /// All paths are absolute, and are resolved based on knowing that they are all
    /// beneath the directory in which this test binary is stored.  See the `BUILD.gn`
    /// file for this test target and the corresponding `host_test_data` targets.
    ///
    /// Note that it is not possible to refer to paths inside the source tree, because
    /// the source infra runners only have access to the output artifacts (i.e. contents
    /// of the "out" directory).
    #[derive(Debug)]
    struct Paths {
        /// `.../host_x64`
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        test_root_dir: Utf8PathBuf,

        /// `.../host_x64/test_data`, this is the root of the runfiles tree, a
        /// path //foo/bar will be copied at `.../host_x64/test_data/foo/bar` for
        /// this test.
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        test_data_dir: Utf8PathBuf,

        /// `.../host_x64/test_data/tools/auto_owners/tests`: this is the directory
        /// where GN golden files are placed. Corresponds to `//tools/auto_owners/tests`.
        test_base_dir: Utf8PathBuf,

        /// `.../host_x64/test_data/tools/auto_owners/runfiles`: this is the directory
        /// where the binary runfiles live.
        // TODO(https://fxbug.dev/42165549)
        #[allow(unused)]
        runfiles_dir: Utf8PathBuf,

        /// `.../runfiles/gn`: the absolute path to the gn binary. gn is used for
        /// formatting.
        gn_binary_path: Utf8PathBuf,
    }

    /// Gets the hermetic test paths for the runfiles and tools used in this test.
    ///
    /// The hermetic test paths are computed based on the parent directory of this
    /// binary.
    static PATHS: std::sync::LazyLock<Paths> = std::sync::LazyLock::new(|| {
        let cwd = Utf8PathBuf::from_path_buf(std::env::current_dir().unwrap()).unwrap();

        let first_arg = concat!(env!("ROOT_OUT_DIR"), "/auto_owners_test");
        //let first_arg = std::env::args().next().unwrap();
        let test_binary_path = cwd.join(first_arg);

        let test_root_dir = test_binary_path.parent().unwrap();

        let test_data_dir = test_root_dir.join("test_data");
        let test_base_dir = test_data_dir.join("tools").join("auto_owners").join("tests");
        let runfiles_dir =
            test_root_dir.join("test_data").join("tools").join("auto_owners").join("runfiles");
        let gn_binary_path = runfiles_dir.join("gn").join("gn");

        Paths {
            test_root_dir: test_root_dir.to_path_buf(),
            test_data_dir,
            test_base_dir,
            runfiles_dir,
            gn_binary_path,
        }
    });
}
