// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]
#![cfg(test)]
use anyhow::Error;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_pkg::{
    self as fpkg, PackageCacheRequest, PackageCacheRequestStream, PackageResolverRequest,
    PackageResolverRequestStream, RepositoryIteratorRequest, RepositoryManagerRequest,
    RepositoryManagerRequestStream, ResolveError,
};
use fidl_fuchsia_pkg_ext::{
    MirrorConfig, MirrorConfigBuilder, RepositoryConfig, RepositoryConfigBuilder, RepositoryKey,
};
use fidl_fuchsia_pkg_garbagecollector as fpkg_gc;
use fidl_fuchsia_pkg_rewrite::{
    EditTransactionRequest, EngineRequest, EngineRequestStream, RuleIteratorMarker,
    RuleIteratorRequest,
};
use fidl_fuchsia_pkg_rewrite_ext::Rule;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_hyper_test_support::TestServer;
use fuchsia_hyper_test_support::handler::StaticResponse;
use fuchsia_sync::Mutex;
use fuchsia_url::RepositoryUrl;
use futures::prelude::*;
use http::Uri;
use shell_process::ProcessOutput;
use std::fs::{File, create_dir};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use vfs::directory::entry_container::Directory;
use zx::Status;

const BINARY_PATH: &str = "/pkg/bin/pkgctl";

struct TestEnv {
    engine: Arc<MockEngineService>,
    repository_manager: Arc<MockRepositoryManagerService>,
    package_cache: Arc<MockPackageCacheService>,
    package_resolver: Arc<MockPackageResolverService>,
    space_manager: Arc<MockSpaceManagerService>,
    _test_dir: TempDir,
    repo_config_arg_path: PathBuf,
    svc_proxy: fidl_fuchsia_io::DirectoryProxy,
}

impl TestEnv {
    fn new() -> Self {
        let mut fs = ServiceFs::new();
        fs.add_proxy_service::<fidl_fuchsia_net_http::LoaderMarker, _>();

        let package_resolver = Arc::new(MockPackageResolverService::new());
        let package_resolver_clone = package_resolver.clone();
        fs.add_fidl_service(move |stream: PackageResolverRequestStream| {
            let package_resolver_clone = package_resolver_clone.clone();
            fasync::Task::spawn(
                package_resolver_clone
                    .run_service(stream)
                    .unwrap_or_else(|e| panic!("error running resolver service: {e:?}")),
            )
            .detach()
        });

        let package_cache = Arc::new(MockPackageCacheService::new());
        let package_cache_clone = package_cache.clone();
        fs.add_fidl_service(move |stream: PackageCacheRequestStream| {
            let package_cache_clone = package_cache_clone.clone();
            fasync::Task::spawn(
                package_cache_clone
                    .run_service(stream)
                    .unwrap_or_else(|e| panic!("error running cache service: {e:?}")),
            )
            .detach()
        });

        let engine = Arc::new(MockEngineService::new());
        let engine_clone = engine.clone();
        fs.add_fidl_service(move |stream: EngineRequestStream| {
            let engine_clone = engine_clone.clone();
            fasync::Task::spawn(
                engine_clone
                    .run_service(stream)
                    .unwrap_or_else(|e| panic!("error running engine service: {e:?}")),
            )
            .detach()
        });

        let repository_manager = Arc::new(MockRepositoryManagerService::new());
        let repository_manager_clone = repository_manager.clone();
        fs.add_fidl_service(move |stream: RepositoryManagerRequestStream| {
            let repository_manager_clone = repository_manager_clone.clone();
            fasync::Task::spawn(
                repository_manager_clone
                    .run_service(stream)
                    .unwrap_or_else(|e| panic!("error running repository service: {e:?}")),
            )
            .detach()
        });

        let space_manager = Arc::new(MockSpaceManagerService::new());
        let space_manager_clone = space_manager.clone();
        fs.add_fidl_service(move |stream: fpkg_gc::ManagerRequestStream| {
            let space_manager_clone = space_manager_clone.clone();
            fasync::Task::spawn(
                space_manager_clone
                    .run_service(stream)
                    .unwrap_or_else(|e| panic!("error running space service: {e:?}")),
            )
            .detach()
        });

        let _test_dir = TempDir::new().expect("create test tempdir");

        let repo_config_arg_path = _test_dir.path().join("repo_config");
        create_dir(&repo_config_arg_path).expect("create repo_config_arg dir");

        let (svc_proxy, svc_server_end) = fidl::endpoints::create_proxy();

        let _env = fs.serve_connection(svc_server_end).expect("serve connection");

        fasync::Task::spawn(fs.collect()).detach();

        Self {
            engine,
            repository_manager,
            package_cache,
            package_resolver,
            space_manager,
            _test_dir,
            repo_config_arg_path,
            svc_proxy,
        }
    }

