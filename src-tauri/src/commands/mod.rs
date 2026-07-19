//! Tauri-команды GUI, сгруппированные по областям: сессии (state +
//! подключение), vault, профили, персистентность (queries/settings/history/
//! лог), выполнение SQL, интроспекция каталога и прикладные мелочи.
//! Всё реэкспортируется плоско — invoke_handler в lib.rs и внешний код
//! обращаются к командам как `commands::<имя>`, как и до распила.

mod app;
mod introspect;
mod profiles;
mod query;
mod session;
mod storage;
mod vault;

pub use app::*;
pub use introspect::*;
pub use profiles::*;
pub use query::*;
pub use session::*;
pub use storage::*;
pub use vault::*;
