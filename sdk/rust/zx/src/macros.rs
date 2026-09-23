// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Implements the common traits for a Handle newtype struct
macro_rules! impl_handle_based {
    ($type_name:path) => {
        impl $crate::AsHandleRef for $type_name {
            fn as_handle_ref(&self) -> $crate::HandleRef<'_> {
                self.0.as_handle_ref()
            }
        }

        impl From<$crate::NullableHandle> for $type_name {
            fn from(handle: $crate::NullableHandle) -> Self {
                $type_name(handle)
            }
        }

        impl From<$crate::Handle> for $type_name {
            fn from(handle: $crate::Handle) -> Self {
                $type_name(handle.into())
            }
        }

        impl From<$type_name> for $crate::NullableHandle {
            fn from(x: $type_name) -> $crate::NullableHandle {
                x.0
            }
        }

        impl TryFrom<$type_name> for $crate::Handle {
            type Error = $crate::Status;

            fn try_from(x: $type_name) -> Result<$crate::Handle, Self::Error> {
                Ok($crate::Handle::try_from(x.0)?)
            }
        }

        impl $type_name {
            delegated_concrete_handle_based_impls!(Self);
        }
    };
    ($type_name:ident, [$($impl_generics:tt)*], [$($ty_generics:tt)*]) => {
        impl<$($impl_generics)*> crate::AsHandleRef for $type_name<$($ty_generics)*> {
            fn as_handle_ref(&self) -> crate::HandleRef<'_> {
                self.0.as_handle_ref()
            }
        }

        impl<$($impl_generics)*> AsRef<crate::NullableHandle> for $type_name<$($ty_generics)*> {
            fn as_ref(&self) -> &crate::NullableHandle {
                &self.0
            }
        }

        impl<$($impl_generics)*> From<crate::NullableHandle> for $type_name<$($ty_generics)*> {
            fn from(handle: crate::NullableHandle) -> Self {
                $type_name(handle, ::std::marker::PhantomData)
            }
        }

        impl<$($impl_generics)*> From<$crate::Handle> for $type_name<$($ty_generics)*> {
            fn from(handle: $crate::Handle) -> Self {
                $type_name(handle.into(), ::std::marker::PhantomData)
            }
        }


        impl<$($impl_generics)*> From<$type_name<$($ty_generics)*>> for crate::NullableHandle {
            fn from(x: $type_name<$($ty_generics)*>) -> crate::NullableHandle {
                x.0
            }
        }

        impl<$($impl_generics)*> TryFrom<$type_name<$($ty_generics)*>> for $crate::Handle {
            type Error = $crate::Status;

            fn try_from(x: $type_name<$($ty_generics)*>) -> Result<$crate::Handle, Self::Error> {
                Ok($crate::Handle::try_from(x.0)?)
            }
        }

        impl<$($impl_generics)*> $type_name<$($ty_generics)*> {
            delegated_concrete_handle_based_impls!(|h| Self(h, ::std::marker::PhantomData));
        }
    };
}