    async fn run_pkgctl<'a>(&'a self, args: Vec<&'a str>) -> ProcessOutput {
        let repo_config_arg_dir = fuchsia_fs::directory::open_in_namespace(
            &self.repo_config_arg_path.display().to_string(),
            fio::PERM_READABLE,
        )
        .unwrap();

        shell_process::run_process(
            BINARY_PATH,
            args,
            [("/svc", &self.svc_proxy), ("/repo-configs", &repo_config_arg_dir)],
        )
        .await
    }

    fn add_repository(&self, repo_config: RepositoryConfig) {
        self.repository_manager.repos.lock().push(repo_config);
    }

    fn assert_only_repository_manager_called_with(
        &self,
        expected_args: Vec<CapturedRepositoryManagerRequest>,
    ) {
        assert_eq!(self.package_cache.captured_args.lock().len(), 0);
        assert_eq!(self.package_resolver.captured_args.lock().len(), 0);
        assert_eq!(self.engine.captured_args.lock().len(), 0);
        assert_eq!(*self.repository_manager.captured_args.lock(), expected_args);
        assert_eq!(*self.space_manager.call_count.lock(), 0);
    }

    fn add_rule(&self, rule: Rule) {
        self.engine.rules.lock().push(rule);
    }

    fn assert_only_engine_called_with(&self, expected_args: Vec<CapturedEngineRequest>) {
        assert_eq!(self.package_cache.captured_args.lock().len(), 0);
        assert_eq!(self.package_resolver.captured_args.lock().len(), 0);
        assert_eq!(*self.engine.captured_args.lock(), expected_args);
        assert_eq!(self.repository_manager.captured_args.lock().len(), 0);
        assert_eq!(*self.space_manager.call_count.lock(), 0);
    }

    fn assert_only_space_manager_called(&self) {
        assert_eq!(self.package_cache.captured_args.lock().len(), 0);
        assert_eq!(self.package_resolver.captured_args.lock().len(), 0);
        assert_eq!(self.engine.captured_args.lock().len(), 0);
        assert_eq!(self.repository_manager.captured_args.lock().len(), 0);
        assert_eq!(*self.space_manager.call_count.lock(), 1);
    }

    fn assert_only_package_resolver_called_with(&self, reqs: Vec<CapturedPackageResolverRequest>) {
        assert_eq!(self.package_cache.captured_args.lock().len(), 0);
        assert_eq!(self.engine.captured_args.lock().len(), 0);
        assert_eq!(self.repository_manager.captured_args.lock().len(), 0);
        assert_eq!(*self.space_manager.call_count.lock(), 0);
        assert_eq!(*self.package_resolver.captured_args.lock(), reqs);
    }

    fn assert_only_package_resolver_and_package_cache_called_with(
        &self,
        resolver_reqs: Vec<CapturedPackageResolverRequest>,
        cache_reqs: Vec<CapturedPackageCacheRequest>,
    ) {
        assert_eq!(*self.package_cache.captured_args.lock(), cache_reqs);
        assert_eq!(self.engine.captured_args.lock().len(), 0);
        assert_eq!(self.repository_manager.captured_args.lock().len(), 0);
        assert_eq!(*self.space_manager.call_count.lock(), 0);
        assert_eq!(*self.package_resolver.captured_args.lock(), resolver_reqs);
    }
}

#[derive(PartialEq, Eq, Debug)]
enum CapturedEngineRequest {
    StartEditTransaction,
}

