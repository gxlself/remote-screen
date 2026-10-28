//! RSP/1 — the Remote Screen Protocol, minimal M0 subset.
//!
//! Transport is QUIC (see the `client` and `host` crates). This crate only
//! defines what goes over it and is free of I/O so it can be unit tested and
//! shared by the host agent, the desktop probe and the iOS client.
//!
//! Channels used in M0:
//! - **control**: the first bidirectional stream, opened by the client.
//!   Length-prefixed [`control::ControlMsg`] in both directions.
//! - **input**: a unidirectional stream opened by the client.
//!   Length-prefixed [`input::InputEvent`].
//! - **video**: QUIC DATAGRAM frames, each carrying one fragment of an encoded
//!   frame behind a [`video::FragmentHeader`]. The payload of a reassembled
//!   frame is an HEVC access unit in Annex-B format.

pub mod clock;
pub mod control;
pub mod input;
pub mod nal;
pub mod video;
pub mod wire;

/// ALPN token negotiated during the QUIC handshake.
pub const ALPN: &[u8] = b"rsp/1";
/// Version carried in `ClientHello` / `Hello`.
pub const PROTOCOL_VERSION: u16 = 1;
/// Default UDP port of the host agent.
pub const DEFAULT_PORT: u16 = 7400;
/// TLS server name the client uses. Identity is checked by certificate pinning,
/// not by name.
pub const SERVER_NAME: &str = "rs-host";

pub mod clipboard;
pub mod cursor;
pub mod errors;
pub mod features;
pub mod fec;
pub mod file;
pub mod file_name;
pub mod file_progress;
pub mod hid;
pub mod ids;
pub mod pairfile;
pub mod pairing;
pub mod pb;
pub mod pointer;
pub mod qr;
pub mod ratectl;
pub mod stats;
pub mod trust;

pub mod audio;
pub mod file_ctl;
