// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_dash::LauncherError;
use fidl_fuchsia_hardware_pty::DeviceProxy;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_kernel as fkernel;
use fuchsia_component::client::connect_to_protocol;
use indexmap::IndexMap;
use log::warn;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::simple::Simple as PseudoDirectory;
use vfs::file::vmo;

// The location of the added trampolines. The path will be of the form:
// `/.dash/tools/<package-name>/<trampoline-name>`.
const BASE_TOOLS_PATH: &str = "/.dash/tools";

// For each package, PkgDir holds the URL and its resolved directory.
#[derive(Debug)] // Necessary for PkgDir to be used in assert_matches!().
struct PkgDir {
    url: url::Url,
    resource: Option<String>,
    dir: fio::DirectoryProxy,
}

/// Returns (url without fragment, fragment if present)
fn parse_url(url: &str) -> Result<(url::Url, Option<String>), LauncherError> {
    let mut url = url::Url::parse(url).map_err(|e| {
        log::warn!(url:?; "bad url: {:#}", anyhow::anyhow!(e));
        LauncherError::BadUrl
    })?;
    if let Some(fragment) = url.fragment() {
        let fragment = fragment.to_owned();
        url.set_fragment(None);
        Ok((url, Some(fragment)))
    } else {
        Ok((url, None))
    }
}

// For each of the given packages, resolve them and create a PkgDir with its URL and DirectoryProxy.
async fn get_pkg_dirs(
    package_resolver: &mut crate::package_resolver::PackageResolver,
    tool_urls: Vec<String>,
) -> (Vec<PkgDir>, Vec<anyhow::Error>) {
    let mut dirs: Vec<PkgDir> = vec![];
    let mut errs: Vec<anyhow::Error> = vec![];
    for url in tool_urls {
        match parse_url(&url) {
            Ok((url, resource)) => match package_resolver.resolve(url.as_str()).await {
                Ok(dir) => dirs.push(PkgDir { url, resource, dir }),
                Err(e) => errs.push(format_err!("while resolving tool package {url}: {e}")),
            },
            Err(e) => errs.push(format_err!("while parsing tool url {url}: {e:?}")),
        }
    }
    (dirs, errs)
}

// A Trampoline holds the resolve script contents and the name that the used to run it.
#[derive(Clone)]
struct Trampoline {
    contents: String,
    binary_name: String,
}

// Using a BTreeSet will ensure that binaries are unique by name and are stored in insertion order.
// The Hash, Ord, PartialOrd, PartialEq, and Eq implementations enable Trampoline to be used in
// BTreeSet and are defined only on the `binary_name` field.
impl Hash for Trampoline {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.binary_name.hash(state);
    }
}
impl Ord for Trampoline {
    fn cmp(&self, other: &Self) -> Ordering {
        self.binary_name.cmp(&other.binary_name)
    }
}