struct MockEngineService {
    captured_args: Mutex<Vec<CapturedEngineRequest>>,
    rules: Mutex<Vec<Rule>>,
}

impl MockEngineService {
    fn new() -> Self {
        Self { captured_args: Mutex::new(vec![]), rules: Mutex::new(vec![]) }
    }
    async fn run_service(self: Arc<Self>, mut stream: EngineRequestStream) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                EngineRequest::StartEditTransaction {
                    transaction,
                    control_handle: _control_handle,
                } => {
                    self.captured_args.lock().push(CapturedEngineRequest::StartEditTransaction);
                    let mut stream = transaction.into_stream();
                    while let Some(sub_req) = stream.try_next().await? {
                        match sub_req {
                            EditTransactionRequest::ListDynamic {
                                iterator,
                                control_handle: _control_handle,
                            } => {
                                self.list_dynamic_handler(iterator).await?;
                            }
                            EditTransactionRequest::Commit { responder } => {
                                responder.send(Ok(())).expect("send ok");
                            }
                            _ => {
                                panic!("unhandled request method {sub_req:?}");
                            }
                        }
                    }
                }
                _ => {
                    panic!("unhandled request method {req:?}");
                }
            }
        }
        Ok(())
    }

    async fn list_dynamic_handler(
        &self,
        iterator: ServerEnd<RuleIteratorMarker>,
    ) -> Result<(), Error> {
        let mut stream = iterator.into_stream();
        let rules =
            self.rules.lock().clone().into_iter().map(|rule| rule.into()).collect::<Vec<_>>();
        let mut iter = rules.chunks(5).fuse();
        while let Some(RuleIteratorRequest::Next { responder }) = stream.try_next().await? {
            responder.send(iter.next().unwrap_or(&[])).expect("next send")
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Debug)]
enum CapturedRepositoryManagerRequest {
    Add { repo: RepositoryConfig },
    Remove { repo_url: String },
    AddMirror { repo_url: String, mirror: MirrorConfig },
    RemoveMirror { repo_url: String, mirror_url: String },
    List,
}

struct MockRepositoryManagerService {
    captured_args: Mutex<Vec<CapturedRepositoryManagerRequest>>,
    repos: Mutex<Vec<RepositoryConfig>>,
}

