What is F2FS?
=============
F2FS is the most commonly used log-structured file system in Linux. It supports
flash-friendly features such as adaptive logging, multi-head logging, fsync acceleration,
and node address translation.
For more information see: https://www.usenix.org/conference/fast15/technical-sessions/presentation/lee

Limitations
=============
* Features under development: xattr, fallocate

How to test
=============
Fuchsia filesystem tests
-------------
* Build configuration for fs-tests
(Set the product as "core_with_f2fs.x64" if you want to format /data to f2fs)
> $ fx set core.x64 --with //src/storage/f2fs:tests

* Run Fuchsia filesystem test suite with f2fs
> $ fx test f2fs-fs-tests f2fs-slow-fs-tests

* Run only unit tests
> $ fx test f2fs-unittest

Debian guest based Linux compatibility tests
-------------
* Prerequisite
> Generate Linux image

> $ ./src/virtualization/packages/debian_guest/build-image.sh \
>   prebuilt/virtualization/packages/debian_guest/images/x64 x64

* Build configuration
> $ fx set core.x64 --with //src/storage/f2fs:tests

* Run the test
> $ fx test f2fs-compatibility-test
