// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::args::{
    FuzzCtlCommand, FuzzCtlSubcommand, ResetSubcommand, ResumeLibFuzzerSubcommand,
    RunLibFuzzerSubcommand,
};
use anyhow::{Context as _, Result, anyhow, bail};
use argh::FromArgs;
use fidl_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
use fuchsia_fuzzctl::{Controller, InputPair, Manager, OutputSink, Writer, save_artifact};
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::vec::IntoIter as VecIter;
use url::Url;
use walkdir::WalkDir;

pub struct FuzzCtl<O: OutputSink> {
    manager: Manager,
    output_dir: PathBuf,
    writer: Writer<O>,
}

impl<O: OutputSink> FuzzCtl<O> {
    /// Returns a new `FuzzCtl`.
    ///
    /// The object will communicate with the fuzz-manager using the given `proxy`. It will forward
    /// logs using the given `writer`, and save logs and artifacts produced by a fuzzer under the
    /// given `output_dir`.
    ///
    pub fn new<P: AsRef<Path>>(
        proxy: fuzz::ManagerProxy,
        output_dir: P,
        writer: &Writer<O>,
    ) -> Self {
        Self {
            manager: Manager::new(proxy),
            output_dir: PathBuf::from(output_dir.as_ref()),
            writer: writer.clone(),
        }
    }

    /// Parses command line arguments and executes commands based on them.
    pub async fn run(&self, args: &Vec<String>) -> Result<()> {
        let libfuzzer_re = Regex::new(r"-([^=\-\s][^=\s]*)=(.*)").expect("failed to compile regex");
        let mut libfuzzer_args = Vec::new();
        for arg in args {
            match libfuzzer_re.captures(&arg) {
                Some(captures) => {
                    libfuzzer_args
                        .push(format!("--{}", captures.get(1).unwrap().as_str()).replace("_", "-"));
                    libfuzzer_args.push(captures.get(2).unwrap().as_str().to_string());
                }
                None => libfuzzer_args.push(arg.to_string()),
            }
        }

        let args: Vec<&str> = libfuzzer_args.iter().map(|s| s.as_str()).collect();
        let args = match FuzzCtlCommand::from_args(&[""], args.as_slice()) {
            Ok(args) => args,
            Err(argh::EarlyExit { output, status }) => {
                return match status {
                    Ok(_) => {
                        // User provided '-h' or '--help'.
                        self.writer.println(format!("{}", output));
                        Ok(())
                    }
                    Err(e) => Err(anyhow!("{:?}: output: {}", e, output)),
                };
            }
        };
        let (result, url) = match args.command {
            FuzzCtlSubcommand::Reset(cmd) => (self.reset(&cmd).await, cmd.url),
            FuzzCtlSubcommand::RunLibFuzzer(cmd) => (self.run_libfuzzer(&cmd).await, cmd.url),
            FuzzCtlSubcommand::ResumeLibFuzzer(cmd) => (self.resume_libfuzzer(&cmd).await, cmd.url),
        };
        if result.is_err() {
            // Make a best effort to teardown a fuzzer that returns an error.
            if let Err(e) = self.manager.stop(&url).await {
                self.writer.error(format!("failed to stop fuzzer on error: {}", e));
            }
        }
        result
    }

    // Stops and restarts a fuzzer component.
    async fn reset(&self, cmd: &ResetSubcommand) -> Result<()> {
        // Ensure the fuzzer is stopped or was never started.
        self.manager.stop(&cmd.url).await?;

        // Reset local data files. Ignore errors from removing non-existent directories.
        let fuzzer_dir = self.get_fuzzer_dir(&cmd.url).context("failed to get fuzzer directory")?;
        let _ = fs::remove_dir_all(&fuzzer_dir);
        fs::create_dir_all(&fuzzer_dir).with_context(|| {
            format!("failed to create directory: '{}'", fuzzer_dir.to_string_lossy())
        })?;

        // Ensure the fuzzer is fully resolved by connecting to it.
        self.manager.connect(&cmd.url).await?;
        Ok(())
    }