impl MockRepositoryManagerService {
    fn new() -> Self {
        Self { captured_args: Mutex::new(vec![]), repos: Mutex::new(vec![]) }
    }
    async fn run_service(
        self: Arc<Self>,
        mut stream: RepositoryManagerRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                RepositoryManagerRequest::Add { repo, responder } => {
                    self.captured_args.lock().push(CapturedRepositoryManagerRequest::Add {
                        repo: RepositoryConfig::try_from(repo).expect("valid repo config"),
                    });
                    responder.send(Ok(())).expect("send ok");
                }
                RepositoryManagerRequest::Remove { repo_url, responder } => {
                    self.captured_args
                        .lock()
                        .push(CapturedRepositoryManagerRequest::Remove { repo_url });
                    responder.send(Ok(())).expect("send ok");
                }
                RepositoryManagerRequest::AddMirror { repo_url, mirror, responder } => {
                    self.captured_args.lock().push(CapturedRepositoryManagerRequest::AddMirror {
                        repo_url,
                        mirror: MirrorConfig::try_from(mirror).expect("valid mirror config"),
                    });
                    responder.send(Ok(())).expect("send ok");
                }
                RepositoryManagerRequest::RemoveMirror { repo_url, mirror_url, responder } => {
                    self.captured_args.lock().push(
                        CapturedRepositoryManagerRequest::RemoveMirror { repo_url, mirror_url },
                    );
                    responder.send(Ok(())).expect("send ok");
                }
                RepositoryManagerRequest::List { iterator, control_handle: _control_handle } => {
                    self.captured_args.lock().push(CapturedRepositoryManagerRequest::List);
                    let mut stream = iterator.into_stream();
                    let repos: Vec<_> =
                        self.repos.lock().clone().into_iter().map(|r| r.into()).collect();
                    let mut iter = repos.chunks(5).fuse();
                    while let Some(RepositoryIteratorRequest::Next { responder }) =
                        stream.try_next().await?
                    {
                        responder.send(iter.next().unwrap_or(&[])).expect("next send")
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Debug)]
enum CapturedPackageResolverRequest {
    Resolve { package_url: String },
    GetHash { package_url: String },
}

#[allow(clippy::type_complexity)]
struct MockPackageResolverService {
    captured_args: Mutex<Vec<CapturedPackageResolverRequest>>,
    get_hash_response: Mutex<Option<Result<fidl_fuchsia_pkg::BlobId, Status>>>,
    resolve_response: Mutex<
        Option<(
            Arc<dyn Directory>,
            Result<fpkg::ResolutionContext, fidl_fuchsia_pkg::ResolveError>,
        )>,
    >,
}

impl MockPackageResolverService {
    fn new() -> Self {
        Self {
            captured_args: Mutex::new(vec![]),
            get_hash_response: Mutex::new(None),
            resolve_response: Mutex::new(None),
        }
    }
    async fn run_service(
        self: Arc<Self>,
        mut stream: PackageResolverRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                PackageResolverRequest::Resolve { package_url, dir: server_end, responder } => {
                    self.captured_args
                        .lock()
                        .push(CapturedPackageResolverRequest::Resolve { package_url });

                    let (dir, res) = self.resolve_response.lock().take().unwrap();
                    vfs::directory::serve_on(
                        dir,
                        fio::PERM_READABLE,
                        vfs::ExecutionScope::new(),
                        server_end,
                    );
                    responder.send(res.as_ref().map_err(|e| *e)).unwrap()
                }
                PackageResolverRequest::ResolveWithContext {
                    package_url: _,
                    context: _,
                    dir: _,
                    responder,
                } => {
                    // not implemented
                    responder.send(Err(ResolveError::Internal)).unwrap()
                }
                PackageResolverRequest::GetHash { package_url, responder } => {
                    self.captured_args.lock().push(CapturedPackageResolverRequest::GetHash {
                        package_url: package_url.url,
                    });
                    let response = self.get_hash_response.lock().unwrap();
                    responder.send(response.as_ref().map_err(|s| s.into_raw())).unwrap()
                }
            }
        }
        Ok(())
    }
}

#[derive(PartialEq, Debug, Eq)]
enum CapturedPackageCacheRequest {
    Get { meta_far_blob_id: fpkg::BlobInfo },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GetBehavior {
    AlreadyCached,
    NotCached,
    ImmediateClose,
}

struct MockPackageCacheService {
    captured_args: Mutex<Vec<CapturedPackageCacheRequest>>,
    get_behavior: Mutex<GetBehavior>,
}

impl MockPackageCacheService {
    fn new() -> Self {
        Self {
            captured_args: Mutex::new(vec![]),
            get_behavior: Mutex::new(GetBehavior::ImmediateClose),
        }
    }
    async fn run_service(
        self: Arc<Self>,
        mut stream: PackageCacheRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            match req {
                PackageCacheRequest::Get {
                    meta_far_blob,
                    gc_protection,
                    needed_blobs,
                    dir,
                    responder,
                } => {
                    assert_eq!(gc_protection, fpkg::GcProtection::OpenPackageTracking);
                    self.captured_args
                        .lock()
                        .push(CapturedPackageCacheRequest::Get { meta_far_blob_id: meta_far_blob });
                    let () = self.handle_get(meta_far_blob, needed_blobs, dir, responder).await;
                }
                PackageCacheRequest::GetSubpackage { .. } => {
                    panic!("should only support Get requests, received GetSubpackage")
                }
                PackageCacheRequest::BasePackageIndex { .. } => {
                    panic!("should only support Get requests, received BasePackageIndex")
                }
                PackageCacheRequest::CachePackageIndex { .. } => {
                    panic!("should only support Get requests, received CachePackageIndex")
                }
                PackageCacheRequest::Sync { .. } => {
                    panic!("should only support Get requests, received Sync")
                }
                PackageCacheRequest::SetUpgradableUrls { .. } => {
                    panic!("should only support Get requests, received SetUpgradableUrls")
                }
                PackageCacheRequest::WriteBlobs { .. } => {
                    panic!("should only support Get requests, received WriteBlobs")
                }
                PackageCacheRequest::_UnknownMethod { .. } => {
                    panic!("should only support Get requests, received UnknownMethod")
                }
            }
        }
        Ok(())
    }
    async fn handle_get(
        &self,
        _meta_far: fpkg::BlobInfo,
        needed_blobs: ServerEnd<fpkg::NeededBlobsMarker>,
        _dir: ServerEnd<fio::DirectoryMarker>,
        get_responder: fpkg::PackageCacheGetResponder,
    ) {
        let behavior = *self.get_behavior.lock();
        match behavior {
            GetBehavior::AlreadyCached => {
                let (_, control) = needed_blobs.into_stream_and_control_handle();
                let () = control.shutdown_with_epitaph(Ok(()));
                let () = get_responder.send(Ok(())).unwrap();
            }
            GetBehavior::NotCached => {
                let mut stream = needed_blobs.into_stream();
                let req = stream.next().await.unwrap().unwrap();
                let fpkg::NeededBlobsRequest::OpenMetaBlob { responder, .. } = req else {
                    panic!("unexpected NeededBlobsRequest: {req:?}");
                };
                let () = responder.send(Ok(Some(fidl::endpoints::create_endpoints().0))).unwrap();
            }
            GetBehavior::ImmediateClose => {}
        }
    }
}

