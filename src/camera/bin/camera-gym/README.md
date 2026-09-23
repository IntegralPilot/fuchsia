# Fuchsia Camera Gym (camera3 config exerciser)

Example app to exercise the various stream configurations available for Sherlock.

## Flatland Instruction

See go/try-flatland

Note: Currently flatland support is limited to showing only camera streams on supported physical device.
Bug that tracks full flatland migration for camera-gym is https://fxbug.dev/42073155.

### Run camera-gym

Note: Component `camera-gym` now runs using Flatland. To switch to GFX, update `fx vendor google camera-gym` to launch component name `camera-gym-gfx`.

```
> fx vendor google camera-gym
```

## How To Select Config

TBD

## Building camera-gym for GFX

Include the `camera-gym` package and disable flatland:

```
> fx set <product>.<arch> --with //src/camera/bin/camera-gym --args=use_flatland_by_default=false
```

### How to Run in Automatic Mode

Flatland

```
> ffx session add fuchsia-pkg://fuchsia.com/camera-gym#meta/camera-gym.cm
```

GFX

```
> ffx session add fuchsia-pkg://fuchsia.com/camera-gym#meta/camera-gym-gfx.cm
```

### How to Run in Manual Mode

Currently only supported in GFX

```
> ffx session add fuchsia-pkg://fuchsia.com/camera-gym#meta/camera-gym-gfx-manual.cm
```

### camera-gym-ctl

camera-gym-ctl is a tool that can interact with a camera-gym in manual mode.
The tool can be activated from `ffx component explore`.

Start by finding the moniker of the camera-gym-gfx-manual that you started above.

```
> ffx component show camera-gym-gfx-manual
               Moniker:  /core/session-manager/session:session/workstation_session/login_shell/application_shell:1/elements:vd5ioej4lvcshlgb
                   URL:  fuchsia-pkg://fuchsia.com/camera-gym#meta/camera-gym-gfx-manual.cm
           Instance ID:  None
                  Type:  CML Component
       Component State:  Resolved
 Incoming Capabilities:  /svc/fuchsia.camera3.DeviceWatcher
                         /svc/fuchsia.element.GraphicalPresenter
                         /svc/fuchsia.sysmem.Allocator
                         /svc/fuchsia.tracing.provider.Registry
                         /data
                         /svc/fuchsia.logger.LogSink
  Exposed Capabilities:  fuchsia.camera.gym.Controller
                         fuchsia.component.Binder
           Merkle root:  00b7161f6f83225e1e525819d0fdc2ac17f927adbec8b618e6e536cc44c2046a
       Execution State:  Running
          Start reason:  '/core/session-manager/session:session/workstation_session/login_shell/application_shell:1/elements:vd5ioej4lvcshlgb' requested capability 'fuchsia.component.Binder'
         Running since:  2022-09-21 17:13:10.895060505 UTC
                Job ID:  610356
            Process ID:  610388
 Outgoing Capabilities:  debug
                         fuchsia.camera.gym.Controller
                         fuchsia.modular.Lifecycle
> ffx component explore /core/session-manager/session:session/workstation_session/login_shell/application_shell:1/elements:vd5ioej4lvcshlgb
$ camera-gym-ctl --set-description=0
OK
$
```

> NOTE: command prompts below beginning with `$` denote the `ffx component
explore` shell.

### How to Run Manual Mode Scripts

```
test_simple.sh
```

Running this bash script will cycle through all configs and all streams forever.

```
test_crop.sh
```

Running this bash script will cycle through crop settings in steps of 1/40's.

### How to turn text descriptions on/off

#### Turn off descriptions

```
$ camera-gym-ctl --set-description=0
```

#### Turn on descriptions

```
$ camera-gym-ctl --set-description=1
```

### How to Capture a Frame in Manual Mode

Caveat (b/200839146): The current frame capture feature only works while a
single stream is running. Trying to do so with multiple streams running at the
same time will result capturing from a stream randomly selected from all of
those running.

#### Capture a frame

```
$ camera-gym-ctl --capture-frame=0
```

#### Review how many frames were captured

```
$ ls /ns/tmp
```

#### Copy out frames that were captured

TODO(https://fxbug.dev/42051182): Update this once `ffx component copy` is mature.

#### Post process NV12 raw dumps to PNG and view the PNG images

```
cd /tmp/my_dest_dir
ls -1 image*.nv12 | sed -e 's@\(.*_\)\([0-9][0-9]*x[0-9][0-9]*\)\(.*\).nv12@ffmpeg -f rawvideo -pixel_format nv12 -video_size \2 -i \1\2\3.nv12 \1\2\3.png@' > CONVERT.SH
. CONVERT.SH
```

#### Visually review converted PNG images (if on same system)

```
eog *.png
```

#### Visually review converted PNG images (if on another system)

```
rsync -av remote.system.somewhere.com:/tmp/my_dest_dir /tmp/.
cd /tmp/my_dest_dir
eog *.png
```
