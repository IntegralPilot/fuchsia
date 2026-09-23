# TPM Tool
The `tpm_tool` is a development only tool intended to test and diagnose issues
with Trusted Platform Module or CR50 devices. It works by launching itself
inside the /core/ffx-laboratory collection and attempting to connect to to the
first available `fuchsia.tpm.TpmDevice` in the `/dev/class/tpm/` directory.

This tool is still very much a work in progress that will be added to as the
TPM stack is built for Fuchsia. This is intended for non-production
enviroments only mostly to assist in the development of the TPM stack.

## Getting Started
If you are wanting to test the tool in an emulated environment first setup your
build to utilize `x64` which will bring in the `tpm-lpc` driver.

```
fx set core.x64 --with //src/security/tools/tpm_tool
```

Next you will want to setup `swtpm` setup to passthrough to QEMU. This will
launch an emulated TPM that will be accessible via the `swtpm-sock`.
```
mkdir /tmp/emulated_tpm
swtpm socket --tpmstate dir=/tmp/emulated_tpm --ctrl type=unixio,path=/tmp/emulated_tpm/swtpm-sock --log level=20 --tpm2

```

Then you will want to setup QEMU to passthrough the `swtpm-sock` into the
guest fuchsia image. This will present itself as a LPC device that the
`tpm-lpc` driver can connect to.
```
fx qemu -N -- -chardev socket,id=chrtpm,path=/tmp/emulated_tpm/swtpm-sock -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0 ~
```

Next you will want to make sure you are serving package for the device:
```
fx serve
```

Finally you can run `tpm_tool` with the following command:
```
ffx component run /core/ffx-laboratory:tpm fuchsia-pkg://fuchsia.com/tpm_tool#meta/tpm-tool.cm --recreate
```

You should be able to search for results by running:
```
fx log | grep tpm_tool
```