struct MockSpaceManagerService {
    call_count: Mutex<u32>,
    gc_err: Mutex<Option<fpkg_gc::GcError>>,
}

impl MockSpaceManagerService {
    fn new() -> Self {
        Self { call_count: Mutex::new(0), gc_err: Mutex::new(None) }
    }
    async fn run_service(
        self: Arc<Self>,
        mut stream: fpkg_gc::ManagerRequestStream,
    ) -> Result<(), Error> {
        while let Some(req) = stream.try_next().await? {
            *self.call_count.lock() += 1;

            match req {
                fpkg_gc::ManagerRequest::Gc { responder } => {
                    if let Some(e) = *self.gc_err.lock() {
                        responder.send(Err(e))?;
                    } else {
                        responder.send(Ok(()))?;
                    }
                }
                fpkg_gc::ManagerRequest::_UnknownMethod { .. } => {
                    panic!("unexpected request")
                }
            }
        }
        Ok(())
    }
}

fn assert_no_errors(output: &ProcessOutput) {
    assert!(
        output.is_ok(),
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.return_code(),
        output.stdout_str(),
        output.stderr_str()
    );
}

fn assert_stdout(output: &ProcessOutput, expected: &str) {
    assert_no_errors(output);
    assert_stdout_disregard_errors(output, expected);
}

fn assert_stdout_disregard_errors(output: &ProcessOutput, expected: &str) {
    assert_eq!(output.stdout_str(), expected, "{:?}", output.stderr_str());
}

fn assert_stderr(output: &ProcessOutput, expected: &str) {
    assert!(!output.is_ok());
    assert_eq!(output.stderr_str(), expected);
}

fn make_test_repo_config() -> RepositoryConfig {
    RepositoryConfigBuilder::new(
        RepositoryUrl::parse_host("example.com".to_string()).expect("valid url"),
    )
    .add_root_key(RepositoryKey::Ed25519(vec![0u8]))
    .add_mirror(
        MirrorConfigBuilder::new("http://example.org".parse::<Uri>().unwrap()).unwrap().build(),
    )
    .build()
}

#[fuchsia::test]
async fn test_repo() {
    let env = TestEnv::new();
    env.add_repository(
        RepositoryConfigBuilder::new(
            RepositoryUrl::parse_host("example.com".to_string()).expect("valid url"),
        )
        .build(),
    );

    let output = env.run_pkgctl(vec!["repo"]).await;

    assert_stdout(&output, "fuchsia-pkg://example.com\n");
    env.assert_only_repository_manager_called_with(vec![CapturedRepositoryManagerRequest::List]);
}

