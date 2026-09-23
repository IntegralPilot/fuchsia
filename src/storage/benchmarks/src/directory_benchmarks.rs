// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Benchmark, CacheClearableFilesystem, Filesystem, OperationDuration, OperationTimer};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::ops::Range;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// Describes the structure of a directory tree to be benchmarked.
#[derive(Copy, Clone)]
pub struct DirectoryTreeStructure {
    /// The number of files that will be created within each directory.
    pub files_per_directory: u64,

    /// The number of subdirectories that will be created within each directory. No subdirectories
    /// are created once |max_depth| is reached.
    pub directories_per_directory: u64,

    /// Specifies how many levels of directories to create.
    pub max_depth: u32,
}

impl DirectoryTreeStructure {
    /// Returns the maximum size of queue required to do a breadth first traversal of a directory
    /// tree with this structure.
    fn max_pending(&self) -> u64 {
        self.directories_per_directory.pow(self.max_depth)
    }

    /// Creates a directory tree as described by this object starting from |root|.
    fn create_directory_tree(&self, root: PathBuf) {
        struct DirInfo {
            path: PathBuf,
            depth: u32,
        }
        let mut pending = VecDeque::new();
        pending.push_back(DirInfo { path: root, depth: 0 });
        while let Some(dir) = pending.pop_front() {
            for i in 0..self.files_per_directory {
                let path = dir.path.join(file_name(i));
                std::fs::File::create(path).unwrap();
            }
            if self.max_depth == dir.depth {
                continue;
            }
            for i in 0..self.directories_per_directory {
                let path = dir.path.join(dir_name(i));
                std::fs::create_dir(&path).unwrap();
                pending.push_back(DirInfo { path, depth: dir.depth + 1 });
            }
        }
    }

    /// Generates a list of paths that would be created by `create_directory_tree`.
    fn enumerate_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        self.enumerate_paths_impl(Path::new(""), &mut paths);
        paths
    }

    fn enumerate_paths_impl(&self, base: &Path, paths: &mut Vec<PathBuf>) {
        for i in 0..self.files_per_directory {
            paths.push(base.join(file_name(i)));
        }
        if self.max_depth == 0 {
            return;
        }
        for i in 0..self.directories_per_directory {
            let path = base.join(dir_name(i));
            paths.push(path.clone());
            Self { max_depth: self.max_depth - 1, ..*self }.enumerate_paths_impl(&path, paths);
        }
    }
}

/// A benchmark that measures how long it takes to walk a directory that should not already be
/// cached in memory.
#[derive(Clone)]
pub struct WalkDirectoryTreeCold {
    dts: DirectoryTreeStructure,
    iterations: u64,
}

impl WalkDirectoryTreeCold {
    pub fn new(dts: DirectoryTreeStructure, iterations: u64) -> Self {
        Self { dts, iterations }
    }
}

#[async_trait]
impl<T: CacheClearableFilesystem> Benchmark<T> for WalkDirectoryTreeCold {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!(
            "benchmark",
            "WalkDirectoryTreeCold",
            "files_per_directory" => self.dts.files_per_directory,
            "directories_per_directory" => self.dts.directories_per_directory,
            "max_depth" => self.dts.max_depth
        );
        let root = fs.benchmark_dir().to_path_buf();

        self.dts.create_directory_tree(root.clone());
        let max_pending = self.dts.max_pending() as usize;
        let mut durations = Vec::new();
        for i in 0..self.iterations {
            fs.clear_cache().await;
            storage_trace::duration!("benchmark", "WalkDirectoryTree", "iteration" => i);
            let timer = OperationTimer::start();
            walk_directory_tree(root.clone(), max_pending);
            durations.push(timer.stop());
        }
        durations
    }

    fn name(&self) -> String {
        format!(
            "WalkDirectoryTreeCold/Files{}/Dirs{}/Depth{}",
            self.dts.files_per_directory, self.dts.directories_per_directory, self.dts.max_depth
        )
    }
}

/// A benchmark that measures how long it takes to walk a directory that was recently accessed and
/// may be cached in memory.
#[derive(Clone)]
pub struct WalkDirectoryTreeWarm {
    dts: DirectoryTreeStructure,
    iterations: u64,
}