    // Performs fuzzing actions based on libFuzzer-style command-line arguments.
    async fn run_libfuzzer(&self, cmd: &RunLibFuzzerSubcommand) -> Result<()> {
        let fuzzer_dir = self.get_fuzzer_dir(&cmd.url).context("failed to get fuzzer directory")?;
        let controller = self
            .get_controller(&cmd.url, &cmd.forward.0)
            .await
            .context("failed to get fuzzer controller")?;
        let mut workflow =
            LibFuzzerWorkflow::new(&fuzzer_dir, &cmd.exact_artifact_path, controller, &self.writer);
        workflow.run(cmd).await
    }

    // Completes fuzzing actions started by a previous instance of fuzz_ctl. If a connection to a
    // fuzzer fails, this method can be used to reconnect and complete any long-running workflows.
    async fn resume_libfuzzer(&self, cmd: &ResumeLibFuzzerSubcommand) -> Result<()> {
        let fuzzer_dir = self.get_fuzzer_dir(&cmd.url).context("failed to get fuzzer directory")?;
        let controller = self
            .get_controller(&cmd.url, &cmd.forward.0)
            .await
            .context("failed to get fuzzer controller")?;
        let workflow =
            LibFuzzerWorkflow::new(&fuzzer_dir, &cmd.exact_artifact_path, controller, &self.writer);
        workflow.resume().await
    }

    // Returns a directory corresponding to a fuzzer URL.
    //
    // A fuzzer URL like:
    //    "fuchsia-pkg://fuchsia.com/my-fuzzers#meta/my-fuzzer.cm"
    //
    // will map to:
    //    ${self.output_dir}/fuchsia.com/my-fuzzers/my-fuzzer/
    //
    fn get_fuzzer_dir(&self, url: &Url) -> Result<PathBuf> {
        let domain = url.host_str().filter(|&d| d != "").with_context(|| {
            format!("invalid Fuchsia package URL: missing repository: {}", url.as_str())
        })?;
        let mut path = self.output_dir.join(domain);

        // Convert an `Option<Split>` to `Vec<String>` where each string is non-empty.
        let segments = url
            .path_segments()
            .map(|s| s.collect::<Vec<_>>())
            .unwrap_or(Vec::new())
            .into_iter()
            .filter_map(|s| match s {
                "" => None,
                s => Some(s.to_string()),
            })
            .collect::<Vec<_>>();
        if segments.is_empty() {
            bail!("invalid Fuchsia package URL: missing package name: {}", url.as_str());
        }
        path.extend(segments.into_iter());

        let fragment = url.fragment().filter(|&d| d != "").with_context(|| {
            format!("invalid Fuchsia package URL: missing resource path: {}", url.as_str())
        })?;
        let fragment = Path::new(fragment);
        let fragment = fragment.file_stem().context("unable to determine last path segment")?;
        path.push(fragment);

        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create directory: '{}'", path.to_string_lossy()))?;
        Ok(path)
    }

    // Returns a `Controller` connected to the fuzzer given by the `url`.
    async fn get_controller(
        &self,
        url: &Url,
        forward: &Vec<fuzz::TestOutput>,
    ) -> Result<Controller<O>> {
        let proxy = self.manager.connect(url).await?;
        let mut controller = Controller::new(proxy, &self.writer);
        let fuzzer_dir = self.get_fuzzer_dir(&url)?;
        for output in forward.iter() {
            let socket = self.manager.get_output(url, *output).await?;
            controller.set_output(socket, *output, &Some(fuzzer_dir.clone()))?;
        }
        Ok(controller)
    }
}

struct LibFuzzerPathBuf {
    base: PathBuf,
}

impl LibFuzzerPathBuf {
    fn new<P: AsRef<Path>>(path: P) -> Self {
        Self { base: path.as_ref().to_path_buf() }
    }

    // Returns an absolute path for the given `relpath` with 'tmp' replaced by the fuzzer directory.
    fn abspath<P: AsRef<Path>>(&self, relpath: P) -> Result<PathBuf> {
        let relpath = relpath.as_ref();
        if relpath.starts_with(&self.base) {
            // Already absolute.
            return Ok(PathBuf::from(relpath));
        }
        let path = relpath.strip_prefix("tmp").with_context(|| {
            format!("data paths must be relative to 'tmp/': {}", relpath.to_string_lossy())
        })?;
        Ok(self.base.join(PathBuf::from(path)))
    }

