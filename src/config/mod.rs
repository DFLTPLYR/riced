//! Configuration schema, discovery, migration, installation policy, and I/O.
pub mod discovery;
mod io;
pub mod migrate;
mod paths;
mod schema;
mod seed;
pub mod util;
pub(crate) use io::take_parse_error;
pub use schema::*;
