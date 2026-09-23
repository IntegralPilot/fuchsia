# input_pipeline > How mouse movement scale

Reviewed on: 2022-12-16

Currently, in input pipeline, we support [2 type of mouse]:

- Relative: most common mouse in the market
- Absolute: the mouse on FEMU

## Relative Mouse

Relative Mouse reports how far the mouse moved in counts / dots. Depends on
sensor, mouse will report different [CPI (counts per inch)]. Sensors can
report more counts per inch means OS or applications can have more precise
data of movement in physical distance. Currently, "standard" mouse is 1000 CPI,
but it can be up to 25000 CPI.

`mouse_binding`
[converts counts to millimeters of the mouse's physical movement] based on the
CPI database once `mouse_binding` receives the event report from the mouse
driver. Inside input pipeline all movement is represented in millimeters.

[`PointerSensorScaleHandler`] scales the motion based on the speed. For
example, it may scale 10mm motion to 20mm. Currently, we have 3 speed class
to apply 3 different scaling curves.

[`PointerDisplayScaleHandler`] scales the motion based on the
[Device-Pixel-Ratio]. This helps cursor motion appear identical across
displays, with the same distance in display logical pixels (given different DPR
but identical physical size). For example, PointerDisplayScaleHandler will keep
10mm movement as 10mm movement on DPR=1.0 display (1080 monitor in 1080 logical
resolution); and scales 10mm movement to 20mm movement on DPR=2.0 display (4k
monitor in 1080 logical resolution).

Before the event is sent to to scenic, the movement will be converted to physical
pixel in [`MouseInjectorHandler`]. To convert physical distance in mm to physical
pixels, input pipeline need to compute `mm * logical_pixels_per_mm * DPR`.
Because DPR is applied in `PointerDisplayScaleHandler`, MouseInjectorHandler only
apply the logical_pixels_per_mm. Currently, we use a fixed number as mm to
logical pixel ratio, we may need to revisit to allow users to adjust the speed
of mouse move or allow different ratio per monitor model.



## Absolute Mouse

Currently, Absolute Mouses are only used for FEMU.

Absolute Mouse reports an absolute position of cursor in device coordinates,
device description will include the x y range. Input pipeline bypass
`PointerSensorScaleHandler` and `PointerDisplayScaleHandler`, then
[`MouseInjectorHandler::scale_absolute_position`] map the absolute position
to display coordinates based on the range of mouse movement from mouse
description.

<!-- xrefs -->

[2 type of mouse]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline/src/mouse_binding.rs?q=MouseLocation
[CPI (counts per inch)]: https://en.wikipedia.org/wiki/Computer_mouse#Mouse_speed
[to millimeters of the mouse's physical movement]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline/src/mouse_binding.rs?q=q="fn%20process_reports("
[`PointerSensorScaleHandler`]: /src/ui/lib/input_pipeline/docs/stages/pointer_motion_sensor_scale_handler.md
[`PointerDisplayScaleHandler`]: /src/ui/lib/input_pipeline/docs/stages/pointer_motion_display_scale_handler.md
[Device-Pixel-Ratio]: https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0174_scale_in_flatland?hl=en#device_pixel_ratio
[`MouseInjectorHandler`]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline/src/mouse_injector_handler.rs?q="fn%20relative_movement_mm_to_phyical_pixel("
[`MouseInjectorHandler::scale_absolute_position`]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/lib/input_pipeline/src/mouse_injector_handler.rs?q="fn%20scale_absolute_position("