#[fuchsia::test]
async fn test_repo_show() {
    let env = TestEnv::new();

    // Add two repos, then verify we can get the details from the one we request.
    env.add_repository(
        RepositoryConfigBuilder::new(
            RepositoryUrl::parse_host("z.com".to_string()).expect("valid url"),
        )
        .build(),
    );
    let repo = RepositoryConfigBuilder::new(
        RepositoryUrl::parse_host("a.com".to_string()).expect("valid url"),
    )
    .build();
    env.add_repository(repo.clone());

    // The JSON conversion does not contain the trailing newline that we get when actually running
    // the command on the command line.
    let expected = serde_json::to_string_pretty(&repo).expect("valid json") + "\n";
    let output = env.run_pkgctl(vec!["repo", "show", "fuchsia-pkg://a.com"]).await;

    assert_stdout(&output, &expected);
    env.assert_only_repository_manager_called_with(vec![CapturedRepositoryManagerRequest::List]);
}

#[fuchsia::test]
async fn test_repo_sorts_lines() {
    let env = TestEnv::new();
    env.add_repository(
        RepositoryConfigBuilder::new(
            RepositoryUrl::parse_host("z.com".to_string()).expect("valid url"),
        )
        .build(),
    );
    env.add_repository(
        RepositoryConfigBuilder::new(
            RepositoryUrl::parse_host("a.com".to_string()).expect("valid url"),
        )
        .build(),
    );

    let output = env.run_pkgctl(vec!["repo"]).await;

    assert_stdout(&output, "fuchsia-pkg://a.com\nfuchsia-pkg://z.com\n");
}

macro_rules! repo_verbose_tests {
    ($($test_name:ident: $flag:expr,)*) => {
        $(
            #[fuchsia::test]
            async fn $test_name() {
                let env = TestEnv::new();
                let repo_config = make_test_repo_config();
                env.add_repository(repo_config.clone());

                let output = env.run_pkgctl(vec!["repo", $flag]).await;

                assert_no_errors(&output);
                let round_trip_repo_configs: Vec<RepositoryConfig> =
                    serde_json::from_slice(output.stdout.as_slice()).expect("valid json");
                assert_eq!(round_trip_repo_configs, vec![repo_config]);
                env.assert_only_repository_manager_called_with(vec![CapturedRepositoryManagerRequest::List]);
            }
        )*
    }
}

repo_verbose_tests! {
    test_repo_verbose_short: "-v",
    test_repo_verbose_long: "--verbose",
}

#[fuchsia::test]
async fn test_repo_rm() {
    let env = TestEnv::new();

    let output = env.run_pkgctl(vec!["repo", "rm", "the-url"]).await;

    assert_stdout(&output, "");
    env.assert_only_repository_manager_called_with(vec![
        CapturedRepositoryManagerRequest::Remove { repo_url: "the-url".to_string() },
    ]);
}

#[fuchsia::test]
async fn test_dump_dynamic() {
    let env = TestEnv::new();
    env.add_rule(Rule::new("fuchsia.com", "test", "/", "/").unwrap());
    let output = env.run_pkgctl(vec!["rule", "dump-dynamic"]).await;
    let expected_value = serde_json::json!({
        "version": "1",
        "content": [
            {
            "host_match": "fuchsia.com",
            "host_replacement": "test",
            "path_prefix_match": "/",
            "path_prefix_replacement": "/",
            }
        ]
    });
    assert_no_errors(&output);
    let actual_value: serde_json::value::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(expected_value, actual_value);
    env.assert_only_engine_called_with(vec![CapturedEngineRequest::StartEditTransaction]);
}

