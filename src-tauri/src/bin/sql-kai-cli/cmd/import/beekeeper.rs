//! Чтение подключений Beekeeper Studio: sqlite `app.db` (таблицы
//! `saved_connection` + `connection_folder`) рядом с файлом `.key`.
//!
//! Секреты в app.db зашифрованы npm-пакетом simple-encryptor:
//! `hmac_hex(64) + iv_hex(32) + base64(AES-256-CBC(JSON.stringify(value)))`,
//! ключ AES = sha256 от строки-ключа. Сам `.key` — такая же матрёшка на
//! зашитом в приложение ключе, внутри `{"encryptionKey": "<64 hex>"}`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::SslConfig;

use super::{ImportProfile, ImportSsh, Imported};

/// Ключ, которым Beekeeper шифрует сам файл `.key` (зашит в приложение).
const KEYFILE_KEY: &str = "38782F413F442A472D4B6150645367566B59703373367639792442264529482B";

/// Цвета метки, совпадающие с палитрой sql-kai; `default` и незнакомые — без цвета.
const ACCENTS: [&str; 7] = ["red", "orange", "yellow", "green", "blue", "purple", "pink"];

fn decrypt(cipher_text: &str, key: &str) -> Option<serde_json::Value> {
    // hmac в начале не проверяем: ключ тот же, а битый шифротекст всё равно
    // не разберётся в JSON ниже.
    if !cipher_text.is_ascii() || cipher_text.len() <= 96 {
        return None;
    }
    let body = &cipher_text[64..];
    let iv = hex::decode(&body[..32]).ok()?;
    let ct = B64.decode(&body[32..]).ok()?;
    let aes_key = Sha256::digest(key.as_bytes());
    let plain = cbc::Decryptor::<aes::Aes256>::new(aes_key.as_slice().into(), iv.as_slice().into())
        .decrypt_padded_vec_mut::<Pkcs7>(&ct)
        .ok()?;
    serde_json::from_slice(&plain).ok()
}

fn decrypt_string(cipher_text: Option<String>, key: &str) -> Option<String> {
    let raw = cipher_text.filter(|s| !s.is_empty())?;
    decrypt(&raw, key)?.as_str().map(str::to_string)
}

fn load_encryption_key(dir: &Path) -> Option<String> {
    let blob = std::fs::read_to_string(dir.join(".key")).ok()?;
    let key = decrypt(blob.trim(), KEYFILE_KEY)?;
    key.get("encryptionKey")?.as_str().map(str::to_string)
}

/// app.db и каталог с `.key` рядом: по --file (файл или каталог) либо в
/// userData электрона (macOS — ~/Library/Application Support/beekeeper-studio).
fn locate(explicit: Option<&Path>) -> Result<(PathBuf, PathBuf), AppError> {
    let (db, dir) = match explicit {
        Some(p) if p.is_file() => {
            let dir = p
                .parent()
                .ok_or_else(|| AppError::Msg("не определить каталог Beekeeper по --file".into()))?;
            (p.to_path_buf(), dir.to_path_buf())
        }
        Some(p) => (p.join("app.db"), p.to_path_buf()),
        None => {
            let dir = dirs::config_dir()
                .ok_or_else(|| AppError::Msg("не найден каталог настроек пользователя".into()))?
                .join("beekeeper-studio");
            (dir.join("app.db"), dir)
        }
    };
    if !db.exists() {
        return Err(AppError::Msg(format!(
            "не найдена база Beekeeper: {} (укажи путь через --file)",
            db.display()
        )));
    }
    Ok((db, dir))
}

