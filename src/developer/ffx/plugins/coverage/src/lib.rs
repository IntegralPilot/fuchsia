// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, anyhow};
use ffx_coverage_args::CoverageCommand;
use ffx_writer::SimpleWriter;
use fho::{FfxMain, FfxTool};
use glob::glob;
use rayon::prelude::*;
#[cfg(test)]
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(test)]
use std::sync::Mutex;
use symbol_index::{SymbolIndex, global_symbol_index_path};
use tempfile::tempdir;

// The line found right above build ID in `llvm-profdata show --binary-ids` output.
#[cfg(not(test))]
const BINARY_ID_LINE: &str = "Binary IDs:";

/// A convenient struct grouping common parameters to export/show functions.
struct ExportParams<'a> {
    llvm_cov_bin: PathBuf,
    merged_profile: PathBuf,
    bin_files_args: Vec<&'a str>,
    src_files: Vec<PathBuf>,
    extra_args: Vec<&'a str>,
}

#[derive(FfxTool)]
pub struct CoverageTool {
    #[command]
    cmd: CoverageCommand,
}

fho::embedded_plugin!(CoverageTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for CoverageTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, _writer: Self::Writer) -> fho::Result<()> {
        coverage(self.cmd).await.map_err(Into::into)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum VerboseMode {
    NotVerbose,
    Verbose,
}

impl From<bool> for VerboseMode {
    fn from(v: bool) -> Self {
        if v { Self::Verbose } else { Self::NotVerbose }
    }
}

pub async fn coverage(cmd: CoverageCommand) -> Result<()> {
    let verbose_mode = VerboseMode::from(cmd.verbose);
    let clang_bin_dir = cmd.clang_dir.join("bin");
    let llvm_profdata_bin = clang_bin_dir.join("llvm-profdata");
    llvm_profdata_bin
        .exists()
        .then_some(())
        .ok_or_else(|| anyhow!("{:?} does not exist", llvm_profdata_bin))?;
    let llvm_cov_bin = clang_bin_dir.join("llvm-cov");
    llvm_cov_bin
        .exists()
        .then_some(())
        .ok_or_else(|| anyhow!("{:?} does not exist", llvm_cov_bin))?;

    let profdata_runner = ProfdataRunner::new(llvm_profdata_bin, verbose_mode);

    let profraws = glob_profraws(&cmd.test_output_dir)?;
    if profraws.is_empty() {
        return Ok(());
    }

    let symbol_index_path = match cmd.symbol_index_json {
        Some(p) => p.to_string_lossy().to_string(),
        None => global_symbol_index_path()?,
    };
    let symbol_index = SymbolIndex::load_aggregate(&symbol_index_path)?;
    let bin_files = profdata_runner.find_binaries(&symbol_index, &profraws)?;

    // TODO(https://fxbug.dev/42182448): find a better place to put merged.profdata.
    let merged_profile = cmd.test_output_dir.join("merged.profdata");
    profdata_runner
        .merge_profraws(&profraws, &merged_profile, &bin_files)
        .context("failed to merge profiles")?;

    // Flatten the binaries for llvm-cov and ensure they are unique.
    let mut unique_binaries = HashSet::new();
    for bins in &bin_files {
        for bin in bins {
            unique_binaries.insert(bin.clone());
        }
    }
    let unique_binaries_vec: Vec<PathBuf> = unique_binaries.into_iter().collect();

    let extra_args = to_extra_export_args(
        &cmd.path_remappings,
        cmd.compilation_dir.as_ref(),
        cmd.include_filename_regex.as_deref(),
        cmd.ignore_filename_regex.as_deref(),
    );

    let params = ExportParams {
        llvm_cov_bin,
        merged_profile,
        bin_files_args: to_llvm_cov_args(&unique_binaries_vec),
        src_files: cmd.src_files,
        extra_args,
    };

    match (cmd.export_html, cmd.export_lcov, cmd.export_json) {
        (None, None, None) => {
            show_coverage(&params, verbose_mode).context("failed to show coverage")?
        }
        (html, lcov, json) => {
            if let Some(ref html_export_dir) = html {
                export_html(&params, html_export_dir, verbose_mode).context(format!(
                    "failed to export HTML coverage report to {:?}",
                    html_export_dir
                ))?
            }

            if let Some(ref lcov_export_path) = lcov {
                export_lcov(&params, lcov_export_path, verbose_mode)
                    .context(format!("failed to export lcov to {:?}", lcov_export_path))?
            }

            if let Some(ref json_export_path) = json {
                export_json(&params, json_export_path, verbose_mode)
                    .context(format!("failed to export json to {:?}", json_export_path))?
            }
        }
    }

    Ok(())
}

struct ProfdataRunner {
    llvm_profdata_bin: PathBuf,
    verbose: VerboseMode,
    #[cfg(test)]
    mock_binary_ids: Mutex<HashMap<PathBuf, Vec<String>>>,
}

impl ProfdataRunner {
    fn new(llvm_profdata_bin: PathBuf, verbose: VerboseMode) -> Self {
        #[cfg(test)]
        {
            Self { llvm_profdata_bin, verbose, mock_binary_ids: Mutex::new(HashMap::new()) }
        }
        #[cfg(not(test))]
        {
            Self { llvm_profdata_bin, verbose }
        }
    }

    #[cfg(test)]
    fn add_mock_binary_ids(&self, binary_ids: HashMap<PathBuf, Vec<String>>) {
        self.mock_binary_ids.lock().unwrap().extend(binary_ids);
    }

    /// Merges input `profraws` using llvm-profdata and writes output to `output_path`.
    fn merge_profraws(
        &self,
        profraws: &[PathBuf],
        output_path: &Path,
        bin_files: &[Vec<PathBuf>],
    ) -> Result<()> {
        let temp_dir = tempdir().context("failed to create temp directory for merging")?;

        let individual_profdatas: Vec<PathBuf> = profraws
            .par_iter()
            .zip(bin_files.par_iter())
            .enumerate()
            .filter_map(|(i, (profraw, bins))| {
                let temp_profdata = temp_dir.path().join(format!("{}.profdata", i));
                let mut cmd = Command::new(&self.llvm_profdata_bin);
                cmd.args(["merge", "--sparse", "--output"]).arg(&temp_profdata);
                for bin in bins {
                    cmd.arg("--binary-file").arg(bin);
                }
                cmd.arg(profraw);

                match run_with_verbose(&mut cmd, self.verbose) {
                    Ok(merge_cmd) if merge_cmd.status.success() => Some(temp_profdata),
                    _ => {
                        if self.verbose == VerboseMode::Verbose {
                            eprintln!(
                                "Warning: failed to merge {:?} with binaries {:?}",
                                profraw, bins
                            );
                        }
                        None
                    }
                }
            })
            .collect();

        if individual_profdatas.is_empty() {
            return Err(anyhow!("no profiles could be merged"));
        }

        let mut final_cmd = Command::new(&self.llvm_profdata_bin);
        final_cmd
            .args(["merge", "--sparse", "--output"])
            .arg(output_path)
            .args(individual_profdatas);

        let final_merge_cmd = run_with_verbose(&mut final_cmd, self.verbose)
            .context("failed to perform final profile merge")?;

        match final_merge_cmd.status.code() {
            Some(0) => Ok(()),
            Some(code) => Err(anyhow!(
                "failed to merge raw profiles: status code {}, stderr: {}",
                code,
                String::from_utf8_lossy(&final_merge_cmd.stderr)
            )),
            None => Err(anyhow!("profile merging terminated by signal unexpectedly")),
        }
    }

    /// Calls `llvm-profdata show --binary-ids` to fetch binary IDs from input raw profile.
    fn show_binary_ids(&self, profraw: &Path) -> Result<Vec<String>> {
        #[cfg(test)]
        {
            let binary_ids_map = self.mock_binary_ids.lock().unwrap();
            return binary_ids_map
                .get(profraw)
                .cloned()
                .ok_or_else(|| anyhow!("no mock binary IDs available for profile {:?}", profraw));
        }
        #[cfg(not(test))]
        {
            let cmd = run_with_verbose(
                Command::new(&self.llvm_profdata_bin).args(["show", "--binary-ids"]).arg(profraw),
                self.verbose,
            )
            .context(format!("failed to show binary IDs from {:?}", profraw))?;
            let stdout = String::from_utf8_lossy(&cmd.stdout);
            let tokens: Vec<&str> = stdout.split(BINARY_ID_LINE).collect();
            match tokens[..] {
                [_, binary_ids_str] => Ok(binary_ids_str
                    .lines()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()),
                _ => Err(anyhow!("unexpected llvm-profdata show output: {}", stdout)),
            }
        }
    }

    /// Find binary files from .build-id directories to pass. These are needed for `llvm-cov show`.
    fn find_binaries(
        &self,
        symbol_index: &SymbolIndex,
        profraws: &[PathBuf],
    ) -> Result<Vec<Vec<PathBuf>>> {
        profraws
            .par_iter()
            .map(|profraw| {
                let binary_ids = self.show_binary_ids(profraw)?;
                binary_ids
                    .iter()
                    .map(|binary_id| {
                        find_debug_file(symbol_index, binary_id).context(anyhow!(
                            "failed to find binary file for ID {} in {:?}",
                            binary_id,
                            profraw,
                        ))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect()
    }
}

/// Finds debug file in local .build-id directories from symbol index.
//
// TODO(https://fxbug.dev/42051063): replace this with llvm-debuginfod-find when it's available.
fn find_debug_file(symbol_index: &SymbolIndex, binary_id: &str) -> Result<PathBuf> {
    if binary_id.len() > 2 {
        // For simplicity always return the first match. Note this is not always safe.
        symbol_index
            .build_id_dirs
            .iter()
            .find_map(|dir| {
                let p = PathBuf::from(&dir.path)
                    .join(&binary_id[..2])
                    .join(format!("{}.debug", &binary_id[2..]));
                p.exists().then_some(p)
            })
            .ok_or_else(|| anyhow!("no matching debug files found for binary ID {}", binary_id))
    } else {
        Err(anyhow!("binary ID must have more than 2 characters, got '{}'", binary_id))
    }
}

fn to_llvm_cov_args(bin_files: &[PathBuf]) -> Vec<&str> {
    bin_files.iter().fold(Vec::new(), |mut acc, val| {
        if acc.len() > 0 {
            acc.push("-object");
        }
        acc.push(val.to_str().expect("failed to convert path to string"));
        acc
    })
}

fn to_extra_export_args<'a>(
    path_remappings: &'a [String],
    compilation_dir: Option<&'a PathBuf>,
    include_filename_regex: Option<&'a str>,
    ignore_filename_regex: Option<&'a str>,
) -> Vec<&'a str> {
    match path_remappings {
        &[] => Vec::new(),
        _ => ["-path-equivalence"]
            .into_iter()
            .chain(path_remappings.iter().map(|s| s.as_str()))
            .collect(),
    }
    .into_iter()
    .chain(match compilation_dir {
        Some(dir) => vec!["-compilation-dir", dir.to_str().unwrap()],
        None => Vec::new(),
    })
    .chain(match include_filename_regex {
        Some(include_regex) => vec!["--include-filename-regex", include_regex],
        None => Vec::new(),
    })
    .chain(match ignore_filename_regex {
        Some(ignore_regex) => vec!["--ignore-filename-regex", ignore_regex],
        None => Vec::new(),
    })
    .collect()
}

/// Calls `llvm-cov show` to display coverage from `merged_profile` for `bin_files`.
/// `src_files` can be used to filter coverage for selected source files.
fn show_coverage(params: &ExportParams<'_>, verbose_mode: VerboseMode) -> Result<()> {
    let show_cmd = run_with_verbose(
        Command::new(&params.llvm_cov_bin)
            .args(["show", "-instr-profile"])
            .arg(&params.merged_profile)
            .args(&params.extra_args)
            .args(&params.bin_files_args)
            .args(&params.src_files)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
        verbose_mode,
    )
    .context("failed to show coverage")?;
    match show_cmd.status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(anyhow!(
            "failed to show coverage: status code {}, stderr: {}",
            code,
            String::from_utf8_lossy(&show_cmd.stderr)
        )),
        None => Err(anyhow!("coverage display terminated by signal unexpectedly")),
    }
}

/// Calls `llvm-cov show -format html` to write HTML pages for collected test coverage.
fn export_html(
    params: &ExportParams<'_>,
    html_export_path: &Path,
    verbose_mode: VerboseMode,
) -> Result<()> {
    let show_cmd = run_with_verbose(
        Command::new(&params.llvm_cov_bin)
            .args(["show", "-format", "html", "-output-dir"])
            .arg(html_export_path)
            .arg("-instr-profile")
            .arg(&params.merged_profile)
            .args(&params.extra_args)
            .args(&params.bin_files_args)
            .args(&params.src_files)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit()),
        verbose_mode,
    )
    .context("failed to export HTML coverage")?;
    match show_cmd.status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(anyhow!(
            "failed to show coverage: status code {}, stderr: {}",
            code,
            String::from_utf8_lossy(&show_cmd.stderr)
        )),
        None => Err(anyhow!("coverage HTML export terminated by signal unexpectedly")),
    }
}