macro_rules! repo_add_tests {
    ($($test_name:ident: $source:expr, $name:expr,)*) => {
        $(
            #[fuchsia::test]
            async fn $test_name() {
                let env = TestEnv::new();

                let repo_config =  make_test_repo_config();

                let output = match $source {
                    "file" => {
                        let f = File::create(env.repo_config_arg_path.join("the-config")).expect("create repo config file");
                        serde_json::to_writer(f, &repo_config).expect("write RepositoryConfig json");
                        let args = if $name == "" {
                            vec!["repo", "add", $source, "/repo-configs/the-config"]
                        } else {
                            vec!["repo", "add", $source, "-n", $name, "/repo-configs/the-config"]
                        };
                        env.run_pkgctl(args).await
                    },
                    "url" => {
                        let response = StaticResponse::ok_body(serde_json::to_string(&repo_config).unwrap());
                        let server = TestServer::builder().handler(response).start().await;
                        let local_url = server.local_url_for_path("some/path").to_owned();
                        let args = if $name == "" {
                            vec!["repo", "add", $source, &local_url]
                        } else {
                            vec!["repo", "add", $source, "-n", $name, &local_url]
                        };
                        env.run_pkgctl(args).await
                    },
                    // Unsupported source
                    _ => env.run_pkgctl(vec!["repo", "add", $source]).await,
                };

                assert_stdout(&output, "");
                env.assert_only_repository_manager_called_with(vec![CapturedRepositoryManagerRequest::Add {
                    repo: repo_config,
                }]);
            }
        )*
    }
}

repo_add_tests! {
    test_repo_add_file: "file", "",
    test_repo_add_file_with_name: "file", "example.com",
    test_repo_add_url: "url", "",
    test_repo_add_url_with_name: "url", "example.com",
}

#[fuchsia::test]
async fn test_gc_success() {
    let env = TestEnv::new();
    *env.space_manager.gc_err.lock() = None;
    let output = env.run_pkgctl(vec!["gc"]).await;
    assert!(output.is_ok());
    env.assert_only_space_manager_called();
}

#[fuchsia::test]
async fn test_gc_error() {
    let env = TestEnv::new();
    *env.space_manager.gc_err.lock() = Some(fpkg_gc::GcError::Internal);
    let output = env.run_pkgctl(vec!["gc"]).await;
    assert!(!output.is_ok());
    env.assert_only_space_manager_called();
}

#[fuchsia::test]
async fn test_experiment_enable_no_admin_service() {
    let env = TestEnv::new();
    let output = env.run_pkgctl(vec!["experiment", "enable", "lightbulb"]).await;
    assert_eq!(output.return_code(), 1);

    // Call another pkgctl command to confirm the tool still works.
    let output = env.run_pkgctl(vec!["gc"]).await;
    assert!(output.is_ok());
}

#[fuchsia::test]
async fn test_experiment_disable_no_admin_service() {
    let env = TestEnv::new();
    let output = env.run_pkgctl(vec!["experiment", "enable", "lightbulb"]).await;
    assert_eq!(output.return_code(), 1);
}

#[fuchsia::test]
async fn test_get_hash_success() {
    let hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let env = TestEnv::new();
    env.package_resolver
        .get_hash_response
        .lock()
        .replace(Ok(hash.parse::<fidl_fuchsia_pkg_ext::BlobId>().unwrap().into()));

    let output = env.run_pkgctl(vec!["get-hash", "the-url"]).await;

    assert_stdout(&output, &(hash.to_owned() + "\n"));
    env.assert_only_package_resolver_called_with(vec![CapturedPackageResolverRequest::GetHash {
        package_url: "the-url".into(),
    }]);
}

#[fuchsia::test]
async fn test_get_hash_failure() {
    let env = TestEnv::new();
    env.package_resolver.get_hash_response.lock().replace(Err(Status::UNAVAILABLE));

    let output = env.run_pkgctl(vec!["get-hash", "the-url"]).await;

    assert_stderr(&output, "Error: Failed to get package hash with error: UNAVAILABLE\n");
    env.assert_only_package_resolver_called_with(vec![CapturedPackageResolverRequest::GetHash {
        package_url: "the-url".into(),
    }]);
}

