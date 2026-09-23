## Adding new releases' fingerprints

First, find the key fingerprint for the release you're adding. For example, to
find the fingerprint for Ubuntu 24.04 LTS Noble Numbat:

```shell
$ cd tools/debroot
$ ./print_fingerprint.sh ubuntu noble
...
key id:
991BC93C
```

Replace `ubuntu` with `debian` for Debian releases.

Take the 8 digit key ID and add it to the appropriate variable in the section
below.

## Updating keyring files

GPG keyring file generated using:

```shell
$ cd tools/debroot/cmd
#            wheezy   jessie   jessie-stable stretch  buster
DEBIAN_KEYS="46925553 2B90D010 518E17E1      1A7B6500 3CBBABEE"
#            trusty   xenial   bionic   focal    jammy    noble
UBUNTU_KEYS="437D05B5 437D05B5 C0B21F32 C0B21F32 991BC93C 991BC93C"
$ gpg --keyserver keyserver.ubuntu.com --recv-keys $DEBIAN_KEYS $UBUNTU_KEYS
$ gpg --output ./debian-archive-keyring.gpg --export $DEBIAN_KEYS
$ gpg --output ./ubuntu-archive-keyring.gpg --export $UBUNTU_KEYS
```

## Defining lock files for new releases

Continuing the example for Ubuntu 24.04 LTS Noble Numbat, define a
`//tools/noble/cmd/noble.yml` file.

Run this tool:

```shell
$ cd tools/debroot/cmd
$ fx host-tool debroot update -config noble.yml -lock noble.lock
```

This will create a lockfile that can be used in a CIPD builder to download the
sysroot.
