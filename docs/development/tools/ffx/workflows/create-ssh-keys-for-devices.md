# Create SSH keys for Fuchsia devices

To make an SSH connection to a Fuchsia device, some `ffx` commands (such as
[`ffx target show`][ffx-target-show] and [`ffx log`][ffx-log]) require
Fuchsia-specific SSH keys to be present on the host machine.

## Concepts

During development, one or more public SSH keys (listed in
`fuchsia_authorized_keys`) are loaded to a Fuchsia device, typically when a new
Fuchsia image is [flashed to the device][flash-device]. Once the device is
loaded with these public SSH keys, the `ffx` commands (running from the host
machine where a matching private SSH key is stored) can establish an SSH
connection to the device.

By default, Fuchsia-specific SSH keys are stored in the `$HOME/.ssh` directory
of the host machine, as shown below:

```none {:.devsite-disable-click-to-copy}
$HOME/.ssh/fuchsia_ed25519
$HOME/.ssh/fuchsia_authorized_keys
```

Note: These keys are not password protected. Don't use these keys for
non-development devices.

These files contain the following:

*   `fuchsia_ed25519`: A private SSH key. The content of this file must not be
    revealed or shared.
*   `fuchsia_authorized_keys`: A list of one or more authorized public SSH keys.

The `fuchsia_authorized_keys` file must include the public SSH key for
`fuchsia_ed25519`. During the flashing process, the `fuchsia_authorized_keys`
file gets uploaded from the host machine to the Fuchsia device.

If you have multiple development machines, it's recommended that the Fuchsia SSH
keys are synchronized across your development machines. This may require you to
copy the existing Fuchsia SSH keys files from one machine to another.

## Create the keys

To create the SSH keys if they are missing, run the following command:

```posix-terminal
ffx config check-ssh-keys --create
```

This command generates the keys if they are missing, or updates the public key
file to include the public key matching the private key if it's missing.

Alternatively, commands that initialize the Fuchsia device, such as `ffx target
flash` and `ffx emu start`, generate SSH keys if they are not found. The
location for the SSH key files can be configured using `ffx config set ssh.pub`
and `ffx config set ssh.priv`.

## Troubleshooting

If you are experiencing issues with SSH connectivity, `ffx` provides tools to
help you verify your SSH keys. For more information, see
[Troubleshoot SSH keys for Fuchsia devices][troubleshoot-ssh-keys].

<!-- Reference links -->

[ffx-target-show]: https://fuchsia.dev/reference/tools/sdk/ffx#show_8
[ffx-log]: https://fuchsia.dev/reference/tools/sdk/ffx#log_2
[flash-device]: ./flash-a-device.md
[troubleshoot-ssh-keys]: /docs/development/tools/ffx/workflows/troubleshoot-ssh-keys.md