    // Returns a relative path for the given `abspath` with the fuzzer directory replaced by 'tmp'.
    fn relpath<P: AsRef<Path>>(&self, abspath: P) -> Result<PathBuf> {
        let abspath = abspath.as_ref();
        if abspath.starts_with("tmp/") {
            // Already relative.
            return Ok(PathBuf::from(abspath));
        }
        let path = abspath.strip_prefix(&self.base).with_context(|| {
            format!("'{}' is outside fuzzer directory", abspath.to_string_lossy())
        })?;
        Ok(PathBuf::from("tmp").join(path))
    }

    fn as_path_ref(&self) -> &Path {
        &self.base.as_path()
    }
}

// Holds paths converted from fuzz_ctl's positional data path arguments.
struct LibFuzzerWorkflow<O: OutputSink> {
    fuzzer_dir: LibFuzzerPathBuf,
    controller: Controller<O>,
    dirs: Vec<PathBuf>,
    files: VecIter<PathBuf>,
    num_dirs: usize,
    num_files: usize,
    output_corpus: Option<PathBuf>,
    exact_artifact_path: Option<PathBuf>,
    writer: Writer<O>,
}

impl<O: OutputSink> LibFuzzerWorkflow<O> {
    fn new<P: AsRef<Path>>(
        fuzzer_dir: P,
        exact_artifact_path: &Option<String>,
        controller: Controller<O>,
        writer: &Writer<O>,
    ) -> Self {
        Self {
            fuzzer_dir: LibFuzzerPathBuf::new(fuzzer_dir),
            controller,
            dirs: Vec::new(),
            files: Vec::new().into_iter(),
            num_dirs: 0,
            num_files: 0,
            output_corpus: None,
            exact_artifact_path: exact_artifact_path.as_ref().map(|s| PathBuf::from(s)),
            writer: writer.clone(),
        }
    }

    // Executes the given `cmd`.
    async fn run(&mut self, cmd: &RunLibFuzzerSubcommand) -> Result<()> {
        let mut files: Vec<PathBuf> = Vec::new();
        for relpath in cmd.data.iter() {
            let abspath = self.fuzzer_dir.abspath(&relpath)?;
            if abspath.is_dir() {
                self.dirs.push(relpath.into());
            } else if abspath.is_file() {
                files.push(relpath.into());
            } else {
                bail!("no such data path: {}", relpath);
            }
        }
        self.num_dirs = self.dirs.len();
        self.num_files = files.len();
        self.files = files.into_iter();
        if self.num_dirs != 0 && self.num_files != 0 {
            bail!("data paths must be files or directories, but not both");
        }

        if cmd.minimize_crash && cmd.merge {
            bail!("cannot specify both '-minimize_crash=1' and '-merge=1'");
        }

        let options = cmd.get_options();
        self.controller.configure(options).await.context("failed to configure fuzzer")?;

        // Minimize
        if cmd.minimize_crash {
            if self.num_files != 1 {
                bail!("'minimize_crash' expects exactly 1 file");
            }
            let input_pair = self.take_test_input()?;
            self.controller
                .minimize(input_pair)
                .await
                .context("failed to minimize fuzzer input")?;
            self.finish().await?;
            return Ok(());
        }

        // Merge
        if cmd.merge {
            if self.num_dirs < 2 {
                bail!("'merge' expects 2 or more directories");
            }
            let input_pairs = self.take_corpora().context("failed to add corpora to be merged")?;
            self.controller
                .add_to_corpus(input_pairs, fuzz::Corpus::Live)
                .await
                .context("failed to add inputs to corpus")?;
            self.controller.merge().await.context("failed to merge fuzzer corpora")?;
            self.finish().await?;
            self.read_corpus().await?;
            return Ok(());
        }

        // TryOne
        if self.num_files != 0 {
            while let Ok(input_pair) = self.take_test_input() {
                self.controller.try_one(input_pair).await.context("failed to try fuzzer input")?;
                let result = self.finish().await?;
                if result != FuzzResult::NoErrors {
                    break;
                }
            }
            return Ok(());
        }

        // Fuzz
        let input_pairs = self.take_corpora().context("failed to add corpora to be fuzzed")?;
        self.controller
            .add_to_corpus(input_pairs, fuzz::Corpus::Live)
            .await
            .context("failed to add inputs to corpus")?;
        self.controller.fuzz().await.context("failed to fuzz")?;
        self.finish().await?;
        self.read_corpus().await?;
        return Ok(());
    }