impl WalkDirectoryTreeWarm {
    pub fn new(dts: DirectoryTreeStructure, iterations: u64) -> Self {
        Self { dts, iterations }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for WalkDirectoryTreeWarm {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!(
            "benchmark",
            "WalkDirectoryTreeWarm",
            "files_per_directory" => self.dts.files_per_directory,
            "directories_per_directory" => self.dts.directories_per_directory,
            "max_depth" => self.dts.max_depth
        );
        let root = fs.benchmark_dir().to_path_buf();

        self.dts.create_directory_tree(root.clone());
        let max_pending = self.dts.max_pending() as usize;
        let mut durations = Vec::new();
        for i in 0..self.iterations {
            storage_trace::duration!("benchmark", "WalkDirectoryTree", "iteration" => i);
            let timer = OperationTimer::start();
            walk_directory_tree(root.clone(), max_pending);
            durations.push(timer.stop());
        }
        durations
    }

    fn name(&self) -> String {
        format!(
            "WalkDirectoryTreeWarm/Files{}/Dirs{}/Depth{}",
            self.dts.files_per_directory, self.dts.directories_per_directory, self.dts.max_depth
        )
    }
}

/// Performs a breadth first traversal of the directory tree starting at |root|. As an optimization,
/// |max_pending| can be supplied to pre-allocate the queue needed for the breadth first traversal.
fn walk_directory_tree(root: PathBuf, max_pending: usize) {
    let mut pending = VecDeque::new();
    pending.reserve(max_pending);
    pending.push_back(root);
    while let Some(dir) = pending.pop_front() {
        storage_trace::duration!("benchmark", "read_dir");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push_back(entry.path());
            }
        }
    }
}

/// A benchmark that measures how long it takes to call stat on a path to a file. A distinct file is
/// used for each iteration.
#[derive(Clone)]
pub struct StatPath {
    file_count: u64,
}

impl StatPath {
    pub fn new() -> Self {
        Self { file_count: 100 }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for StatPath {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!("benchmark", "StatPath");

        let root = fs.benchmark_dir().to_path_buf();
        for i in 0..self.file_count {
            std::fs::File::create(root.join(file_name(i))).unwrap();
        }

        let root_fd =
            open_path(&path_buf_to_c_string(root), libc::O_DIRECTORY | libc::O_RDONLY).unwrap();

        let mut durations = Vec::with_capacity(self.file_count as usize);
        for i in 0..self.file_count {
            let path = path_buf_to_c_string(file_name(i));
            storage_trace::duration!("benchmark", "stat", "file" => i);
            let timer = OperationTimer::start();
            stat_path_at(&root_fd, &path).unwrap();
            durations.push(timer.stop());
        }
        durations
    }

    fn name(&self) -> String {
        "StatPath".to_string()
    }
}

/// A benchmark that measures how long it takes to open a file. A distinct file is used for each
/// iteration.
#[derive(Clone)]
pub struct OpenFile {
    file_count: u64,
}

impl OpenFile {
    pub fn new() -> Self {
        Self { file_count: 100 }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for OpenFile {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!("benchmark", "OpenFile");

        let root = fs.benchmark_dir().to_path_buf();
        for i in 0..self.file_count {
            std::fs::File::create(root.join(file_name(i))).unwrap();
        }

        let root_fd =
            open_path(&path_buf_to_c_string(root), libc::O_DIRECTORY | libc::O_RDONLY).unwrap();

        let mut durations = Vec::with_capacity(self.file_count as usize);
        for i in 0..self.file_count {
            let path = path_buf_to_c_string(file_name(i));
            // Pull the file outside of the trace so it doesn't capture the close call.
            let _file = {
                storage_trace::duration!("benchmark", "open", "file" => i);
                let timer = OperationTimer::start();
                let file = open_path_at(&root_fd, &path, libc::O_RDWR);
                durations.push(timer.stop());
                file
            };
        }
        durations
    }

    fn name(&self) -> String {
        "OpenFile".to_string()
    }
}

/// A benchmark that measures how long it takes to create a file.
#[derive(Clone)]
pub struct CreateFile {
    file_count: u64,
}

impl CreateFile {
    pub fn new() -> Self {
        Self { file_count: 100 }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for CreateFile {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!("benchmark", "CreateFile");

        let root = fs.benchmark_dir().to_path_buf();
        let root_fd =
            open_path(&path_buf_to_c_string(root), libc::O_DIRECTORY | libc::O_RDONLY).unwrap();

        let mut durations = Vec::with_capacity(self.file_count as usize);
        for i in 0..self.file_count {
            let path = path_buf_to_c_string(file_name(i));
            // Pull the file outside of the trace so it doesn't capture the close call.
            let _file = {
                storage_trace::duration!("benchmark", "create", "file" => i);
                let timer = OperationTimer::start();
                let file = open_path_at(&root_fd, &path, libc::O_CREAT | libc::O_RDWR);
                durations.push(timer.stop());
                file
            };
        }
        durations
    }

    fn name(&self) -> String {
        "CreateFile".to_string()
    }
}

/// A benchmark that measures how long it takes to unlink a file.
#[derive(Clone)]
pub struct UnlinkFile {
    file_count: u64,
}

impl UnlinkFile {
    pub fn new() -> Self {
        Self { file_count: 100 }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for UnlinkFile {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!("benchmark", "UnlinkFile");

        let root = fs.benchmark_dir().to_path_buf();
        let root_fd =
            open_path(&path_buf_to_c_string(root), libc::O_DIRECTORY | libc::O_RDONLY).unwrap();

        for i in 0..self.file_count {
            let path = path_buf_to_c_string(file_name(i));
            let _file = open_path_at(&root_fd, &path, libc::O_CREAT | libc::O_RDWR).unwrap();
        }

        let mut durations = Vec::with_capacity(self.file_count as usize);
        for i in 0..self.file_count {
            let path = path_buf_to_c_string(file_name(i));
            storage_trace::duration!("benchmark", "unlink", "file" => i);
            let timer = OperationTimer::start();
            unlink_path_at(&root_fd, &path).unwrap();
            durations.push(timer.stop());
        }
        durations
    }

    fn name(&self) -> String {
        "UnlinkFile".to_string()
    }
}

/// A benchmark that measures how long it takes to open a file from a path that contains multiple
/// directories. A distinct path and file is used for each iteration.
#[derive(Clone)]
pub struct OpenDeeplyNestedFile {
    file_count: u64,
    depth: u64,
}

impl OpenDeeplyNestedFile {
    pub fn new() -> Self {
        Self { file_count: 100, depth: 5 }
    }

