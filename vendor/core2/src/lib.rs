//! csgrs 0.20.1 depends on `core2` for `core2::io::Cursor`, and every release of `core2` has been yanked, so cargo cannot select
//! it. Lamina is not `no_std`, so `std::io` is the same thing. Patched in by the root `Cargo.toml`; drop this when csgrs stops
//! asking for `core2`.
#![forbid(unsafe_code)]

pub mod io {
    pub use std::io::*;
}