// TODO(https://fxbug.dev/552675412): Delete the pkg-status subcommand.
#[ignore]
#[fuchsia::test]
async fn test_pkg_status_success() {
    let hash: fidl_fuchsia_pkg::BlobId =
        "0000000000000000000000000000000000000000000000000000000000000000"
            .parse::<fidl_fuchsia_pkg_ext::BlobId>()
            .unwrap()
            .into();
    let env = TestEnv::new();
    env.package_resolver.get_hash_response.lock().replace(Ok(hash));
    *env.package_cache.get_behavior.lock() = GetBehavior::AlreadyCached;

    let output = env.run_pkgctl(vec!["pkg-status", "the-url"]).await;

    assert_stdout(
        &output,
        "Package in registered TUF repo: yes (merkle=0000000000000000000000000000000000000000000000000000000000000000)\n\
      Package on disk: yes\n",
    );
    env.assert_only_package_resolver_and_package_cache_called_with(
        vec![CapturedPackageResolverRequest::GetHash { package_url: "the-url".into() }],
        vec![CapturedPackageCacheRequest::Get {
            meta_far_blob_id: fpkg::BlobInfo { blob_id: hash, length: 0 },
        }],
    );
}

// TODO(https://fxbug.dev/552675412): Delete the pkg-status subcommand.
#[ignore]
#[fuchsia::test]
async fn test_pkg_status_fail_pkg_in_tuf_repo_but_not_on_disk() {
    let hash: fidl_fuchsia_pkg::BlobId =
        "0000000000000000000000000000000000000000000000000000000000000000"
            .parse::<fidl_fuchsia_pkg_ext::BlobId>()
            .unwrap()
            .into();
    let env = TestEnv::new();
    env.package_resolver.get_hash_response.lock().replace(Ok(hash));
    *env.package_cache.get_behavior.lock() = GetBehavior::NotCached;

    let output = env.run_pkgctl(vec!["pkg-status", "the-url"]).await;

    assert_stdout_disregard_errors(
        &output,
        "Package in registered TUF repo: yes (merkle=0000000000000000000000000000000000000000000000000000000000000000)\n\
      Package on disk: no\n",
    );
    assert_eq!(output.return_code(), 2);
    env.assert_only_package_resolver_and_package_cache_called_with(
        vec![CapturedPackageResolverRequest::GetHash { package_url: "the-url".into() }],
        vec![CapturedPackageCacheRequest::Get {
            meta_far_blob_id: fpkg::BlobInfo { blob_id: hash, length: 0 },
        }],
    );
}

// TODO(https://fxbug.dev/552675412): Delete the pkg-status subcommand.
#[ignore]
#[fuchsia::test]
async fn test_pkg_status_fail_pkg_not_in_tuf_repo() {
    let env = TestEnv::new();
    env.package_resolver.get_hash_response.lock().replace(Err(Status::NOT_FOUND));

    let output = env.run_pkgctl(vec!["pkg-status", "the-url"]).await;

    assert_stdout_disregard_errors(
        &output,
        "Package in registered TUF repo: no\n\
        Package on disk: unknown (did not check since not in tuf repo)\n",
    );
    assert_eq!(output.return_code(), 3);
    env.assert_only_package_resolver_called_with(vec![CapturedPackageResolverRequest::GetHash {
        package_url: "the-url".into(),
    }]);
}

#[fuchsia::test]
async fn test_resolve() {
    let env = TestEnv::new();
    env.package_resolver.resolve_response.lock().replace((
        vfs::pseudo_directory! { "meta" => vfs::pseudo_directory! {} },
        Ok(fpkg::ResolutionContext { bytes: vec![] }),
    ));

    let output = env.run_pkgctl(vec!["resolve", "the-url"]).await;

    assert_stdout(&output, "resolving the-url\n");

    env.assert_only_package_resolver_called_with(vec![CapturedPackageResolverRequest::Resolve {
        package_url: "the-url".into(),
    }]);
}

#[fuchsia::test]
async fn test_resolve_verbose() {
    let env = TestEnv::new();
    env.package_resolver.resolve_response.lock().replace((
        vfs::pseudo_directory! { "meta" => vfs::pseudo_directory! {} },
        Ok(fpkg::ResolutionContext { bytes: vec![] }),
    ));

    let output = env.run_pkgctl(vec!["resolve", "the-url", "--verbose"]).await;

    assert_stdout(&output, "resolving the-url\npackage contents:\n/meta\n");

    env.assert_only_package_resolver_called_with(vec![CapturedPackageResolverRequest::Resolve {
        package_url: "the-url".into(),
    }]);
}
