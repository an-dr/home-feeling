//! Hosts Lua guests on the bones bus with the contract of `bones-module-wasm`
//! (ADR-001, ADR-002): one bus endpoint per guest, the four host operations
//! (subscribe, publish, send, log), `core/lifecycle` events, a time budget and
//! quarantine, atomic reload, and deny-lists. Names nothing of any product, so
//! it can move into bones unchanged.

mod control;
mod guest;
mod host;
mod permissions;

pub use control::{GuestRecord, GuestState, LogLine, LuaControl};
pub use host::LuaHost;
pub use permissions::Permissions;

#[cfg(test)]
mod tests;
