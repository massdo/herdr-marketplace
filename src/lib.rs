//! herdr-marketplace — search, read and install Herdr plugins.
//!
//! Domain stays free of I/O. Application use cases talk to the ports in
//! `application::ports`. Adapters own the network and the Herdr command line.

pub mod adapters;
pub mod application;
pub mod domain;