impl PartialOrd for Trampoline {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Trampoline {
    fn eq(&self, other: &Self) -> bool {
        self.binary_name == other.binary_name
    }
}
impl Eq for Trampoline {}

// A package can have multiple binaries, so Trampolines contains the Trampoline(s) for each package.
struct Trampolines {
    // Use an IndexMap to maintain the keys in insertion order.
    map: IndexMap<url::Url, BTreeSet<Trampoline>>,
}

impl Trampolines {
    fn new() -> Self {
        Trampolines { map: IndexMap::new() }
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    // Insert the given set of trampolines into the map. The trampolines are merged in to common
    // package URLs. The trampoline names within packages are not repeated. Return a
    // NonUniqueBinaryName if an attempt is made to add a duplicate trampoline name within a
    // package.
    fn insert(
        &mut self,
        pkg: url::Url,
        trampolines: BTreeSet<Trampoline>,
    ) -> Result<(), LauncherError> {
        match self.map.entry(pkg) {
            indexmap::map::Entry::Occupied(mut o) => {
                let existing = o.get_mut();
                for t in trampolines {
                    if !existing.insert(t) {
                        return Err(LauncherError::NonUniqueBinaryName);
                    }
                }
            }
            indexmap::map::Entry::Vacant(v) => {
                v.insert(trampolines);
            }
        }
        Ok(())
    }

    // For each of the entries in the map, create an executable VMO file in a directory named after
    // the package. Accumulate these into a tools directory, updating a path string as well. Return
    // the directory and the path.
    fn make_tools_dir(
        &self,
        resource: zx::Resource,
    ) -> Result<(Arc<PseudoDirectory>, String), LauncherError> {
        let tools_dir = vfs::pseudo_directory! {};
        let mut path = String::new();
        for (url, trampolines) in &self.map {
            if !path.is_empty() {
                path.push(':');
            }
            let pkg_shell_name = escape_url_for_shell_path(url);
            path.push_str(&format!("{BASE_TOOLS_PATH}/{pkg_shell_name}"));
            let pkg_dir = vfs::pseudo_directory! {};
            for trampoline in trampolines {
                let read_exec_vmo = make_executable_vmo_file(&resource, &trampoline.contents)?;
                pkg_dir
                    .add_entry(&trampoline.binary_name, read_exec_vmo)
                    .map_err(|_| LauncherError::Internal)?;
            }
            tools_dir.add_entry(pkg_shell_name, pkg_dir).map_err(|_| LauncherError::Internal)?;
        }
        Ok((tools_dir, path))
    }
}

// The tool binaries for each package are stored in a directory named after the package URL so that
// the layout is easy to inspect.
// Forward slashes in the URL are converted to pipes to avoid issues with subdirectories.
// Colons are replaced with carats because the directories are used in the shell's $PATH environment
// variable which uses colon as a delimiter.
// Pipes and carats "should" not be used in URLs, so this shouldn't result in a collision.
// If there is a collision, adding the conflicting directory will fail and we will need a
// new naming scheme.
// Technically the directories do not have to be named after the packages because clients "should"
// be using the tool binaries indirectly via $PATH.
fn escape_url_for_shell_path(url: &url::Url) -> String {
    url.as_str().trim_end_matches('/').replace('/', "|").replace(':', "^")
}

// For each package, create the trampoline specifications for each of its binaries.
// For the user's information, a directory entry will be created even if there are no
// binaries found.
async fn create_trampolines(
    pkg_dirs: &Vec<PkgDir>,
    moniker: Option<String>,
) -> Result<Trampolines, LauncherError> {
    let mut trampolines = Trampolines::new();
    let rename_log = moniker.is_some();

    for pkg_dir in pkg_dirs {
        match &pkg_dir.resource {
            Some(res) => {
                let contents = format!("#!resolve {}#{}\n", pkg_dir.url, res);
                let mut binary_name =
                    res.split('/').next_back().ok_or(LauncherError::BadUrl)?.to_string();
                if rename_log && binary_name == "log" {
                    binary_name = "__log_real".to_string();
                }
                if rename_log && binary_name == "inspect" {
                    binary_name = "__inspect_real".to_string();
                }
                let set = BTreeSet::from([Trampoline { contents, binary_name }]);
                trampolines.insert(pkg_dir.url.clone(), set)?;
            }
            None => {
                // Read the package binaries.
                let bin_dir = fuchsia_fs::directory::open_directory(
                    &pkg_dir.dir,
                    "bin",
                    fio::PERM_READABLE | fio::PERM_EXECUTABLE,
                )
                .await
                .map_err(|_| LauncherError::ToolsBinaryRead)?;
                let entries = fuchsia_fs::directory::readdir(&bin_dir)
                    .await
                    .map_err(|_| LauncherError::Internal)?;

                // Create the trampoline specifications.
                let mut pkg_trampolines = BTreeSet::new();
                for entry in entries {
                    if entry.kind == fio::DirentType::File {
                        let contents = format!("#!resolve {}#bin/{}\n", pkg_dir.url, entry.name);
                        let mut binary_name = entry.name;
                        if rename_log && binary_name == "log" {
                            binary_name = "__log_real".to_string();
                        }
                        if rename_log && binary_name == "inspect" {
                            binary_name = "__inspect_real".to_string();
                        }
                        pkg_trampolines.insert(Trampoline { contents, binary_name });
                    }
                }
                trampolines.insert(pkg_dir.url.clone(), pkg_trampolines)?;
            }
        }
    }

    if let Some(moniker) = &moniker {
        let builtin_url = url::Url::parse("fuchsia-builtin://dash-launcher").unwrap();
        let pkg_shell_name = escape_url_for_shell_path(&builtin_url);
        let log_script_path = format!("{BASE_TOOLS_PATH}/{pkg_shell_name}/log");
        let ss_script_path = format!("{BASE_TOOLS_PATH}/{pkg_shell_name}/set-severity");
        let inspect_script_path = format!("{BASE_TOOLS_PATH}/{pkg_shell_name}/inspect");

        let log_contents = format!(
            "#!/.dash/internal/bin/sh {log_script_path}\n\
            [ \"$1\" = \"{log_script_path}\" ] && shift\n\
            [ \"$1\" = \"log\" ] && shift\n\
            if [ \"$1\" = \"set-severity\" ]; then\n\
                shift\n\
                /.dash/internal/bin/sh \"{ss_script_path}\" \"set-severity\" \"$@\"\n\
                exit $?\n\
            fi\n\
            /.dash/internal/bin/log_listener --hide-moniker --component \"{moniker}\" \"$@\"\n\
            exit $?\n",
        );
        let log_script = Trampoline { contents: log_contents, binary_name: "log".to_string() };

        let ss_contents = format!(
            "#!/.dash/internal/bin/sh {ss_script_path}\n\
            [ \"$1\" = \"{ss_script_path}\" ] && shift\n\
            [ \"$1\" = \"set-severity\" ] && shift\n\
            args=\"\"\n\
            selectors=\"\"\n\
            for arg in \"$@\"; do\n\
                case \"$arg\" in\n\
                    -* ) args=\"$args $arg\" ;;\n\
                    * ) \n\
                        case \"$arg\" in\n\
                            *#* ) selectors=\"$selectors $arg\" ;;\n\
                            * ) selectors=\"$selectors {moniker}#$arg\" ;;\n\
                        esac ;;\n\
                esac\n\
            done\n\
            /.dash/internal/bin/log_listener set-severity $args $selectors\n\
            exit $?\n",
        );
        let ss_script =
            Trampoline { contents: ss_contents, binary_name: "set-severity".to_string() };

        let inspect_contents = format!(
            "#!/.dash/internal/bin/sh {inspect_script_path}\n\
            [ \"$1\" = \"{inspect_script_path}\" ] && shift\n\
            [ \"$1\" = \"inspect\" ] && shift\n\
            /.dash/internal/bin/inspect show \"{moniker}\" \"$@\"\n\
            exit $?\n",
        );
        let inspect_script =
            Trampoline { contents: inspect_contents, binary_name: "inspect".to_string() };

        trampolines.insert(builtin_url, BTreeSet::from([log_script, ss_script, inspect_script]))?;
    }

    Ok(trampolines)
}

async fn create_vmex_resource() -> Result<zx::Resource, LauncherError> {
    let vmex_proxy = connect_to_protocol::<fkernel::VmexResourceMarker>()
        .map_err(|_| LauncherError::VmexResource)?;
    let vmex_resource = vmex_proxy.get().await.map_err(|_| LauncherError::VmexResource)?;
    Ok(zx::Resource::from(vmex_resource))
}

fn make_executable_vmo_file(
    resource: &zx::Resource,
    contents: &str,
) -> Result<Arc<vmo::VmoFile>, LauncherError> {
    let vmo = zx::Vmo::create(contents.len() as u64).map_err(|_| LauncherError::Internal)?;
    vmo.write(contents.as_bytes(), 0).map_err(|_| LauncherError::Internal)?;

    // Make it into a VMO that can be loaded as an executable.
    let exec_vmo = vmo.replace_as_executable(&resource).map_err(|_| LauncherError::Internal)?;
    let exec_file = vmo::VmoFile::new_with_inode_and_executable(
        exec_vmo,
        fio::INO_UNKNOWN,
        /*executable*/ true,
    );
    Ok(exec_file)
}

// Given a list of package trampoline specifications, create the executable files and add them into
// a new directory. Give each package its own subdirectory based on the package name, preventing
// binary name collisions. Binary names can repeat if they are found in different packages. However,
// only the package name is considered, so packages cannot differ only in host, variant, or hash.
//
// The package directories are added in the order they are given on the command line. The enclosing
// directory is prepended to the path environment variable. The resulting preference order for
// same-name binaries is therefore first packages in load order, then built-ins. To avoid shadowing,
// use the complete path to the binary: $ .dash/tools/<pkg>/<binary>
//
// Return the VFS directory containing the executables and the associated path environment variable.
async fn make_trampoline_vfs(
    trampolines: Trampolines,
) -> Result<(Option<fio::DirectoryProxy>, Option<String>), LauncherError> {
    if trampolines.is_empty() {
        return Ok((None, None));
    }
    let resource = match create_vmex_resource().await {
        Ok(r) => r,
        Err(err) => {
            warn!(err:?; "Could not acquire VmexResource. Trampolines will not be available.");
            return Ok((None, None));
        }
    };
    let (tools_dir, path) = trampolines.make_tools_dir(resource)?;
    // Serve directory with execute rights.
    let dir = vfs::directory::serve(
        tools_dir,
        vfs::execution_scope::ExecutionScope::new(),
        fio::PERM_READABLE | fio::PERM_EXECUTABLE,
    );
    Ok((Some(dir), Some(path)))
}

// Given the URLs of some packages, return a directory containing their binaries as trampolines.
// Log to the pty if there are any errors so the user can see them somewhere other than syslog.
pub async fn create_trampolines_from_packages(
    package_resolver: &mut crate::package_resolver::PackageResolver,
    pkg_urls: Vec<String>,
    pty_proxy: &DeviceProxy,
    moniker: Option<String>,
) -> Result<(Option<fio::DirectoryProxy>, Option<String>), LauncherError> {
    let (pkg_dirs, errs) = get_pkg_dirs(package_resolver, pkg_urls).await;
    if !errs.is_empty() {
        let mut summary = "Failed to load at least one tool package: ".to_string();
        for err in errs {
            summary.push_str(&format!("{:?}\n", err));
        }

        log::warn!("{}", summary);
        let _ = pty_proxy.write(summary.as_bytes()).await.unwrap_or_else(|e| {
            log::warn!("Couldn't write to user stdout: {e:?}");
            Ok(0u64)
        });
    }
    let trampolines = create_trampolines(&pkg_dirs, moniker).await?;
    make_trampoline_vfs(trampolines).await
}

// Create a PATH environment variable from the tools_path if present.
pub fn create_env_path(tools_path: Option<String>) -> String {
    let mut path_envvar = "".to_string();
    if let Some(tp) = tools_path {
        path_envvar.push_str("PATH=");
        path_envvar.push_str(&tp);
    }
    path_envvar
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_pkg as fpkg;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::fmt;
    use vfs::file::vmo::read_only;

    #[fuchsia::test]
    async fn parse_url_test() {
        // Resource removed
        let (url, res) = parse_url("my-scheme://example.test/path#resource").unwrap();
        assert_eq!(url.as_str(), "my-scheme://example.test/path");
        assert_eq!(res, Some("resource".to_string()));

        // Missing resource ignored
        let (url, res) = parse_url("my-scheme://example.test/path").unwrap();
        assert_eq!(url.as_str(), "my-scheme://example.test/path");
        assert_eq!(res, None);

        // Query string ignored
        let (url, res) = parse_url("my-scheme://example.test/path?my-query").unwrap();
        assert_eq!(url.as_str(), "my-scheme://example.test/path?my-query");
        assert_eq!(res, None);

        // Multiple path segments supported
        let (url, res) = parse_url("my-scheme://example.test/p0/p1/p2").unwrap();
        assert_eq!(url.as_str(), "my-scheme://example.test/p0/p1/p2");
        assert_eq!(res, None);
    }

    #[fuchsia::test]
    async fn get_pkg_dirs_test() {
        let (resolver, mut stream) = create_proxy_and_stream::<fpkg::PackageResolverMarker>();
        // Spawn a task to handle the stream of requests
        fasync::Task::spawn(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fpkg::PackageResolverRequest::Resolve { responder, package_url: _, dir: _ } => {
                        responder
                            .send(Ok(&fidl_fuchsia_pkg::ResolutionContext { bytes: vec![] }))
                            .unwrap();
                    }
                    _ => {}
                }
            }
        })
        .detach();
        let mut resolver = crate::package_resolver::PackageResolver::new_test(resolver);

