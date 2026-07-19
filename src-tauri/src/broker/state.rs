//! Владелец cli-сессий брокера: их жизненный цикл (TTL, sweep, clear) и
//! отметки активности, по которым holder решает самозавершиться.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::db::{self, TxStatus};

use super::protocol::BrokerSessionInfo;

/// Как долго cli-сессия живёт без запросов, прежде чем брокер её закроет.
const CLI_IDLE_TTL_SEC: u64 = 15 * 60;

/// …но сессия с открытой транзакцией — коротко: висящий BEGIN держит
/// блокировки и мешает vacuum'у на проде. Drop соединения её откатит.
const CLI_TX_IDLE_TTL_SEC: u64 = 180;

pub struct CliEntry {
    pub session: db::Session,
    /// Сериализует запросы к одной сессии (двум sql-kai одновременно нельзя).
    pub(super) busy: tokio::sync::Mutex<()>,
    pub(super) last_used: Mutex<Instant>,
}

impl CliEntry {
    pub(super) fn new(session: db::Session) -> Self {
        CliEntry {
            session,
            busy: tokio::sync::Mutex::new(()),
            last_used: Mutex::new(Instant::now()),
        }
    }
}

/// Владелец cli-сессий. std-Mutex, чтобы состав можно было чистить из
/// синхронных мест хоста (vault_lock); сами запросы держат только `busy`.
#[derive(Default)]
pub struct BrokerState {
    pub(super) cli: Mutex<HashMap<String, Arc<CliEntry>>>,
    /// Момент последнего запроса по сокету (любого). Holder по нему решает,
    /// что он никому не нужен, и самозавершается; GUI-брокер не читает.
    last_activity: Mutex<Option<Instant>>,
}

impl BrokerState {
    /// Снимок cli-сессий (для `sessions` и для фронтенда GUI).
    pub fn cli_sessions(&self) -> Vec<BrokerSessionInfo> {
        self.cli
            .lock()
            .unwrap()
            .values()
            .map(|e| {
                let idle = e.last_used.lock().unwrap().elapsed().as_secs();
                BrokerSessionInfo::from_session(&e.session, "cli", Some(idle))
            })
            .collect()
    }

    /// Закрывает все cli-сессии (lock vault, выход). true = что-то закрыли.
    /// Teardown сессий (kill ssh-туннеля) — вне лока, чтобы не держать
    /// остальных на syscall'ах.
    pub fn clear(&self) -> bool {
        let drained: Vec<Arc<CliEntry>> = {
            let mut map = self.cli.lock().unwrap();
            map.drain().map(|(_, e)| e).collect()
        };
        let had = !drained.is_empty();
        drop(drained);
        had
    }

    /// Убирает мёртвые и простоявшие дольше TTL сессии. true = что-то убрали.
    pub(super) fn sweep(&self) -> bool {
        let mut dead: Vec<Arc<CliEntry>> = Vec::new();
        let removed = {
            let mut map = self.cli.lock().unwrap();
            let before = map.len();
            map.retain(|_, e| {
                let ttl = match TxStatus::from_u8(e.session.tx.load(Ordering::Relaxed)) {
                    TxStatus::Idle => CLI_IDLE_TTL_SEC,
                    _ => CLI_TX_IDLE_TTL_SEC,
                };
                let live = !e.session.client.is_closed()
                    && e.last_used.lock().unwrap().elapsed().as_secs() < ttl;
                if !live {
                    dead.push(e.clone());
                }
                live
            });
            map.len() != before
        };
        drop(dead); // последние Arc-рефы гаснут вне лока
        removed
    }

    /// Отметить активность по сокету (см. `last_activity`).
    pub fn touch(&self) {
        *self.last_activity.lock().unwrap() = Some(Instant::now());
    }

    /// Нет сессий и нет запросов дольше `linger_sec` — holder'у пора выходить.
    /// Без единого запроса (None) простоем считается "бесконечность": holder
    /// вызывает touch() на старте, так что None здесь не встречается.
    pub fn is_idle(&self, linger_sec: u64) -> bool {
        if !self.cli.lock().unwrap().is_empty() {
            return false;
        }
        self.last_activity
            .lock()
            .unwrap()
            .map(|t| t.elapsed().as_secs() >= linger_sec)
            .unwrap_or(true)
    }

    pub(super) fn get_live(&self, profile_id: &str) -> Option<Arc<CliEntry>> {
        let map = self.cli.lock().unwrap();
        map.get(profile_id)
            .filter(|e| !e.session.client.is_closed())
            .cloned()
    }

    pub(super) fn remove_entry(&self, profile_id: &str) {
        self.cli.lock().unwrap().remove(profile_id);
    }
}