    // Resumes waiting for a fuzzer to complete a long-running workflow. This is useful when
    // reconnecting to a fuzzer controller.
    async fn resume(&self) -> Result<()> {
        self.controller.reset_timer().await?;
        self.finish().await?;
        Ok(())
    }

    // Finds and converts all the inputs in a given list of directories to input pairs.
    //
    // Prints the relative path to the directory where the final corpus will be saved. This path is
    // used by undercoat.
    //
    // Returns the absolute path to the first directory, and the accumulated input pairs to be sent.
    //
    fn take_corpora(&mut self) -> Result<Vec<InputPair>> {
        let first = self.dirs.first().cloned();
        let mut input_pairs = Vec::new();
        let dirs: Vec<_> = self.dirs.drain(..).collect();
        for relpath in dirs {
            let dir = self.fuzzer_dir.abspath(relpath).unwrap();
            for entry in WalkDir::new(dir).follow_links(true).into_iter().filter_map(|e| e.ok()) {
                if !entry.file_type().is_file() {
                    continue;
                }
                let file = entry.path();
                let input_pair = InputPair::try_from_path(&self.controller.domain(), file)
                    .with_context(|| {
                        format!("failed to input from '{}'", file.to_string_lossy())
                    })?;
                input_pairs.push(input_pair);
            }
        }

        let first = first.unwrap_or_else(|| PathBuf::from("tmp/corpus"));
        self.writer.println(format!("Using '{}' as the output corpus.", first.to_string_lossy()));
        self.output_corpus = self.fuzzer_dir.abspath(first).ok();
        Ok(input_pairs)
    }

    // Converts a file to an input pair to be used as a test case for workflows like `minimize`.
    //
    // Prints the path to the file being used. This path is used by undercoat.
    //
    fn take_test_input(&mut self) -> Result<InputPair> {
        let relpath = self.files.next().context("no remaining test input files")?;
        self.writer.println(format!("Using '{}' as the test input.", relpath.to_string_lossy()));
        InputPair::try_from_path(
            &self.controller.domain(),
            self.fuzzer_dir.abspath(relpath).unwrap(),
        )
    }

    // Waits for the controller to produce an fuzzing artifact, and reports where data associated
    // with it has been stored.
    //
    // If `self.exact_artifact_path` is not `None`, it renames the artifact data file to that path.
    //
    // Returns an error if renaming is needed but fails.
    //
    async fn finish(&self) -> Result<FuzzResult> {
        let fidl_artifact = self.controller.watch_artifact().await?;
        let artifact = save_artifact(fidl_artifact, self.fuzzer_dir.as_path_ref()).await?;
        if artifact.is_none() {
            self.writer.println("Workflow was canceled.");
            return Ok(FuzzResult::NoErrors);
        }
        let artifact = artifact.unwrap();
        let mut input_path = match artifact.path {
            Some(path) => path,
            None => return Ok(artifact.result),
        };
        match &self.exact_artifact_path {
            Some(exact_artifact_path) => {
                let src = self.fuzzer_dir.abspath(&input_path)?;
                input_path = PathBuf::from(exact_artifact_path);
                let dst = self
                    .fuzzer_dir
                    .abspath(&input_path)
                    .context("invalid `exact_artifact_path`")?;
                fs::rename(&src, &dst).with_context(|| {
                    format!(
                        "failed to rename {} to {}",
                        src.to_string_lossy(),
                        dst.to_string_lossy()
                    )
                })?;
            }
            None => {
                input_path = self.fuzzer_dir.relpath(input_path)?;
            }
        };
        let input_path = input_path.to_string_lossy().to_string();
        let prologue = match artifact.result {
            FuzzResult::NoErrors => return Ok(artifact.result),
            FuzzResult::Cleansed => "Cleansed input written",
            FuzzResult::Minimized => "Minimized input written",
            _ => "Input saved",
        };
        self.writer.println(format!("{} to '{}'.", prologue, input_path));
        Ok(artifact.result)
    }

