#[doc(hidden)]
pub use crate::custom::{ref_cast_custom, CurrentCrate, RefCastCustom};
#[doc(hidden)]
pub use crate::layout::{assert_layout, Layout, LayoutUnsized};
#[doc(hidden)]
pub use crate::trivial::assert_trivial;
#[doc(hidden)]
pub use core::mem::transmute;

// Make the private module with patch version suffix available to other crates.
// This is similar to how it is handled for serde.
#[doc(hidden)]
pub mod __private25 {
    #[doc(hidden)]
    pub use crate::private::*;
}