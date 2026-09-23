The fake haptics service's goal is to provide a component that offers a fake
[`fuchsia.hardware.haptics.Service` FIDL service](/sdk/fidl/fuchsia.hardware.haptics/haptics.fidl)
instance.

Currently, it returns `ZX_ERR_NOT_IMPLEMENTED` for all FIDL requests.