    fn dir_path(&self, n: u64) -> PathBuf {
        let mut path = PathBuf::new();
        for i in 0..self.depth {
            path = path.join(format!("dir-{:03}-{:03}", n, i));
        }
        path
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for OpenDeeplyNestedFile {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        storage_trace::duration!("benchmark", "OpenDeeplyNestedFile");

        let root = fs.benchmark_dir().to_path_buf();
        for i in 0..self.file_count {
            let dir_path = root.join(self.dir_path(i));
            std::fs::create_dir_all(&dir_path).unwrap();
            std::fs::File::create(dir_path.join(file_name(i))).unwrap();
        }

        let root_fd =
            open_path(&path_buf_to_c_string(root), libc::O_DIRECTORY | libc::O_RDONLY).unwrap();

        let mut durations = Vec::with_capacity(self.file_count as usize);
        for i in 0..self.file_count {
            let path = path_buf_to_c_string(self.dir_path(i).join(file_name(i)));
            // Pull the file outside of the trace so it doesn't capture the close call.
            let _file = {
                storage_trace::duration!("benchmark", "open", "file" => i);
                let timer = OperationTimer::start();
                let file = open_path_at(&root_fd, &path, libc::O_RDWR);
                durations.push(timer.stop());
                file
            };
        }
        durations
    }

    fn name(&self) -> String {
        "OpenDeeplyNestedFile".to_string()
    }
}

/// A benchmark that mimics the filesystem usage pattern of `git status`.
#[derive(Clone)]
pub struct GitStatus {
    dts: DirectoryTreeStructure,
    iterations: u64,

    /// The number of threads to use when stat'ing all of the files.
    stat_threads: usize,
}

impl GitStatus {
    pub fn new() -> Self {
        // This will create 254 directories and 1020 files. A depth of 13 would create 16382
        // directories and 65532 files which is close to the size of fuchsia.git.
        let dts = DirectoryTreeStructure {
            files_per_directory: 4,
            directories_per_directory: 2,
            max_depth: 7,
        };

        // Git uses many threads when stat'ing large repositories but using multiple threads in the
        // benchmarks significantly increases the performance variation between runs.
        Self { dts, iterations: 5, stat_threads: 1 }
    }

    /// Stat all of the paths in `paths`. This mimics git checking to see if any of the files in its
    /// index have been modified.
    fn stat_paths(&self, root: &OpenFd, paths: &Vec<CString>) {
        storage_trace::duration!("benchmark", "GitStatus::stat_paths");
        std::thread::scope(|scope| {
            for thread in 0..self.stat_threads {
                scope.spawn(move || {
                    let paths = &paths;
                    for path in batch_range(paths.len(), self.stat_threads, thread) {
                        storage_trace::duration!("benchmark", "stat");
                        stat_path_at(root, &paths[path]).unwrap();
                    }
                });
            }
        });
    }

