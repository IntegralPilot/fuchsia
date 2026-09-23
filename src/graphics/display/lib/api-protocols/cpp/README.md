# C++ wrappers for the display drivers stack's API protocols

## Conventions

### FIDL client conventions

### Variable names

Functions that make a single FIDL call use the following variable names.

* `fidl_transport_status` for one-way call results, which must be typed
  `fidl::OneWayStatus`
* `fidl_transport_result` for two-way call results, whose types start with
  either `fidl::WireResult` or `fdf::WireUnownedResult`
* `fidl_domain_result` for variables obtained by calling `value()` on
  `fidl_transport_result`
* `fidl_transport_error` for variables obtained by calling `error()` on
  `fidl_transport_result`
* `fidl_domain_error` for variables obtained by calling `error_value()` on
  `fidl_domain_result` (when `fidl_domain_result` is a `fit::result` type)

In functions that make multiple FIDL client calls, the "fidl_" variable name
prefix is replaced with a unique prefix for each call. For example, a function
that makes two one-way FIDL calls to `CheckAllBuffersAllocated()` and
`SetConstraints()` uses variables named
`check_all_buffers_allocated_transport_status` and
`set_constraints_transport_status`, instead of two variables named
`fidl_transport_status`.

#### Variable types

The variables described above use explicit types, instead of relying on `auto`
(type deduction) or CTAD (Class Template Argument Deduction).
`fidl_domain_result` and `fidl_domain_error` variables use mutable reference
types, to avoid copying.

Good type examples:

* `fidl::WireResult<fuchsia_hardware_backlight::Device::GetMaxAbsoluteBrightness>`
  for a `fidl_transport_result` variable
* `fit::result<zx_status_t>&`, for a `fidl_domain_result` variable
* `fit::result<zx_status_t,fuchsia_hardware_backlight::wire::DeviceGetMaxAbsoluteBrightnessResponse*>&`
  for a `fidl_domain_result` variable
* `fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionResponse&`
  for a `fidl_domain_result` variable
* `fuchsia_driver_framework::wire::NodeError&` for a `fidl_domain_error`
  variable

Bad type examples:

* `fidl::Status` - should be `fidl::OneWayStatus`
* `fdf::WireUnownedResult`, `fit::result&` - CTAD
* `const fidl::OneWayStatus` - may cause unnecessary copying
* `const fidl::WireResult<fuchsia_hardware_backlight::Device::GetMaxAbsoluteBrightness>`
  - may cause unnecessary copying
* `fit::result<zx_status_t>` - may cause unnecessary copying
* `const fit::result<zx_status_t>&` - may cause unnecessary copying
* `auto`, `auto&` -  type deduction

#### Variable initialization

`fidl_domain_result` variables are initialized by explicitly calling `value()`
on `fidl_transport_result` variables. Good initialization example:
`fit::result<zx_status_t> &fidl_domain_result = fidl_transport_result.value();`
Bad initialization example:
`fit::result<zx_status_t>& fidl_domain_result = *fidl_transport_result;`

If `value()` is called repeatedly on `fidl_transport_result` variables
(explicitly or implicitly via the -> operator), the repetition must be removed
by introducing a `fidl_domain_result` variable.

#### Error reporting

One-way FIDL call transport errors are logged according to the examples below.

* `fdf::error("FIDL error calling MethodName: {}", fidl_transport_status.error());`
  - in code that can use C++20 `<format>`-style logging
* `FDF_LOG(ERROR, "FIDL error calling MethodName: %s", fidl_transport_status.error().FormatDescription().c_str());`
  - in code that has to use `printf()`-style logging
* `ZX_ASSERT_MSG(fidl_transport_status.ok(), "FIDL error calling MethodName: %s", fidl_transport_status.error().FormatDescription().c_str())` - in code that cannot handle errors

Two-way FIDL calls are handled similarly, using `fidl_transport_result` instead
of `fidl_transport_status`.

FIDL domain errors are reported according to the examples below.

* `fdf::warn("MethodName failed: {}", zx::make_result(fidl_domain_result.error_value()));`
  - in code that can use C++20 `<format>`-style logging
* `FDF_LOG(WARN, "MethodName failed: %s", zx_status_get_string(fidl_domain_result.error_value()));`
  - in code that has to use `printf()`-style logging
* `ZX_ASSERT_MSG(fidl_domain_result.is_ok(), "MethodName failed: %s", zx_status_get_string(fidl_domain_result.error_value()));` - in code that cannot handle errors

#### Arena tags

`fdf::Arena` arenas instantiated by test code must use the `'TEST'` tag.
Production code must use the `'DISP'` tag. `fidl::Arena` arenas do not use tags.