        // Empty package list.
        let (dirs, errs) = get_pkg_dirs(&mut resolver, vec![]).await;
        assert!(dirs.is_empty());
        assert!(errs.is_empty());

        // Non-empty package list, but with a malformed URL.
        let (dirs, errs) = get_pkg_dirs(&mut resolver, vec!["".to_string()]).await;
        assert!(dirs.is_empty());
        assert_eq!(errs.len(), 1);

        // Valid package list.
        let (dirs, errs) = get_pkg_dirs(&mut resolver, vec!["fuchsia-pkg://h/n".to_string()]).await;
        assert!(errs.is_empty());
        assert!(dirs.len() == 1);
        assert_eq!(dirs[0].url.as_str(), "fuchsia-pkg://h/n");
        assert_eq!(dirs[0].resource, None);
    }

    // Required for assert_matches!().
    impl fmt::Debug for Trampolines {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "map size: {}", self.map.len())
        }
    }

    impl Trampolines {
        fn len(&self) -> usize {
            self.map.len()
        }

        fn get_nth_package(&self, i: usize) -> Option<(&url::Url, &BTreeSet<Trampoline>)> {
            self.map.iter().nth(i)
        }
    }

    async fn make_pkg(url: &str, name: &str, root: &Arc<PseudoDirectory>) -> PkgDir {
        let (url, resource) = parse_url(url).unwrap();
        let path = vfs::path::Path::validate_and_split(name).unwrap();
        let dir = vfs::serve_directory(
            root.clone(),
            path,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_EXECUTABLE,
        );
        PkgDir { url, dir, resource }
    }

    #[track_caller]
    fn check_trampoline(
        trampolines: &BTreeSet<Trampoline>,
        bin: usize,
        contents: &str,
        binary_name: &str,
    ) {
        let trampoline = trampolines.iter().nth(bin).unwrap();
        assert_eq!(trampoline.contents, contents);
        assert_eq!(trampoline.binary_name, binary_name);
    }

    #[fuchsia::test]
    async fn create_trampolines_test() {
        async fn create_test_directory_proxies() -> Vec<PkgDir> {
            let root = vfs::pseudo_directory! {
                "Nasa" => vfs::pseudo_directory! {
                    "bin" => vfs::pseudo_directory! {
                        "go2moon_v1969" => read_only(b"Apollo"),
                        "go2moon_v2024" => read_only(b"Artemis"),
                    },
                },
                "SpaceX" => vfs::pseudo_directory! {
                    "bin" => vfs::pseudo_directory! {
                        "go2orbit" => read_only(b"Falcon 9"),
                        "go2mars" => read_only(b"Starship"),
                        "bogus_dir" => vfs::pseudo_directory! {"bogus_file" => read_only(b"bogus_content"),}
                    },
                },
                "BlueOrigin" => vfs::pseudo_directory! {
                    "bin" => vfs::pseudo_directory! {},
                },
            };
            vec![
                make_pkg("fuchsia-pkg://earth.org/nasa_pkg", "Nasa", &root).await,
                make_pkg("fuchsia-pkg://earth.org/spacex_pkg", "SpaceX", &root).await,
                make_pkg("fuchsia-pkg://earth.org/blueorigin_pkg", "BlueOrigin", &root).await,
            ]
        }

        let pkg_dirs = create_test_directory_proxies().await;
        let pkg_trampolines = create_trampolines(&pkg_dirs, None).await.unwrap();
        assert_eq!(pkg_trampolines.len(), 3); // Includes one for "blueorigin_pkg", which had no binaries.
        let (url, list) = pkg_trampolines.get_nth_package(0).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/nasa_pkg");
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/nasa_pkg#bin/go2moon_v1969\n",
            "go2moon_v1969",
        );
        check_trampoline(
            &list,
            1,
            "#!resolve fuchsia-pkg://earth.org/nasa_pkg#bin/go2moon_v2024\n",
            "go2moon_v2024",
        );

        let (url, list) = pkg_trampolines.get_nth_package(1).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/spacex_pkg");
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/spacex_pkg#bin/go2mars\n",
            "go2mars",
        );
        check_trampoline(
            &list,
            1,
            "#!resolve fuchsia-pkg://earth.org/spacex_pkg#bin/go2orbit\n",
            "go2orbit",
        );

        let (url, list) = pkg_trampolines.get_nth_package(2).unwrap();
        assert_eq!(list.len(), 0);
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/blueorigin_pkg");
    }

    #[fuchsia::test]
    async fn trampolines_binary_collisions_test() {
        // Create two packages containing the same binary name.
        let root = vfs::pseudo_directory! {
            "Nasa" => vfs::pseudo_directory! {
                "bin" => vfs::pseudo_directory! {
                    "collision" => read_only(b"Apollo"),
                },
            },
            "SpaceX" => vfs::pseudo_directory! {
                "bin" => vfs::pseudo_directory! {
                    "collision" => read_only(b"Falcon 9"),
                },
            },
        };
        // But use URLs that have unique URLs.
        let pkg_dirs = vec![
            make_pkg("fuchsia-pkg://earth.org/nasa_pkg", "Nasa", &root).await,
            make_pkg("fuchsia-pkg://earth.org/spacex_pkg", "SpaceX", &root).await,
        ];

        let pkg_trampolines = create_trampolines(&pkg_dirs, None).await.unwrap();
        // Below, the trampolines are both named `collision`. However, they are in different
        // packages, so do not conflict.
        let (url, list) = pkg_trampolines.get_nth_package(0).unwrap();
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/nasa_pkg");
        assert_eq!(list.len(), 1);
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/nasa_pkg#bin/collision\n",
            "collision",
        );
        let (url, list) = pkg_trampolines.get_nth_package(1).unwrap();
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/spacex_pkg");
        assert_eq!(list.len(), 1);
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/spacex_pkg#bin/collision\n",
            "collision",
        );

        // Query strings are included in the directory name and so prevent collisions.
        let pkg_trampolines =  create_trampolines(&vec![
            make_pkg("fuchsia-pkg://earth.org/spacex_pkg", "SpaceX", &root).await,
            make_pkg("fuchsia-pkg://earth.org/spacex_pkg?hash=0000000000000000000000000000000000000000000000000000000000000000", "SpaceX", &root).await,
        ], None).await.unwrap();
        let (url, list) = pkg_trampolines.get_nth_package(0).unwrap();
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/spacex_pkg");
        assert_eq!(list.len(), 1);
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/spacex_pkg#bin/collision\n",
            "collision",
        );
        let (url, list) = pkg_trampolines.get_nth_package(1).unwrap();
        assert_eq!(
            url.as_str(),
            "fuchsia-pkg://earth.org/spacex_pkg?hash=0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert_eq!(list.len(), 1);
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/spacex_pkg?hash=0000000000000000000000000000000000000000000000000000000000000000#bin/collision\n",
            "collision",
        );
    }

    #[fuchsia::test]
    async fn trampolines_package_collisions_test() {
        // Create two packages containing the same package name, but different binaries.
        let root = vfs::pseudo_directory! {
            "UpGoer" => vfs::pseudo_directory! {
                "bin" => vfs::pseudo_directory! {
                    "apollo" => read_only(b"Apollo"),
                },
            }
        };
        let root1 = vfs::pseudo_directory! {
            "UpGoer" => vfs::pseudo_directory! {
                "bin" => vfs::pseudo_directory! {
                    "falcon9" => read_only(b"Falcon 9"),
                },
             }
        };
        // The package names, being the same, are merged. The binaries are different, so do not
        // conflict.
        let pkg_trampolines = create_trampolines(
            &vec![
                make_pkg("fuchsia-pkg://earth.org/upgoer_pkg", "UpGoer", &root).await,
                make_pkg("fuchsia-pkg://earth.org/upgoer_pkg", "UpGoer", &root1).await,
            ],
            None,
        )
        .await
        .unwrap();
        let (url, list) = pkg_trampolines.get_nth_package(0).unwrap();
        assert_eq!(url.as_str(), "fuchsia-pkg://earth.org/upgoer_pkg");
        assert_eq!(list.len(), 2);
        check_trampoline(
            &list,
            0,
            "#!resolve fuchsia-pkg://earth.org/upgoer_pkg#bin/apollo\n",
            "apollo",
        );
        check_trampoline(
            &list,
            1,
            "#!resolve fuchsia-pkg://earth.org/upgoer_pkg#bin/falcon9\n",
            "falcon9",
        );
    }

    #[fuchsia::test]
    async fn make_trampoline_vfs_test_empty() {
        let (dirs, path) = make_trampoline_vfs(Trampolines::new()).await.unwrap();
        assert_matches!(dirs, None);
        assert_matches!(path, None);
    }

    #[fuchsia::test]
    async fn make_trampoline_vfs_test_if_package_repeated() {
        let mut set = BTreeSet::new();
        set.insert(Trampoline { contents: "foo".to_string(), binary_name: "bar".to_string() });
        let mut set2 = BTreeSet::new();
        set2.insert(Trampoline { contents: "foo2".to_string(), binary_name: "bar2".to_string() });
        let mut trampolines = Trampolines::new();
        trampolines.insert("fuchsia-pkg://h/pkg_foobar".parse().unwrap(), set).unwrap();
        trampolines.insert("fuchsia-pkg://h/pkg_foobar".parse().unwrap(), set2).unwrap();
        let (dir, path) = make_trampoline_vfs(trampolines).await.unwrap();
        assert!(dir.is_some());
        // There is only one entry in the path for pkg_foobar.
        assert_eq!(path.unwrap(), "/.dash/tools/fuchsia-pkg^||h|pkg_foobar");
    }

    #[fuchsia::test]
    async fn make_trampoline_vfs_test() {
        async fn contents_of(path: &str, dir: &fio::DirectoryProxy) -> String {
            fuchsia_fs::directory::read_file_to_string(dir, path)
                .await
                .unwrap_or_else(|e| panic!("could not read file: {path}: {e:?}"))
        }

        let mut pkg_trampolines = Trampolines::new();
        pkg_trampolines
            .insert(
                "fuchsia-pkg://h/pkg_foobar".parse().unwrap(),
                BTreeSet::from([
                    Trampoline {
                        contents: "#!resolve foo".to_string(),
                        binary_name: "foo".to_string(),
                    },
                    Trampoline {
                        contents: "#!resolve bar".to_string(),
                        binary_name: "bar".to_string(),
                    },
                ]),
            )
            .unwrap();
        pkg_trampolines
            .insert(
                "fuchsia-pkg://h/pkg_foobar2".parse().unwrap(),
                BTreeSet::from([
                    Trampoline {
                        contents: "#!resolve foo2".to_string(),
                        binary_name: "foo2".to_string(),
                    },
                    Trampoline {
                        contents: "#!resolve bar2".to_string(),
                        binary_name: "bar2".to_string(),
                    },
                ]),
            )
            .unwrap();
        let (dirs, path) = make_trampoline_vfs(pkg_trampolines).await.unwrap();

        // Check the path. Order is insertion order.
        assert_eq!(
            path,
            Some(
                "/.dash/tools/fuchsia-pkg^||h|pkg_foobar:/.dash/tools/fuchsia-pkg^||h|pkg_foobar2"
                    .to_string()
            )
        );

        // Check the directory.
        let dir = dirs.unwrap();
        assert_eq!(
            fuchsia_fs::directory::readdir(&dir).await.unwrap(),
            vec![
                fuchsia_fs::directory::DirEntry {
                    name: "fuchsia-pkg^||h|pkg_foobar".to_owned(),
                    kind: fuchsia_fs::directory::DirentKind::Directory
                },
                fuchsia_fs::directory::DirEntry {
                    name: "fuchsia-pkg^||h|pkg_foobar2".to_owned(),
                    kind: fuchsia_fs::directory::DirentKind::Directory
                },
            ]
        );

        assert_eq!(&contents_of("fuchsia-pkg^||h|pkg_foobar/foo", &dir).await, "#!resolve foo");
        assert_eq!(&contents_of("fuchsia-pkg^||h|pkg_foobar/bar", &dir).await, "#!resolve bar");
        assert_eq!(&contents_of("fuchsia-pkg^||h|pkg_foobar2/foo2", &dir).await, "#!resolve foo2");
        assert_eq!(&contents_of("fuchsia-pkg^||h|pkg_foobar2/bar2", &dir).await, "#!resolve bar2");
    }

    #[fuchsia::test]
    async fn create_env_path_test() {
        assert_eq!(create_env_path(Some("bar".to_string())), "PATH=bar");
        assert_eq!(create_env_path(Some("".to_string())), "PATH=");
        assert_eq!(create_env_path(Some(" ".to_string())), "PATH= ");
        assert_eq!(create_env_path(None), "");
    }
}