    /// Performs a recursive depth first traversal of the directory tree.
    fn walk_repo(&self, dir: &Path) {
        storage_trace::duration!("benchmark", "GitStatus::walk_repo");
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                self.walk_repo(&entry.path());
            }
        }
    }
}

#[async_trait]
impl<T: Filesystem> Benchmark<T> for GitStatus {
    async fn run(&self, fs: &mut T) -> Vec<OperationDuration> {
        let root = &fs.benchmark_dir().to_path_buf();

        self.dts.create_directory_tree(root.clone());
        let paths = self.dts.enumerate_paths().into_iter().map(path_buf_to_c_string).collect();

        let root_fd =
            open_path(&path_buf_to_c_string(root.clone()), libc::O_DIRECTORY | libc::O_RDONLY)
                .unwrap();

        let mut durations = Vec::new();
        for i in 0..self.iterations {
            storage_trace::duration!("benchmark", "GitStatus", "iteration" => i);
            let timer = OperationTimer::start();
            self.stat_paths(&root_fd, &paths);
            self.walk_repo(&root);
            durations.push(timer.stop());
        }
        durations
    }

    fn name(&self) -> String {
        "GitStatus".to_owned()
    }
}

pub fn file_name(n: u64) -> PathBuf {
    format!("file-{:03}.txt", n).into()
}

pub fn dir_name(n: u64) -> PathBuf {
    format!("dir-{:03}", n).into()
}

pub fn path_buf_to_c_string(path: PathBuf) -> CString {
    CString::new(path.into_os_string().into_vec()).unwrap()
}

pub fn stat_path_at(dir: &OpenFd, path: &CStr) -> Result<libc::stat, std::io::Error> {
    unsafe {
        let mut stat = MaybeUninit::uninit();
        // `libc::fstatat` is directly used instead of `std::fs::metadata` because
        // `std::fs::metadata` uses `statx` on Linux.
        let result =
            libc::fstatat(dir.0, path.as_ptr(), stat.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW);

        if result == 0 { Ok(stat.assume_init()) } else { Err(std::io::Error::last_os_error()) }
    }
}

/// Splits a collection of items into batches and returns the range of items that `batch_num`
/// contains.
fn batch_range(item_count: usize, batch_count: usize, batch_num: usize) -> Range<usize> {
    let items_per_batch = (item_count + (batch_count - 1)) / batch_count;
    let start = items_per_batch * batch_num;
    let end = std::cmp::min(item_count, start + items_per_batch);
    start..end
}

pub struct OpenFd(RawFd);

impl Drop for OpenFd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

pub fn open_path(path: &CStr, flags: libc::c_int) -> Result<OpenFd, std::io::Error> {
    let result = unsafe { libc::open(path.as_ptr(), flags) };
    if result >= 0 { Ok(OpenFd(result)) } else { Err(std::io::Error::last_os_error()) }
}

pub fn open_path_at(
    dir: &OpenFd,
    path: &CStr,
    flags: libc::c_int,
) -> Result<OpenFd, std::io::Error> {
    let result = unsafe { libc::openat(dir.0, path.as_ptr(), flags) };
    if result >= 0 { Ok(OpenFd(result)) } else { Err(std::io::Error::last_os_error()) }
}

pub fn unlink_path_at(dir: &OpenFd, path: &CStr) -> Result<(), std::io::Error> {
    let result = unsafe { libc::unlinkat(dir.0, path.as_ptr(), 0) };
    if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestFilesystem;

    const ITERATION_COUNT: u64 = 3;

    #[derive(PartialEq, Debug)]
    struct DirectoryTree {
        files: u64,
        directories: Vec<DirectoryTree>,
    }

    fn read_in_directory_tree(root: PathBuf) -> DirectoryTree {
        let mut tree = DirectoryTree { files: 0, directories: vec![] };
        for entry in std::fs::read_dir(root).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                tree.directories.push(read_in_directory_tree(entry.path()));
            } else {
                tree.files += 1;
            }
        }
        tree
    }

