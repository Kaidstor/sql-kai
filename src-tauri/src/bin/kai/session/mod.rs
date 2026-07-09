//! Общий путь «алиас -> профиль -> vault -> соединение» для команд CLI:
//! `resolve` — алиас -> профиль, `pw` — пароли и vault, `connect` — соединение.

mod connect;
mod pw;
mod resolve;

pub use connect::{mux_ttl, open_for};
pub use pw::{read_new_password, unlock_vault, PwSource};
pub use resolve::{filter_profiles, resolve_profile};
