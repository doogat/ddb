/// Library surface for integration tests.
///
/// The CLI binary (`main.rs`) is the primary entry point. This library
/// target exposes items that `tests/` integration tests need to access.
mod warnings;

pub mod commands {
    /// Warning formatting helpers.
    pub mod crud {
        pub use crate::warnings::write_warnings;
    }
}