fn col<T: rusqlite::types::FromSql>(row: &rusqlite::Row, name: &str) -> Option<T> {
    // Схема app.db росла миграциями — у старой версии колонки может не быть.
    row.get::<_, Option<T>>(name).ok().flatten()
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Полный путь папки подключения ("родитель/дочерняя") — в группу профиля.
fn folder_path(folders: &HashMap<i64, (String, Option<i64>)>, mut id: i64) -> Option<String> {
    let mut parts = Vec::new();
    // Циклическая ссылка parentId в чужом файле не должна вешать импорт.
    for _ in 0..16 {
        let (name, parent) = folders.get(&id)?;
        parts.push(name.clone());
        match parent {
            Some(p) => id = *p,
            None => break,
        }
    }
    parts.reverse();
    Some(parts.join("/"))
}

/// Строка saved_connection — только поля, которым есть куда лечь в профиль.
struct Saved {
    name: String,
    kind: String,
    host: Option<String>,
    port: Option<i64>,
    database: Option<String>,
    user: Option<String>,
    password: Option<String>,
    url: Option<String>,
    socket: bool,
    ssh_enabled: bool,
    ssh_host: Option<String>,
    ssh_port: Option<i64>,
    ssh_user: Option<String>,
    ssh_mode: Option<String>,
    ssh_keyfile: Option<String>,
    ssh_keyfile_password: Option<String>,
    ssh_keepalive: Option<i64>,
    bastion: Option<String>,
    color: Option<String>,
    read_only: bool,
    ssl: bool,
    ssl_ca: Option<String>,
    ssl_cert: Option<String>,
    ssl_key: Option<String>,
    ssl_verify: bool,
    folder: Option<i64>,
}

fn read_saved(conn: &Connection) -> Result<Vec<Saved>, AppError> {
    let fail = |e: rusqlite::Error| AppError::Msg(format!("не прочитать saved_connection: {e}"));
    let mut stmt = conn
        .prepare("select * from saved_connection order by name collate nocase")
        .map_err(fail)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Saved {
                name: col(r, "name").unwrap_or_default(),
                kind: col::<String>(r, "connectionType").unwrap_or_default(),
                host: col(r, "host"),
                port: col(r, "port"),
                database: col(r, "defaultDatabase"),
                user: col(r, "username"),
                password: col(r, "password"),
                url: col(r, "url"),
                socket: col(r, "socketPathEnabled").unwrap_or(false),
                ssh_enabled: col(r, "sshEnabled").unwrap_or(false),
                ssh_host: col(r, "sshHost"),
                ssh_port: col(r, "sshPort"),
                ssh_user: col(r, "sshUsername"),
                ssh_mode: col(r, "sshMode"),
                ssh_keyfile: col(r, "sshKeyfile"),
                ssh_keyfile_password: col(r, "sshKeyfilePassword"),
                ssh_keepalive: col(r, "sshKeepaliveInterval"),
                bastion: col(r, "sshBastionHost"),
                color: col(r, "labelColor"),
                read_only: col(r, "readOnlyMode").unwrap_or(false),
                ssl: col(r, "ssl").unwrap_or(false),
                ssl_ca: col(r, "sslCaFile"),
                ssl_cert: col(r, "sslCertFile"),
                ssl_key: col(r, "sslKeyFile"),
                ssl_verify: col(r, "sslRejectUnauthorized").unwrap_or(true),
                folder: col(r, "connectionFolderId"),
            })
        })
        .map_err(fail)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(fail)?;
    Ok(rows)
}

/// id -> (имя, родитель). Папок может не быть вовсе (старая схема) — тогда
/// просто нет групп, это не повод валить импорт.
fn read_folders(conn: &Connection) -> HashMap<i64, (String, Option<i64>)> {
    let Ok(mut stmt) = conn.prepare("select id, name, parentId from connection_folder") else {
        return HashMap::new();
    };
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            (r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?),
        ))
    });
    match rows {
        Ok(it) => it.flatten().collect(),
        Err(_) => HashMap::new(),
    }
}