macro_rules! delegated_concrete_handle_based_impls {
    ($ctor:expr) => {
        /// Return the handle's integer value.
        pub fn raw_handle(&self) -> $crate::sys::zx_handle_t {
            self.0.raw_handle()
        }

        // TODO(https://fxbug.dev/465766514) remove with NullableHandle migration
        pub fn invalid() -> Self {
            $ctor($crate::NullableHandle::invalid())
        }

        // TODO(https://fxbug.dev/465766514) remove with NullableHandle migration
        pub fn is_invalid(&self) -> bool {
            self.0.is_invalid()
        }

        /// Return the raw handle's integer value without closing it when `self` is dropped.
        pub fn into_raw(self) -> $crate::sys::zx_handle_t {
            self.0.into_raw()
        }

        /// Returns an [Unowned] referring to this handle.
        pub fn unowned(&self) -> $crate::Unowned<'_, Self> {
            $crate::Unowned::cast(self.as_handle_ref())
        }

        /// Returns a [HandleRef] referring to this handle.
        pub fn as_handle_ref(&self) -> $crate::HandleRef<'_> {
            self.0.as_handle_ref()
        }

        // TODO(https://fxbug.dev/384752843) only generate on types that can be duped
        /// Wraps the
        /// [`zx_handle_duplicate`](https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_duplicate)
        /// syscall.
        pub fn duplicate_handle(&self, rights: $crate::Rights) -> Result<Self, $crate::Status> {
            self.0.duplicate_handle(rights).map($ctor)
        }

        /// Wraps the
        /// [`zx_handle_replace`](https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_replace)
        /// syscall.
        pub fn replace_handle(self, rights: $crate::Rights) -> Result<Self, $crate::Status> {
            self.0.replace_handle(rights).map($ctor)
        }

        /// Converts the value into its inner handle.
        pub fn into_handle(self) -> $crate::NullableHandle {
            self.into()
        }

        /// Wraps the
        /// [`zx_object_signal`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_signal)
        /// syscall.
        pub fn signal(
            &self,
            clear_mask: $crate::Signals,
            set_mask: $crate::Signals,
        ) -> Result<(), $crate::Status> {
            self.0.signal(clear_mask, set_mask)
        }

        /// Wraps the
        /// [`zx_object_wait_one`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_wait_one)
        /// syscall.
        pub fn wait_one(
            &self,
            signals: $crate::Signals,
            deadline: $crate::MonotonicInstant,
        ) -> $crate::WaitResult {
            self.0.wait_one(signals, deadline)
        }

        /// Wraps the
        /// [`zx_object_wait_async`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_wait_async)
        /// syscall.
        pub fn wait_async(
            &self,
            port: &$crate::Port,
            key: u64,
            signals: $crate::Signals,
            options: $crate::WaitAsyncOpts,
        ) -> Result<(), $crate::Status> {
            self.0.wait_async(port, key, signals, options)
        }

        /// Return a `WaitItem` for this handle and the provided `signals` to use with
        /// `object_wait_many`.
        pub fn wait_item(&self, signals: $crate::Signals) -> $crate::WaitItem<'_> {
            self.0.wait_item(signals)
        }

        /// Wraps a call to the
        /// [`zx_object_get_property`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_property)
        /// syscall for the `ZX_PROP_NAME` property.
        pub fn get_name(&self) -> Result<$crate::Name, $crate::Status> {
            self.0.get_name()
        }

        /// Wraps a call to the
        /// [`zx_object_set_property`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_set_property)
        /// syscall for the `ZX_PROP_NAME` property.
        pub fn set_name(&self, name: &$crate::Name) -> Result<(), $crate::Status> {
            self.0.set_name(name)
        }

        /// Wraps the
        /// [`zx_object_get_info`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info)
        /// syscall for the `ZX_INFO_HANDLE_BASIC` topic.
        pub fn basic_info(&self) -> Result<$crate::HandleBasicInfo, $crate::Status> {
            self.0.basic_info()
        }

        /// Wraps the
        /// [`zx_object_get_info`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info)
        /// syscall for the `ZX_INFO_HANDLE_COUNT` topic.
        pub fn count_info(&self) -> Result<$crate::HandleCountInfo, $crate::Status> {
            self.0.count_info()
        }

        /// Returns the [Koid] for the object referred to by this handle.
        pub fn koid(&self) -> Result<$crate::Koid, $crate::Status> {
            self.0.koid()
        }
    };
}