/// Calls `llvm-cov export -format lcov` to write a LCOV file for collected test coverage.
fn export_lcov(
    params: &ExportParams<'_>,
    lcov_export_path: &Path,
    verbose_mode: VerboseMode,
) -> Result<()> {
    let output_lcov = File::create(lcov_export_path)?;
    let show_cmd = run_with_verbose(
        Command::new(&params.llvm_cov_bin)
            .args(["export", "-format", "lcov", "-skip-expansions", "-skip-functions"])
            .arg("-instr-profile")
            .arg(&params.merged_profile)
            .args(&params.extra_args)
            .args(&params.bin_files_args)
            .args(&params.src_files)
            .stdout(output_lcov)
            .stderr(Stdio::inherit()),
        verbose_mode,
    )?;

    match show_cmd.status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(anyhow!(
            "failed to show coverage: status code {}, stderr: {}",
            code,
            String::from_utf8_lossy(&show_cmd.stderr)
        )),
        None => Err(anyhow!("LCOV export terminated by signal unexpectedly")),
    }
}

/// Calls `llvm-cov export -format text` to write a JSON file for collected test coverage.
fn export_json(
    params: &ExportParams<'_>,
    json_export_path: &Path,
    verbose_mode: VerboseMode,
) -> Result<()> {
    let output_json = File::create(json_export_path)?;
    let show_cmd = run_with_verbose(
        Command::new(&params.llvm_cov_bin)
            .args(["export", "-format", "text"])
            .arg("-instr-profile")
            .arg(&params.merged_profile)
            .args(&params.extra_args)
            .args(&params.bin_files_args)
            .args(&params.src_files)
            .stdout(output_json)
            .stderr(Stdio::inherit()),
        verbose_mode,
    )?;

    match show_cmd.status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(anyhow!(
            "failed to show coverage: status code {}, stderr: {}",
            code,
            String::from_utf8_lossy(&show_cmd.stderr)
        )),
        None => Err(anyhow!("JSON export terminated by signal unexpectedly")),
    }
}

