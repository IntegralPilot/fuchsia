# funnel

`funnel` takes a Fuchsia device in Product mode (i.e. not fastboot or
zedboot) and forwards the necessary ports from it over `ssh`. This allows for
you to develop in a remote workflow from your remote host.

## Usage

```
Usage: funnel -h <host> [-t <target-name>] [-r <repository-port>] [-l <log-level>] [-p <additional-port-forwards...>] [-w <wait-for-target-time>]

ffx Remote forwarding.

Options:
  -h, --host        the remote host to forward to
  -t, --target-name the name of the target to forward
  -r, --repository-port
                    the repository port to forward to the remote host
  -l, --log-level   the level to log at.
  -p, --additional-port-forwards
                    additional ports to forward from the remote host to the
                    target
  -w, --wait-for-target-time
                    time to wait to discover targets.
  --help            display usage information
```

## Notes

This binary is intended to replace the existing `fssh tunnel` command
([source](/tools/sdk-tools/fssh/tunnel/)).

## TODO

* Auto add and remove targets when the ssh connection is established/dropped.
