// Make the private module with patch version suffix available to other crates.
// This is similar to how it is handled for serde.
#[doc(hidden)]
pub mod __private20 {
    #[doc(hidden)]
    pub use crate::private::*;
}