/// Finds all raw coverage profiles in `test_out_dir`.
fn glob_profraws(test_out_dir: &Path) -> Result<Vec<PathBuf>> {
    let pattern = test_out_dir.join("**").join("*.profraw");
    Ok(glob(pattern.to_str().unwrap())?.filter_map(Result::ok).collect::<Vec<PathBuf>>())
}

/// Run a command, respecting the --verbose setting to output command line and outputs if set.
fn run_with_verbose(cmd: &mut Command, verbose_mode: VerboseMode) -> Result<std::process::Output> {
    if verbose_mode == VerboseMode::Verbose {
        println!("Command: {:?}", cmd);
    }
    let cmd = cmd.output().context("failed to run command")?;
    if verbose_mode == VerboseMode::Verbose {
        println!("Command stdout:\n{}", String::from_utf8_lossy(&cmd.stdout));
        println!("Command stderr:\n{}", String::from_utf8_lossy(&cmd.stderr));
    }
    Ok(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use std::fs::{Permissions, create_dir, set_permissions};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use symbol_index::BuildIdDir;
    use tempfile::TempDir;

    #[test]
    fn test_glob_profraws() {
        let test_dir = TempDir::new().unwrap();
        create_dir(test_dir.path().join("nested")).unwrap();

        File::create(test_dir.path().join("foo.profraw")).unwrap();
        File::create(test_dir.path().join("nested").join("bar.profraw")).unwrap();
        File::create(test_dir.path().join("foo.not_profraw")).unwrap();
        File::create(test_dir.path().join("nested").join("baz.not_profraw")).unwrap();

        assert_eq!(
            glob_profraws(&test_dir.path().to_path_buf()).unwrap(),
            vec![
                PathBuf::from(test_dir.path().join("foo.profraw")),
                PathBuf::from(test_dir.path().join("nested").join("bar.profraw")),
            ],
        );
    }

    #[fuchsia::test]
    async fn test_coverage() {
        let _env = ffx_config::test_init().unwrap();

        let test_dir = TempDir::new().unwrap();
        let test_dir_path = test_dir.path().to_path_buf();
        let test_bin_dir = test_dir_path.join("bin");
        create_dir(&test_bin_dir).unwrap();

        // Create an empty symbol index for testing.
        let test_symbol_index_json = test_dir.path().join("symbol_index.json");
        File::create(&test_symbol_index_json).unwrap().write_all(b"{}").unwrap();

        // Missing both llvm-profdata and llvm-cov.
        assert!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: None,
                export_json: None,
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await
            .is_err()
        );

        // Create empty test binaries for the coverage function to call.
        File::create(test_bin_dir.join("llvm-profdata")).unwrap().write_all(b"#!/bin/sh").unwrap();
        set_permissions(test_bin_dir.join("llvm-profdata"), Permissions::from_mode(0o770)).unwrap();

        // Still missing llvm-cov.
        assert!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: None,
                export_json: None,
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await
            .is_err()
        );

        File::create(test_bin_dir.join("llvm-cov")).unwrap().write_all(b"#!/bin/sh").unwrap();
        set_permissions(test_bin_dir.join("llvm-cov"), Permissions::from_mode(0o770)).unwrap();

        // Print coverage to stdout.
        assert_matches!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: None,
                export_json: None,
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await,
            Ok(())
        );

        // Export HTML.
        assert_matches!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: Some(PathBuf::from(&test_dir_path)),
                export_lcov: None,
                export_json: None,
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await,
            Ok(())
        );

        // Export LCOV.
        assert_matches!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: Some(PathBuf::from(&test_dir_path).join("test.lcov")),
                export_json: None,
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await,
            Ok(())
        );

        // Export JSON.
        assert_matches!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: None,
                export_json: Some(PathBuf::from(&test_dir_path).join("test.json")),
                path_remappings: Vec::new(),
                compilation_dir: None,
                include_filename_regex: None,
                ignore_filename_regex: None,
                src_files: Vec::new(),
                verbose: false,
            })
            .await,
            Ok(())
        );

        // Export with non-empty path_remappings and compilation_dir.
        assert_matches!(
            coverage(CoverageCommand {
                test_output_dir: PathBuf::from(&test_dir_path),
                clang_dir: PathBuf::from(&test_dir_path),
                symbol_index_json: Some(PathBuf::from(&test_symbol_index_json)),
                export_html: None,
                export_lcov: None,
                export_json: None,
                path_remappings: vec![
                    "from_path,to_path".to_string(),
                    "from_path2,to_path2".to_string()
                ],
                compilation_dir: Some(PathBuf::from("path/to/comp/dir")),
                include_filename_regex: Some("include_pat".to_string()),
                ignore_filename_regex: Some("ignore_pat".to_string()),
                src_files: Vec::new(),
                verbose: false,
            })
            .await,
            Ok(())
        );
    }

    #[test]
    fn test_find_binaries_single_match() {
        let test_dir = TempDir::new().unwrap();
        create_dir(test_dir.path().join("fo")).unwrap();
        let debug_file = test_dir.path().join("fo").join("obar.debug");
        File::create(&debug_file).unwrap();

        let profraw = PathBuf::from("test.profraw");
        let profdata_cmd = ProfdataRunner::new(PathBuf::new(), VerboseMode::NotVerbose);
        let mut mocks = HashMap::new();
        mocks.insert(profraw.clone(), vec!["foobar".to_string()]);
        profdata_cmd.add_mock_binary_ids(mocks);

        assert_eq!(
            profdata_cmd
                .find_binaries(
                    &SymbolIndex {
                        build_id_dirs: vec![BuildIdDir {
                            path: test_dir.path().to_str().unwrap().to_string(),
                            build_dir: None,
                        }],
                        includes: Vec::new(),
                        ids_txts: Vec::new(),
                        gcs_flat: Vec::new(),
                        debuginfod: Vec::new(),
                    },
                    &[profraw],
                )
                .unwrap(),
            vec![vec![debug_file]],
        )
    }

    #[test]
    fn test_find_binaries_multiple_matches() {
        let test_dir1 = TempDir::new().unwrap();
        create_dir(test_dir1.path().join("fo")).unwrap();
        let debug_file1 = test_dir1.path().join("fo").join("obar.debug");
        File::create(&debug_file1).unwrap();

        let test_dir2 = TempDir::new().unwrap();
        create_dir(test_dir2.path().join("ba")).unwrap();
        let debug_file2 = test_dir2.path().join("ba").join("rbaz.debug");
        File::create(&debug_file2).unwrap();

        let profraw1 = PathBuf::from("test1.profraw");
        let profraw2 = PathBuf::from("test2.profraw");

        let profdata_cmd = ProfdataRunner::new(PathBuf::new(), VerboseMode::NotVerbose);
        let mut mocks = HashMap::new();
        mocks.insert(profraw1.clone(), vec!["foobar".to_string()]);
        mocks.insert(profraw2.clone(), vec!["barbaz".to_string()]);
        profdata_cmd.add_mock_binary_ids(mocks);

        let results = profdata_cmd
            .find_binaries(
                &SymbolIndex {
                    build_id_dirs: vec![
                        BuildIdDir {
                            path: test_dir1.path().to_str().unwrap().to_string(),
                            build_dir: None,
                        },
                        BuildIdDir {
                            path: test_dir2.path().to_str().unwrap().to_string(),
                            build_dir: None,
                        },
                    ],
                    includes: Vec::new(),
                    ids_txts: Vec::new(),
                    gcs_flat: Vec::new(),
                    debuginfod: Vec::new(),
                },
                &[profraw1, profraw2],
            )
            .unwrap();

        // Use Set for comparison since par_iter order is non-deterministic
        let results_flat: HashSet<_> = results.into_iter().flatten().collect();
        let expected_flat: HashSet<_> = vec![debug_file1, debug_file2].into_iter().collect();
        assert_eq!(results_flat, expected_flat);
    }

    #[test]
    fn test_find_binaries_no_matches() {
        let test_dir = TempDir::new().unwrap();
        let profraw = PathBuf::from("test.profraw");
        let profdata_cmd = ProfdataRunner::new(PathBuf::new(), VerboseMode::NotVerbose);
        let mut mocks = HashMap::new();
        mocks.insert(profraw.clone(), vec!["foobar".to_string()]);
        profdata_cmd.add_mock_binary_ids(mocks);

        assert!(
            profdata_cmd
                .find_binaries(
                    &SymbolIndex {
                        build_id_dirs: vec![BuildIdDir {
                            path: test_dir.path().to_str().unwrap().to_string(),
                            build_dir: None,
                        }],
                        includes: Vec::new(),
                        ids_txts: Vec::new(),
                        gcs_flat: Vec::new(),
                        debuginfod: Vec::new(),
                    },
                    &[profraw],
                )
                .is_err()
        )
    }

    #[test]
    fn test_find_binaries_show_id_err() {
        let profdata_cmd = ProfdataRunner::new(PathBuf::new(), VerboseMode::NotVerbose);
        // no mock IDs, so it will throw an error

        assert!(
            profdata_cmd
                .find_binaries(
                    &SymbolIndex {
                        build_id_dirs: Vec::new(),
                        includes: Vec::new(),
                        ids_txts: Vec::new(),
                        gcs_flat: Vec::new(),
                        debuginfod: Vec::new(),
                    },
                    &[PathBuf::new()], // profraws, actual values don't matter
                )
                .is_err()
        )
    }

    #[test]
    fn test_find_binaries_id_too_short() {
        let profraw = PathBuf::from("test.profraw");
        let profdata_cmd = ProfdataRunner::new(PathBuf::new(), VerboseMode::NotVerbose);
        let mut mocks = HashMap::new();
        mocks.insert(profraw.clone(), vec!["a".to_string()]);
        profdata_cmd.add_mock_binary_ids(mocks);

        assert!(
            profdata_cmd
                .find_binaries(
                    &SymbolIndex {
                        build_id_dirs: Vec::new(),
                        includes: Vec::new(),
                        ids_txts: Vec::new(),
                        gcs_flat: Vec::new(),
                        debuginfod: Vec::new(),
                    },
                    &[profraw],
                )
                .is_err()
        )
    }

    #[test]
    fn test_to_extra_export_args() {
        assert_eq!(to_extra_export_args(&[], None, None, None), Vec::<&str>::new());
        assert_eq!(
            to_extra_export_args(
                &["from,to".to_string(), "path1,path2".to_string()],
                None,
                None,
                None
            ),
            vec!["-path-equivalence", "from,to", "path1,path2"]
        );
        assert_eq!(
            to_extra_export_args(&[], Some(&PathBuf::from("path/to/comp/dir")), None, None),
            vec!["-compilation-dir", "path/to/comp/dir"]
        );
        assert_eq!(
            to_extra_export_args(
                &["p1,p2".to_string()],
                Some(&PathBuf::from("comp_dir")),
                Some("include_pattern"),
                Some("ignore_pattern"),
            ),
            vec![
                "-path-equivalence",
                "p1,p2",
                "-compilation-dir",
                "comp_dir",
                "--include-filename-regex",
                "include_pattern",
                "--ignore-filename-regex",
                "ignore_pattern",
            ]
        );
    }

    #[fuchsia::test]
    async fn test_export_json() {
        let test_dir = TempDir::new().unwrap();
        let test_dir_path = test_dir.path().to_path_buf();
        let test_bin_dir = test_dir_path.join("bin");
        create_dir(&test_bin_dir).unwrap();

        let llvm_cov_bin = test_bin_dir.join("llvm-cov");
        File::create(&llvm_cov_bin)
            .unwrap()
            .write_all(
                b"#!/bin/sh\n\
                for arg in \"$@\"; do\n\
                  if [ \"$arg\" = \"text\" ]; then\n\
                    echo '{\"data\":[{\"files\":[{\"filename\":\"a.cc\",\"segments\":[],\"branches\":[],\"summary\":{\"lines\":{\"count\":1,\"covered\":1,\"percent\":100.0}}}],\"totals\":{\"lines\":{\"count\":1,\"covered\":1,\"percent\":100.0}}}],\"type\":\"llvm.coverage.json.export\",\"version\":\"2.0.1\"}'\n\
                    exit 0\n\
                  fi\n\
                done",
            )
            .unwrap();
        set_permissions(&llvm_cov_bin, Permissions::from_mode(0o770)).unwrap();

        let json_export_path = test_dir_path.join("test.json");
        let params = ExportParams {
            llvm_cov_bin,
            merged_profile: PathBuf::from("merged.profdata"),
            bin_files_args: vec![],
            src_files: vec![],
            extra_args: vec![],
        };

        export_json(&params, &json_export_path, VerboseMode::NotVerbose).unwrap();

        let json_content = std::fs::read_to_string(json_export_path).unwrap();
        assert_eq!(
            json_content,
            "{\"data\":[{\"files\":[{\"filename\":\"a.cc\",\"segments\":[],\"branches\":[],\"summary\":{\"lines\":{\"count\":1,\"covered\":1,\"percent\":100.0}}}],\"totals\":{\"lines\":{\"count\":1,\"covered\":1,\"percent\":100.0}}}],\"type\":\"llvm.coverage.json.export\",\"version\":\"2.0.1\"}\n"
        );
    }
}
