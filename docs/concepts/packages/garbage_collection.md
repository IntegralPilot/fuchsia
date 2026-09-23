# Garbage collection

Garbage Collection (GC) is the process for removing "unnecessary" blobs from a
device. It is usually performed to free up disk space so that new ephemeral
packages can be resolved or to OTA (which internally makes use of package
resolution).

## How to trigger

GC is implemented by pkg-cache, which exposes the functionality via [fuchsia.pkg.garbagecollector/Manager.GC][src_gc_fidl].

It can be triggered manually by running `fx shell pkgctl gc`.

It is automatically triggered (if necessary) at various points during OTA by the
[system-updater][src_system_updater_gc].

It is automatically triggered (if necessary) by the [system-update-checker][src_system_update_checker_gc],
which is the update checker used by engineering builds.

## The algorithm

The [current algorithm][src_pkg_cache_implementation] is based on the design
proposed by the [Open Package Tracking RFC][rfc_0217].


### Definitions

* **[Base packages][src_base_packages]**
  * The "base" or "system_image" package (identified by hash in the boot
  arguments) and the packages listed (by hash) in its `data/static_packages`
  file.
  * Generally intended to be the minimal set of packages required to run a
  particular configuration.
* **[Cache packages][src_cache_packages]**
  * The packages listed (by hash) in the "base" package's
    `data/cache_packages.json` file.
  * Conceptually packages that we would like to use without networking but still want the
    option to ephemerally update.
* **[Open packages][src_root_dir_cache]**
  * Packages whose package directories are [currently being served][src_pkg_cache_open_packages]
    by pkg-cache
* **[Writing index][src_writing_index]**
  * Packages whose blobs are currently being written as part of a resolve
* **[Retained index][src_retained_index]**
  * Packages that were/will be downloaded during OTA and are used during the OTA
    process or are necessary for the next system version.
  * [Manipulated][src_retained_index_fidl] by the
    [system-updater][src_system_updater_retained] during the OTA process to meet
    the OTA storage requirements.
* **Package blobs**
  * All the blobs required by a package.
  * The meta.far and content blobs, plus the package blobs of all subpackages,
    recursively.
  * As a result of this definition, protecting a package from GC protects all of
    its subpackages. The subpackages, as packages themselves, may or may not be
    protected independently of protection provided by a superpackage.

### Implementation

1. Fail if the current boot partition has not been marked healthy, to avoid
   deleting blobs needed by the previous system in case of fallback
2. Determine the set of all resident blobs, `Br`
3. Lock the Writing and Retained indices
4. Determine the set of all protected blobs, `Bp`, which is the package blobs of
   all of the following packages:
  * Base packages
  * Cache packages
  * Open packages
  * Writing index packages
  * Retained index packages
5. Delete the blobs of the set difference `Br - Bp`
6. Unlock the Writing and Retained indices


[rfc_0217]: /docs/contribute/governance/rfcs/0217_open_package_tracking.md

[src_gc_fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.space/space.fidl;l=18;drc=4eae0501e94a15718fea7e2410b55ef0e0fef979
[src_system_updater_gc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/system-updater/src/update.rs;l=1254;drc=f183b2bad311eb09c2be4d72411ddfd8e8db6e63
[src_system_update_checker_gc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/system-update-checker/src/check.rs;l=178;drc=f183b2bad311eb09c2be4d72411ddfd8e8db6e63
[src_pkg_cache_implementation]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/gc_service.rs;l=52;drc=c33e19363ac233f6be9d8cb9df460fc0e30551ad
[src_base_packages]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/base_packages.rs;l=36;drc=d7e51f5a7d6b09dcc24b684aa19ccda7eb6c0757
[src_cache_packages]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/system-image/src/cache_packages.rs;l=16;drc=156eb8041a38d097e146c99f54fcb06aaa3c7fe6
[src_retained_index]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/index/retained.rs;l=14;drc=e3239bddfc70bc7076a1c54f97f2408a95a4b207
[src_retained_index_fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.pkg/cache.fidl;l=292;drc=905c18c5ba0899a26963cb9dc5fcf1343068c0cc
[src_writing_index]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/index/writing.rs;l=21;drc=fe32c41fc7cec966080646c47cb1f4ff1874452f
[src_system_updater_retained]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/system-updater/src/update.rs;l=1402;drc=f183b2bad311eb09c2be4d72411ddfd8e8db6e63
[src_root_dir_cache]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/package-directory/src/root_dir_cache.rs;l=45;drc=a507f17eb78a603033c068d1f7f0d4d05c28cf20
[src_pkg_cache_open_packages]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/main.rs;l=187;drc=3adb26a7b1c4589755132a1fe3b918b6128bac6f
