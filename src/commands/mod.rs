//! The thin CLI surface. By design only the handful of actions where *typing
//! beats pointing* live here - open and list. Everything you'd otherwise have
//! to look up (passwords, tunnels, adding connections) lives in the TUI, where
//! there's nothing to memorize.

pub mod connect;
pub mod ls;
pub mod selfcmd;
