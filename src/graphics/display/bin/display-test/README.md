# display-test

This manual test exercises display functionality.

This will be replaced by `//src/graphics/display/bin/display-tool/meta/display-tool`
after all functionality is ported over.

## Running
There are two ways to run this test, which are described below.  Either way, be
aware that this test is not hermetic; because `fuchsia.hardware.display.Coordinator`
does not support multiple simultaneous clients, you need to ensure that there is
no other client.  For example, on products that include a session, it should
suffice to run these commands first:
```
ffx session stop
fx shell killall scenic.cm
```

### Method 1: `fx shell`
Product assembly includes the test in the `bootstrap_eng` assembly bundle.  So,
on an `eng` build you can run the following (after first killing Scenic, etc.):
```
fx shell display-test  # plus additional options; see below.
```

### Method 2: `fx test`
The test can also be run via `fx test` in the usual way:
- ensure that your `fx set` includes `//src/graphics/display/bin/display-test:display-test-test`
- ensure there is no other display client (by killing
Scenic, etc.)
- then run:
```
fx test -o display-test-test
```
Compared to the `fx shell` approach, it less convenient to use command line
options: you must modify the `program` section of `display-test.cml`, and then
rebuild the test.  For example:
```
    program: {
        binary: "bin/display-test",
        args: [ "--bundle", "1" ],
    },
```

## "Test Bundles"
Different display hardware has different functionality.  For example, some
hardware might support scaling and/or rotation, and hardware will vary in the
number of supported layers.  The test supports different modes ("bundles") which
allow exercising various functionality on hardware that supports it.  By default
a bundle is chosen based on the type of hardware.  However, it can also be
specified via the `--bundle` command line argument.  For example:
```
fx shell "display-test --bundle 1"  # should show an animated checkerboard
```

## Usage
```
display-test [OPTIONS]

    --dump                   : print properties of attached display
    --mode-set D N           : Set Display D to mode N (use dump option for choices)
    --format-set D N         : Set Display D to format N (use dump option for choices)
    --grayscale              : Display images in grayscale mode (default off)
    --num-frames N           : Run test in N number of frames (default 120)
                               N can be an integer or 'infinite'
    --delay N                : Add delay (ms) between Vsync complete and next configuration
    --capture                : Capture each display frame and verify
    --fgcolor 0xaarrggbb     : Set foreground color
    --bgcolor 0xaarrggbb     : Set background color
    --preoffsets x,y,z       : set preoffsets for color correction
    --postoffsets x,y,z      : set postoffsets for color correction
    --coeff c00,c01,...,,c22 : 3x3 coefficient matrix for color correction
    --enable-alpha           : Enable per-pixel alpha blending.
    --opacity o              : Set the opacity of the screen
                               <o> is a value between [0 1] inclusive
    --enable-compression     : Enable framebuffer compression.
    --apply-config-once      : Apply configuration once in single buffer mode.
    --clamp-rgb c            : Set minimum RGB value [0 255].
    --configs-per-vsync n    : Number of configs applied per vsync
    --pattern pattern        : Image pattern to use - 'checkerboard' (default) or 'border'

    Test Modes:

    --bundle N       : Run test from test bundle N as described below
                       0: Display a single pattern using single buffer
                       1: Flip between two buffers to display a pattern
                       2: Run the standard Intel-based display tests. This includes
                          hardware composition of 1 color layer and 3 primary layers.
                          The tests include alpha blending, translation, scaling
                          and rotation
                       3: 4 layer hardware composition with alpha blending
                          and image translation
                       4: Blank the screen and sleep for --num-frames.
                       (default: bundle 2, "INTEL")
    --help           : Show this help message
```