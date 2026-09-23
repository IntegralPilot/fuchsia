# Puppet Automation
hansens@google.com

## Introduction

Puppet is a systems configuration management platform. Its typical usecase is
for server configuration from a centralized puppet server. Puppet may operate in
one of three modes:
1. Client/server mode, useful for managing a large fleet of servers.
1. Standalone mode with a local module-base, or directory of importable puppet
   resources; useful when there are a lot of individual resources to manage.
1. Standalone mode from a single file; useful for smaller-in-scope manifests
   which may be contained in a single file.

Of these three modes, the third is used. As the set of standalone manifests
grows, there may come a time where it makes sense to provide a local
module-base.

The basic operation of puppet is to source configuration directives (known as a
manifest), calculate a catalog of applicable changes, and then apply the
catalog. Puppet supports a dry-run mode wherein the catalog of changes are
enumerated without application.

For more information about puppet, or its configuration syntax, see [http://puppet.com](https://www.puppet.com/docs/puppet/latest/puppet_index.html)

## Standalone Invocation

Normally, puppet files end in an extension of `.pp`. Operating in single-file
standalone mode, puppet may be invoked using:

```
puppet apply path/to/my-manifest.pp
```

Puppet will calculate the catalog, and then attempt to apply the changes
therein. Puppet itself requires no special permissions beyond the necessary
permissions to manipulate the catalog resources. I.e. if a root-owned
file/service/etc is managed, puppet needs to be run as root to affect the
necessary delta.

If no file is provided via argv, puppet will attempt to read the manifest from
stdin.

To run in dry-run mode, append the `--noop` flag using:

```
puppet apply --noop path/to/my-manifest.pp
```

This will calculate the catalog without actually applying any of the contained
changes. Typically, puppet need not run as root in dry-run mode unless root
privileges are required to *calculate* the catalog (e.g. read contents of files
owned by root with no read permissions).

In either mode, puppet will fail early if the manifest contains any kind of
syntax error.

## Developer Guide

See [http://puppet.com](https://www.puppet.com/docs/puppet/latest/puppet_index.html)
for a description of the puppet language.

Please run `puppet-lint` against any changes to puppet files and ensure there
are no findings.

Since you happen to be looking at a document about puppet, here's an example
manifest wrapped in a cli invocation which ensures `puppet-lint` is installed on
the local system:

```bash
sudo puppet apply <<EOF
package { 'puppet-lint':
  ensure => present,
}
EOF
```

