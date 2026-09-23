# escher/base/

## Lifecycle Management

Provides base classes which are used for lifecycle management, working with `fxl::RefPtr` to provide
customizable behavior when the object's ref-count becomes zero.

`Reffable` adds a virtual `OnZeroRefCount()` method which is invoked when the ref-count becomes
zero.

`Ownable` and `Owner` build upon this, so that when an `Ownable` object is no longer referenced,
it is passed to the `Owner` to decide what to do with it.  This is useful e.g. for disposing of
Vulkan resources which cannot be destroyed immediately because they may still be in use by the GPU.

`Ownable` also makes use of the simple RTTI (run-time type information) system described below.

## Simple RTTI

Fuchsia policy doesn't allow the use of standard C++ RTTI functionality.  Escher provides a very
limited opt-in RTTI mechanism, tailored to Escher's specific needs requirements.


## Misc

- `make.h`
  - defines `Make()` function, a more concise synonym for `fxl::internal::MakeRefCountedHelper<T>()`.
