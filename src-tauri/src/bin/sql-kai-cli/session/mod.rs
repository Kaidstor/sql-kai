//! Общий путь «алиас -> профиль -> vault -> соединение» для команд CLI:
//! `resolve` — алиас -> профиль, `pw` — пароли и vault, `connect` — соединение,
//! `prod` — барьер на запись в профили с меткой production.

mod connect;
mod prod;
mod pw;
mod resolve;

pub use connect::{mux_ttl, open_for};
pub use prod::{
    authorize_prod_write, authorize_prod_write_ssh, guard_prod_write, guard_prod_write_by_id,
};
pub use pw::{
    headless_unlock_possible, read_new_password, unlock_vault, unlock_vault_headless, PwSource,
};
pub use resolve::{filter_profiles, resolve_profile};