    #[fuchsia::test]
    async fn create_directory_tree_with_depth_of_zero_test() {
        let test_fs = Box::new(TestFilesystem::new());
        let dts = DirectoryTreeStructure {
            files_per_directory: 3,
            directories_per_directory: 2,
            max_depth: 0,
        };
        let root = test_fs.benchmark_dir().to_owned();
        dts.create_directory_tree(root.clone());

        let directory_tree = read_in_directory_tree(root);
        assert_eq!(directory_tree, DirectoryTree { files: 3, directories: vec![] });
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn create_directory_tree_with_depth_of_two_test() {
        let test_fs = Box::new(TestFilesystem::new());
        let dts = DirectoryTreeStructure {
            files_per_directory: 4,
            directories_per_directory: 2,
            max_depth: 2,
        };
        let root = test_fs.benchmark_dir().to_owned();
        dts.create_directory_tree(root.clone());

        let directory_tree = read_in_directory_tree(root);
        assert_eq!(
            directory_tree,
            DirectoryTree {
                files: 4,
                directories: vec![
                    DirectoryTree {
                        files: 4,
                        directories: vec![
                            DirectoryTree { files: 4, directories: vec![] },
                            DirectoryTree { files: 4, directories: vec![] }
                        ]
                    },
                    DirectoryTree {
                        files: 4,
                        directories: vec![
                            DirectoryTree { files: 4, directories: vec![] },
                            DirectoryTree { files: 4, directories: vec![] }
                        ]
                    }
                ]
            }
        );
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    fn enumerate_paths_test() {
        let dts = DirectoryTreeStructure {
            files_per_directory: 2,
            directories_per_directory: 2,
            max_depth: 0,
        };
        let paths = dts.enumerate_paths();
        assert_eq!(paths, vec![PathBuf::from("file-000.txt"), PathBuf::from("file-001.txt")],);

        let dts = DirectoryTreeStructure {
            files_per_directory: 2,
            directories_per_directory: 2,
            max_depth: 1,
        };
        let paths = dts.enumerate_paths();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("file-000.txt"),
                PathBuf::from("file-001.txt"),
                PathBuf::from("dir-000"),
                PathBuf::from("dir-000/file-000.txt"),
                PathBuf::from("dir-000/file-001.txt"),
                PathBuf::from("dir-001"),
                PathBuf::from("dir-001/file-000.txt"),
                PathBuf::from("dir-001/file-001.txt"),
            ],
        );
    }

    #[fuchsia::test]
    fn batch_range_test() {
        assert_eq!(batch_range(10, 1, 0), 0..10);

        assert_eq!(batch_range(10, 3, 0), 0..4);
        assert_eq!(batch_range(10, 3, 1), 4..8);
        assert_eq!(batch_range(10, 3, 2), 8..10);

        assert_eq!(batch_range(10, 4, 3), 9..10);
        assert_eq!(batch_range(12, 4, 3), 9..12);
    }

    #[fuchsia::test]
    async fn walk_directory_tree_cold_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let dts = DirectoryTreeStructure {
            files_per_directory: 2,
            directories_per_directory: 2,
            max_depth: 2,
        };
        let results = WalkDirectoryTreeCold::new(dts, ITERATION_COUNT).run(test_fs.as_mut()).await;

        assert_eq!(results.len(), ITERATION_COUNT as usize);
        assert_eq!(test_fs.clear_cache_count().await, ITERATION_COUNT);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn walk_directory_tree_warm_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let dts = DirectoryTreeStructure {
            files_per_directory: 2,
            directories_per_directory: 2,
            max_depth: 2,
        };
        let results = WalkDirectoryTreeWarm::new(dts, ITERATION_COUNT).run(test_fs.as_mut()).await;

        assert_eq!(results.len(), ITERATION_COUNT as usize);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn stat_path_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let benchmark = StatPath { file_count: 5 };
        let results = benchmark.run(test_fs.as_mut()).await;
        assert_eq!(results.len(), 5);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn open_file_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let benchmark = OpenFile { file_count: 5 };
        let results = benchmark.run(test_fs.as_mut()).await;
        assert_eq!(results.len(), 5);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn create_file_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let benchmark = CreateFile { file_count: 5 };
        let results = benchmark.run(test_fs.as_mut()).await;
        assert_eq!(results.len(), 5);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn unlink_file_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let benchmark = UnlinkFile { file_count: 5 };
        let results = benchmark.run(test_fs.as_mut()).await;
        assert_eq!(results.len(), 5);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn open_deeply_nested_file_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let benchmark = OpenDeeplyNestedFile { file_count: 5, depth: 3 };
        let results = benchmark.run(test_fs.as_mut()).await;
        assert_eq!(results.len(), 5);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }

    #[fuchsia::test]
    async fn git_status_test() {
        let mut test_fs = Box::new(TestFilesystem::new());
        let dts = DirectoryTreeStructure {
            files_per_directory: 2,
            directories_per_directory: 2,
            max_depth: 2,
        };
        let benchmark = GitStatus { dts, iterations: ITERATION_COUNT, stat_threads: 1 };
        let results = benchmark.run(test_fs.as_mut()).await;

        assert_eq!(results.len(), ITERATION_COUNT as usize);
        assert_eq!(test_fs.clear_cache_count().await, 0);
        test_fs.shutdown().await;
    }
}
