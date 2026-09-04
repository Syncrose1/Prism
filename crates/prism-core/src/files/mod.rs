//! Remote file access.
//!
//! Confinement to configured roots is the security boundary and lives in
//! [`path`]. Nothing in this module should reach the filesystem without a path
//! that came back from [`path::resolve`].

pub mod list;
pub mod path;
