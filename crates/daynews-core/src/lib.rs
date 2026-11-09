// SPDX-License-Identifier: Apache-2.0
//! Fleet Reader state. Native platforms use an asynchronous database actor; the web build
//! retains its OPFS transport until transferable worker jobs are available there.
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::*;
#[cfg(any(target_arch = "wasm32", test))]
#[allow(dead_code, unused_imports)]
mod legacy;
#[cfg(target_arch = "wasm32")]
pub use legacy::*;

/// The library's file name in the app's data directory ([`store_dir`]).
pub const LIBRARY: &str = "library.db";
