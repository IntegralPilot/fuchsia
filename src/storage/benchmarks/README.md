# Fuchsia Filesystem Benchmarks

## Benchmarks

There are 12 benchmarks that get run for every filesystem. The currently supported filesystems are
Fxfs, F2fs, Memfs, and Minfs.

### IO Benchmarks
The IO benchmarks are all of the combinations of read/write, sequential/random, and warm/cold. Every
read/write call uses an 8KiB buffer and each operation is performed 1024 times spread across an 8MiB
file. The benchmarks measure how long each read/write operation takes.
* **Read**: makes `pread` calls to the file.
  * **Cold**: the file is written, the filesystem is remounted, then we measure the time to read the
    file.
  * **Warm**: same as cold but without remounting.
* **Write**: makes `pwrite` call to the file.
  * **Cold**: we create a new file, then measure the time to write to it.
  * **Warm**: we create a new file, write to it (to populate it), then measure the time to write to
    it a second time.
* **Sequential**: the reads/writes are performed sequentially from the start of the file to the end
  of the file.
* **Sparse**: the reads/writes are performed linearly but sparsely, with large gaps where no data is
  read/written between the accessed regions.
* **Random**: the reads/writes are performed randomly across the entire file. Every part of the file
  is accessed exactly once.
* **Fsync**: the fsync is performed for every write.

### WalkDirectoryTree Benchmarks
The `WalkDirectoryTree` benchmarks measure how long it takes to walk a directory tree with POSIX
`readdir` calls. The directory tree consists of 62 directories and 189 files and is traversed 20
times by the benchmarks. The "cold" variant of the benchmarks remounts the filesystem between each
traversal and the "warm" variant does not.

### OpenFile Benchmarks
The `OpenFile` benchmark measures how long it takes for a filesystem to open a file.

The `OpenDeeplyNestedFile` benchmark expands on the `OpenFile` benchmark by placing the file several
directories deep and then opening it from the root of the filesystem. When compared to the
`OpenFile` benchmark, the `OpenDeeplyNestedFile` captures how long it takes the filesystem to
internally traverse directories.

### CreateFile Benchmarks
The `CreateFile` benchmark measures how long it takes for a filesystem to create a file.

### UnlinkFile Benchmarks
The `UnlinkFile` benchmark measures how long it takes for a filesystem to unlink a file.

### StatPath Benchmark
The `StatPath` benchmark measure how long it takes to call `stat` on a path to a file.

### GitStatus Benchmark
The `GitStatus` benchmark mimics the filesystem usage pattern of running `git status`. The benchmark
contains 2 phases:
* Phase 1: Calling `fstatat` on all of the files in the index to see if any of them have changed.
  All of the `fstatat` calls happen relative to the top level directory.
* Phase 2: Walking the directory tree in a recursive DFS order to see if any new files were added.

### Blob Benchmarks
The `PageInBlob` benchmarks measure page fault times for mmap'ed blobs.
* `PageInBlobSequentialUncompressed` creates an incompressible blob and pages it in by sequentially
  accessing each page.
* `PageInBlobSequentialCompressed` creates a compressible blob and pages it in by sequentially
  accessing each page.
* `PageInBlobRandomCompressed` creates a compressible blob and randomly accesses 60% of the pages in
  a way similar to executing an executable. Only 60% of pages are accessed to try to mimic an
  executable starting.

The blob writing benchmarks measure how long it takes to write blobs. This is important for both
fast updates in production and development workflows.
* `WriteBlob` writes a single realistically compressible blob to a blob filesystem.
* `WriteRealisticBlobs` creates several realistically compressible blobs with varying sizes and
  concurrently writes 2 blobs to a blob filesystem. This ideally mimics how pkg-cache writes blobs.
  The benchmark measure how long it takes to write all of the blobs.

The `OpenAndGetVmo` benchmarks measure how long it takes to open a package and
get the VMO for a blob within it. Notably, `OpenAndGetVmo` goes through the
package directory as opposed to directly opening the blob through Blobfs/Fxblob,
and thus allows us to more accurately measure open times via SWD.

* `OpenAndGetVmoMetaFile` creates and opens a metafile (prefix "meta/" in the resource path).
* `OpenAndGetVmoContentBlob` creates and opens a content blob (non-"meta" prefix in the
  resource path i.e. "data/").

## "Cold" Benchmarks
At the beginning of most benchmarks is a setup phase that creates files within the filesystem.
Simply closing all handles to those files doesn't guarantee that the filesystem will immediately
clear all caches related to those files. If the caches aren't cleared then the benchmark may only
ever hit cached (warm) data. "Cold" (uncached) read benchmarks remount the Fuchsia filesystem before
doing their read operations. Remounting the filesystem guarantees that all data related the file
that isn't normally cached gets dropped.

### Memfs and Cold Writes
When cold writing to memfs, the kernel needs to allocate pages for the VMO backing the file as the
pages are used. This causes cold writes to be slower than warm writes which have the pages already
allocated.

### Verity Benchmarks
The verity benchmarks are implemented as a Starnix integration test. The test launches a Linux
program that creates and writes a new file and then enables verity on it. The test then clears
the filesystem cache and then launches a second Linux program that performs a verified read on
the verity-enabled file. The benchmark has two metrics: 1) The time to enable verity 2) Time to
perform a verified read.

## Framework
The Fuchsia Filesystem Benchmarks use a custom framework for timing filesystem operations.
Filesystems hold state external to the `read` or `write` operations being benchmarked which can lead
to drastically different timings between consecutive operations. For other performance tests, we
want to treat the initial one or more iterations as warm-up iterations and drop their timings. (For
example, for some IPC performance tests, the initial iteration doesn't complete until a subprocess
has finished starting up, making it much slower than the later iterations.) These storage tests
differ in that we don't want to drop the initial iterations' timings.

> Ex. On the first `read` operation to a file in Minfs, Minfs reads the entire file into memory and
> each subsequent `read` is served from memory. The warm-up phase of [fuchsia-criterion] would hide
> the extremely slow `read` call.

## Running the Benchmarks
1. Include `//src/storage/benchmarks` in `fx set`.
2. Run `fx test fuchsia-pkg://fuchsia.com/storage-benchmarks#meta/storage-benchmarks.cm`
3. If you are running on an emulator, you will need to run the emulator in UEFI mode:

```sh
ffx emu start --net tap -H --uefi \
  --vbmeta-key \
    third_party/android/platform/external/avb/test/data/testkey_atx_psk.pem \
  --vbmeta-key-metadata \
    third_party/android/platform/external/avb/test/data/atx_metadata.bin
```

The set of benchmarks and filesystems can filtered with the `--filter` flag.

[fuchsia-criterion]: https://fuchsia.googlesource.com/fuchsia/+/HEAD/src/developer/fuchsia-criterion