pub fn read(explicit: Option<&Path>, with_passwords: bool) -> Result<Imported, AppError> {
    let (db, dir) = locate(explicit)?;
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| AppError::Msg(format!("не открыть {}: {e}", db.display())))?;
    let saved = read_saved(&conn)?;
    let folders = read_folders(&conn);
    let key = with_passwords.then(|| load_encryption_key(&dir)).flatten();

    let mut out = Imported::default();
    let mut foreign: HashMap<String, u32> = HashMap::new();
    if with_passwords && key.is_none() {
        out.notes.push(format!(
            "не прочитан ключ шифрования {} — профили перенесутся без паролей",
            dir.join(".key").display()
        ));
    }

    for s in saved {
        if !s.kind.eq_ignore_ascii_case("postgresql") {
            *foreign.entry(s.kind.clone()).or_default() += 1;
            continue;
        }
        let name = nonempty(Some(s.name)).unwrap_or_else(|| "без имени".into());
        if s.socket {
            out.notes.push(format!(
                "{name}: подключение через unix-сокет — sql-kai умеет только tcp, пропущено"
            ));
            continue;
        }
        let Some(host) = nonempty(s.host) else {
            let hint = if s.url.is_some() { " (задан URL)" } else { "" };
            out.notes.push(format!("{name}: не указан хост{hint}, пропущено"));
            continue;
        };

        let ssh = match s.ssh_enabled.then(|| nonempty(s.ssh_host)).flatten() {
            Some(ssh_host) => {
                let mode = s.ssh_mode.unwrap_or_default();
                if mode == "userpass" {
                    out.notes.push(format!(
                        "{name}: ssh по паролю не поддерживается — пропиши ключ в профиле"
                    ));
                }
                if nonempty(s.bastion).is_some() {
                    out.notes.push(format!(
                        "{name}: ssh-бастион не переносится, опиши цепочку в ~/.ssh/config"
                    ));
                }
                Some(ImportSsh {
                    host: ssh_host,
                    user: nonempty(s.ssh_user),
                    port: s.ssh_port.and_then(|p| u16::try_from(p).ok()),
                    key_path: (mode == "keyfile").then(|| nonempty(s.ssh_keyfile)).flatten(),
                    keepalive_interval: s.ssh_keepalive.and_then(|k| u32::try_from(k).ok()),
                })
            }
            None => None,
        };

        let (mut password, mut ssh_passphrase) = (None, None);
        if let Some(key) = &key {
            let stored = s.password.filter(|v| !v.is_empty());
            let decrypted = decrypt_string(stored.clone(), key);
            if stored.is_some() && decrypted.is_none() {
                out.notes
                    .push(format!("{name}: пароль не расшифровался, введи его вручную"));
            }
            password = decrypted;
            ssh_passphrase = decrypt_string(s.ssh_keyfile_password, key);
        }

        let ssl = s.ssl.then(|| SslConfig {
            enabled: true,
            ca_cert: nonempty(s.ssl_ca),
            client_cert: nonempty(s.ssl_cert),
            client_key: nonempty(s.ssl_key),
            reject_unauthorized: s.ssl_verify,
        });

        out.profiles.push(ImportProfile {
            name,
            host,
            port: s.port.and_then(|p| u16::try_from(p).ok()).unwrap_or(5432),
            database: nonempty(s.database).unwrap_or_else(|| "postgres".into()),
            user: nonempty(s.user).unwrap_or_default(),
            password,
            ssh,
            ssh_passphrase,
            group: s.folder.and_then(|id| folder_path(&folders, id)),
            color: nonempty(s.color).filter(|c| ACCENTS.contains(&c.as_str())),
            production: s.read_only,
            ssl,
        });
    }

    if !foreign.is_empty() {
        let mut kinds: Vec<String> = foreign.iter().map(|(k, n)| format!("{k}: {n}")).collect();
        kinds.sort();
        out.notes.push(format!(
            "пропущены подключения не к postgres ({})",
            kinds.join(", ")
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{decrypt, decrypt_string, folder_path, KEYFILE_KEY};
    use std::collections::HashMap;

    // Шифротекст сделан самим simple-encryptor'ом (node), а не этим кодом —
    // иначе тест проверял бы согласованность с собой, а не с Beekeeper.
    const SECRET: &str = "1b1f82d630463b69d3180a6de813f247ea2809ec60856d1692994ca37a1a222e07070707070707070707070707070707+3LlF2ZGSseEQXe/EpmHdQ==";
    const KEYFILE: &str = "957928bd71feb9cfd85eaf56a8f31f315c9c0ada533cabda0f105b8c6f22032e07070707070707070707070707070707xmu269Fpmus+CdgCsIaAiFPlU+qBEc2fudHaPdfjpT0fI4v09AXD0vz4HHv6PJl8zBtzjmqVKuIOOMnzzfEkRzCTs/9OjZUBtD6PfYAK9OOkwJpkBP7IB507Fzar7Kjn";

    #[test]
    fn decrypts_simple_encryptor_payload() {
        assert_eq!(
            decrypt_string(Some(SECRET.into()), "test-key"),
            Some("hunter2".into())
        );
        let key = decrypt(KEYFILE, KEYFILE_KEY).unwrap();
        assert_eq!(key["encryptionKey"], "a".repeat(64));
    }

    #[test]
    fn rejects_wrong_key_and_garbage() {
        assert_eq!(decrypt_string(Some(SECRET.into()), "other-key"), None);
        assert_eq!(decrypt_string(Some("короткая строка".into()), "test-key"), None);
        assert_eq!(decrypt_string(Some(String::new()), "test-key"), None);
    }

    #[test]
    fn folder_path_joins_parents_and_survives_a_cycle() {
        let folders = HashMap::from([
            (1, ("prod".to_string(), None)),
            (2, ("eu".to_string(), Some(1))),
            (3, ("loop".to_string(), Some(3))),
        ]);
        assert_eq!(folder_path(&folders, 2), Some("prod/eu".into()));
        assert_eq!(folder_path(&folders, 1), Some("prod".into()));
        assert_eq!(folder_path(&folders, 9), None);
        assert!(folder_path(&folders, 3).is_some());
    }
}