    // Updates the local copy of the live corpus to match the inputs in the test realm.
    async fn read_corpus(&self) -> Result<()> {
        let output_corpus = self.output_corpus.as_ref().unwrap();
        self.controller
            .read_corpus(fuzz::Corpus::Live, output_corpus)
            .await
            .context("failed to read corpus")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{FuzzCtl, LibFuzzerPathBuf};
    use anyhow::Result;
    use fidl::endpoints::create_proxy;
    use fidl_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
    use fuchsia_async as fasync;
    use fuchsia_fuzzctl::constants::*;
    use fuchsia_fuzzctl::{OutputSink, digest_path};
    use fuchsia_fuzzctl_test::{BufferSink, TEST_URL, Test, create_task, serve_manager};
    use std::path::PathBuf;
    use url::Url;

    // Test fixtures.

    fn perform_setup() -> Result<(Test, FuzzCtl<BufferSink>, LibFuzzerPathBuf, fasync::Task<()>)> {
        let test = Test::try_new()?;
        let (proxy, server_end) = create_proxy::<fuzz::ManagerMarker>();
        let fuzz_ctl = FuzzCtl::new(proxy, test.root_dir(), test.writer());
        let task = create_task(serve_manager(server_end, test.clone()), test.writer());

        let url = Url::parse(TEST_URL)?;
        let fuzzer_dir = fuzz_ctl.get_fuzzer_dir(&url)?;
        let fuzzer_dir = LibFuzzerPathBuf::new(&fuzzer_dir);
        let abspath = fuzzer_dir.abspath("tmp/corpus1")?;
        let corpus1 = test.create_dir(abspath)?;
        let abspath = fuzzer_dir.abspath("tmp/corpus2")?;
        test.create_dir(abspath)?;
        test.create_test_files(&corpus1, vec!["hello", "world"].iter())?;

        Ok((test, fuzz_ctl, fuzzer_dir, task))
    }

    async fn run_cmd<O: OutputSink>(fuzz_ctl: &FuzzCtl<O>, cmdline: &Vec<&str>, test: &Test) {
        let args: Vec<String> = cmdline.join(" ").split(' ').map(|s| s.to_string()).collect();
        if let Err(e) = fuzz_ctl.run(&args).await {
            test.writer().error(format!("{:?}", e));
        }
    }

    // Unit tests.

    #[fuchsia::test]
    async fn test_reset_missing_url() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["reset"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("positional arguments not provided");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_reset_invalid_urls() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["reset", "bad_url"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("relative URL without a base");

        let cmdline = vec!["reset", "fuchsia-pkg://"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("invalid Fuchsia package URL: missing repository");

        let cmdline = vec!["reset", "fuchsia-pkg://fuchsia.com"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("invalid Fuchsia package URL: missing package name");

        let cmdline = vec!["reset", "fuchsia-pkg://fuchsia.com/my-fuzzers"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("invalid Fuchsia package URL: missing resource path");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_reset() -> Result<()> {
        let (mut test, fuzz_ctl, fuzzer_dir, _task) = perform_setup()?;

        // Fuzzer-related files should be cleared and requests made to the fuzz-manager.
        let hello = fuzzer_dir.abspath("tmp/corpus1/hello")?;
        let world = fuzzer_dir.abspath("tmp/corpus1/world")?;
        assert!(hello.exists());
        assert!(world.exists());

        let _ = test.requests();
        let cmdline = vec!["reset", TEST_URL];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        assert_eq!(*test.url().borrow(), Some(TEST_URL.to_string()));

        assert!(!hello.exists());
        assert!(!world.exists());

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Stop({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_missing_url() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("positional arguments not provided");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_mixed_data_paths() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1/hello", "tmp/corpus2"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("data paths must be files or directories, but not both");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_invalid_data_paths() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "pkg/data/hello"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("data paths must be relative to 'tmp/': ");

        let cmdline = vec!["run_libfuzzer", TEST_URL, "pkg/data/corpus1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("data paths must be relative to 'tmp/': ");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_invalid_argh_options() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "--nonsense"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("Unrecognized argument: --nonsense");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_parse_argh_options() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();
        fake.set_result(Ok(FuzzResult::NoErrors));

        let mut cmdline = vec!["run_libfuzzer", TEST_URL];
        cmdline.push("--runs 1");
        cmdline.push("--seed 2");
        cmdline.push("--mutate-depth 3");
        cmdline.push("--timeout 4.0");
        cmdline.push("--rss-limit-mb 5.0");
        cmdline.push("--print-final-stats 1");

        test.output_matches("Using 'tmp/corpus' as the output corpus.");
        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let options = fake.get_options();
        assert_eq!(options.runs, Some(1));
        assert_eq!(options.seed, Some(2));
        assert_eq!(options.mutation_depth, Some(3));
        assert_eq!(options.run_limit, Some(4 * NANOS_PER_SECOND));
        assert_eq!(options.oom_limit, Some(5 * BYTES_PER_MB));
        assert_eq!(options.print_final_stats, Some(true));

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_invalid_options() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "-nonsense=1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("Unrecognized argument: --nonsense");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_parse_options() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let mut cmdline = vec!["run_libfuzzer", TEST_URL];
        cmdline.push("-max_total_time=10.0");
        cmdline.push("-max_len=20.0");
        cmdline.push("-detect_leaks=0");
        cmdline.push("-malloc_limit_mb=30.0");
        cmdline.push("-purge_allocator_interval=40.0");
        cmdline.push("-use_value_profile=1");

        test.output_matches("Using 'tmp/corpus' as the output corpus.");
        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let options = fake.get_options();
        assert_eq!(options.max_total_time, Some(10 * NANOS_PER_SECOND));
        assert_eq!(options.max_input_size, Some(20));
        assert_eq!(options.detect_leaks, Some(false));
        assert_eq!(options.malloc_limit, Some(30 * BYTES_PER_MB));
        assert_eq!(options.purge_interval, Some(40 * NANOS_PER_SECOND));
        assert_eq!(options.use_value_profile, Some(true));

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_fuzz_error() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        fake.set_result(Err(zx::Status::INTERNAL));
        let cmdline = vec!["run_libfuzzer", TEST_URL];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("failed to fuzz");

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.Fuzz".to_string(),
            format!("fuchsia.fuzzer/Manager.Stop({})", TEST_URL),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_fuzz_default_options() -> Result<()> {
        let (mut test, fuzz_ctl, fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        fake.set_result(Ok(FuzzResult::Crash));
        fake.set_input_to_send(b"crash");

        let cmdline = vec!["run_libfuzzer", TEST_URL];
        test.output_matches("Using 'tmp/corpus' as the output corpus.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let input_path = digest_path(fuzzer_dir.as_path_ref(), Some(FuzzResult::Crash), b"crash");
        let input_path = input_path.strip_prefix(fuzzer_dir.as_path_ref())?;
        let input_path = PathBuf::from("tmp").join(input_path);
        test.output_matches(format!("Input saved to '{}'.", input_path.to_string_lossy()));

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.Fuzz".to_string(),
            "fuchsia.fuzzer/Controller.ReadCorpus".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_fuzz_directories() -> Result<()> {
        let (mut test, fuzz_ctl, fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        fake.set_result(Ok(FuzzResult::Oom));
        fake.set_input_to_send(b"oom");

        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1", "tmp/corpus2"];
        test.output_matches("Using 'tmp/corpus1' as the output corpus.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let input_path = digest_path(fuzzer_dir.as_path_ref(), Some(FuzzResult::Oom), b"oom");
        let input_path = input_path.strip_prefix(fuzzer_dir.as_path_ref())?;
        let input_path = PathBuf::from("tmp").join(input_path);
        test.output_matches(format!("Input saved to '{}'.", input_path.to_string_lossy()));

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.AddToCorpus(Live)".to_string(),
            "fuchsia.fuzzer/Controller.AddToCorpus(Live)".to_string(),
            "fuchsia.fuzzer/Controller.Fuzz".to_string(),
            "fuchsia.fuzzer/Controller.ReadCorpus".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_fuzz_exact_artifact_path() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        fake.set_result(Ok(FuzzResult::Death));
        fake.set_input_to_send(b"death");

        let cmdline = vec!["run_libfuzzer", TEST_URL, "-exact_artifact_path=tmp/artifact"];
        test.output_matches("Using 'tmp/corpus' as the output corpus.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        test.output_matches("Input saved to 'tmp/artifact'.");

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.Fuzz".to_string(),
            "fuchsia.fuzzer/Controller.ReadCorpus".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_try_one() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1/hello"];
        test.output_matches("Using 'tmp/corpus1/hello' as the test input.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let data = fake.get_received_input();
        assert_eq!(data, b"hello");

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.TryOne".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_try_multiple() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1/hello", "tmp/corpus1/world"];
        test.output_matches("Using 'tmp/corpus1/hello' as the test input.");
        test.output_matches("Using 'tmp/corpus1/world' as the test input.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let data = fake.get_received_input();
        assert_eq!(data, b"world");

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.TryOne".to_string(),
            "fuchsia.fuzzer/Controller.TryOne".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_minimize_directory() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1", "--minimize-crash 1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("'minimize_crash' expects exactly 1 file");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_minimize_multiple_files() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec![
            "run_libfuzzer",
            TEST_URL,
            "tmp/corpus1/hello",
            "tmp/corpus1/world",
            "-minimize_crash=1",
        ];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("'minimize_crash' expects exactly 1 file");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_minimize_one_file() -> Result<()> {
        let (mut test, fuzz_ctl, fuzzer_dir, _task) = perform_setup()?;
        let fake = test.controller();

        let _ = test.requests();
        fake.set_input_to_send(b"minimized");

        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1/hello", "-minimize_crash=1"];
        test.output_matches("Using 'tmp/corpus1/hello' as the test input.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let input_path =
            digest_path(fuzzer_dir.as_path_ref(), Some(FuzzResult::Minimized), b"minimized");
        let input_path = input_path.strip_prefix(fuzzer_dir.as_path_ref())?;
        let input_path = PathBuf::from("tmp").join(input_path);
        test.output_matches(format!(
            "Minimized input written to '{}'.",
            input_path.to_string_lossy()
        ));

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.Minimize".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_merge_files() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline =
            vec!["run_libfuzzer", TEST_URL, "tmp/corpus1/hello", "tmp/corpus1/world", "-merge=1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("'merge' expects 2 or more directories");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_merge_one_directory() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1", "--merge 1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("'merge' expects 2 or more directories");

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_merge_directories() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let _ = test.requests();
        let cmdline = vec!["run_libfuzzer", TEST_URL, "tmp/corpus1", "tmp/corpus2", "--merge 1"];
        test.output_matches("Using 'tmp/corpus1' as the output corpus.");

        run_cmd(&fuzz_ctl, &cmdline, &test).await;

        let requests = vec![
            format!("fuchsia.fuzzer/Manager.Connect({})", TEST_URL),
            format!("fuchsia.fuzzer/Manager.GetOutput({}, Stderr)", TEST_URL),
            "fuchsia.fuzzer/Controller.Configure".to_string(),
            "fuchsia.fuzzer/Controller.AddToCorpus(Live)".to_string(),
            "fuchsia.fuzzer/Controller.AddToCorpus(Live)".to_string(),
            "fuchsia.fuzzer/Controller.Merge".to_string(),
            "fuchsia.fuzzer/Controller.ReadCorpus".to_string(),
        ];
        assert_eq!(test.requests(), requests);

        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run_libfuzzer_minimize_and_merge() -> Result<()> {
        let (mut test, fuzz_ctl, _fuzzer_dir, _task) = perform_setup()?;

        let cmdline = vec!["run_libfuzzer", TEST_URL, "--minimize-crash 1", "-merge=1"];
        run_cmd(&fuzz_ctl, &cmdline, &test).await;
        test.output_includes("cannot specify both '-minimize_crash=1' and '-merge=1'");

        test.verify_output()
    }
}