/// Convenience macro for creating get/set property functions on an object.
///
/// This is for use when the underlying property type is a simple raw type.
/// It creates an empty 'tag' struct to implement the relevant PropertyQuery*
/// traits against. One, or both, of a getter and setter may be defined
/// depending upon what the property supports. Example usage is
/// unsafe_handle_propertyes!(ObjectType[get_foo_prop,set_foo_prop:FooPropTag,FOO,u32;]);
/// unsafe_handle_properties!(object: Foo,
///     props: [
///         {query_ty: FOO_BAR, tag: FooBarTag, prop_ty: usize, get:get_bar},
///         {query_ty: FOO_BAX, tag: FooBazTag, prop_ty: u32, set:set_baz},
///     ]
/// );
/// And will create
/// Foo::get_bar(&self) -> Result<usize, Status>
/// Foo::set_baz(&self, val: &u32) -> Result<(), Status>
/// Using Property::FOO as the underlying property.
///
///  # Safety
///
/// This macro will implement unsafe traits on your behalf and any combination
/// of query_ty and prop_ty must respect the Safety requirements detailed on the
/// PropertyQuery trait.
macro_rules! unsafe_handle_properties {
    (
        object: $object_ty:ty,
        props: [$( {
            query_ty: $query_ty:ident,
            tag: $query_tag:ident,
            prop_ty: $prop_ty:ty
            $(,get: $get:ident)*
            $(,set: $set:ident)*
            $(,)*
        }),*$(,)*]
    ) => {
        $(
            struct $query_tag {}
            unsafe impl $crate::PropertyQuery for $query_tag {
                const PROPERTY: $crate::Property = $crate::Property::$query_ty;
                type PropTy = $prop_ty;
            }

            $(
                impl $object_ty {
                    pub fn $get(&self) -> Result<$prop_ty, $crate::Status> {
                        self.0.get_property::<$query_tag>()
                    }
                }
            )*

            $(
                impl $object_ty {
                    pub fn $set(&self, val: &$prop_ty) -> Result<(), $crate::Status> {
                        self.0.set_property::<$query_tag>(val)
                    }
                }
            )*
        )*
    }
}

// Creates associated constants of TypeName of the form
// `pub const NAME: TypeName = TypeName(path::to::value);`
// and provides a private `assoc_const_name` method and a `Debug` implementation
// for the type based on `$name`.
// If multiple names match, the first will be used in `name` and `Debug`.
#[macro_export]
macro_rules! assoc_values {
    ($typename:ident, [$($(#[$attr:meta])* $name:ident = $value:path;)*]) => {
        #[allow(non_upper_case_globals)]
        impl $typename {
            $(
                $(#[$attr])*
                pub const $name: $typename = $typename($value);
            )*

            fn assoc_const_name(&self) -> Option<&'static str> {
                match self.0 {
                    $(
                        $(#[$attr])*
                        $value => Some(stringify!($name)),
                    )*
                    _ => None,
                }
            }
        }

        impl ::std::fmt::Debug for $typename {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(concat!(stringify!($typename), "("))?;
                match self.assoc_const_name() {
                    Some(name) => f.write_str(&name)?,
                    None => ::std::fmt::Debug::fmt(&self.0, f)?,
                }
                f.write_str(")")
            }
        }
    }
}

/// Declare a struct that needs to be statically aligned with another, equivalent struct. The syntax
/// is the following:
/// rust```
/// static_assert_align! (
///     #[derive(Trait1, Trait2)]
///     <other_aligned_struct> pub struct MyStruct {
///         field_1 <equivalent_field1_on_other_struct>: bool,
///         field_2 <equivalent_field2_on_other_struct>: u32,
///         special_field_3: [u8; 10],  // This field will be ignored when comparing alignment.
///     }
/// );
/// ```
macro_rules! static_assert_align {
    (
        $(#[$attrs:meta])* <$equivalent:ty> $vis:vis struct $struct_name:ident {
            $($field_vis:vis $field_ident:ident $(<$field_eq:ident>)?: $field_type:ty,)*
        }
    ) => {
        $(#[$attrs])* $vis struct $struct_name {
            $($field_vis $field_ident: $field_type,)*
        }

        static_assertions::assert_eq_size!($struct_name, $equivalent);
        $(_static_assert_one_field!($struct_name, $field_ident, $equivalent $(, $field_eq)?);)*
    }
}

/// Internal macro used by [static_assert_align].
macro_rules! _static_assert_one_field {
    ($struct_1:ty, $field_1:ident, $struct_2:ty) => {};
    ($struct_1:ty, $field_1:ident, $struct_2:ty, $field_2:ident) => {
        static_assertions::const_assert_eq!(
            std::mem::offset_of!($struct_1, $field_1),
            std::mem::offset_of!($struct_2, $field_2)
        );
    };
}
