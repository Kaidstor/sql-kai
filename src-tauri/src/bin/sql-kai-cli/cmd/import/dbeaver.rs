//! Чтение подключений DBeaver: `<project>/.dbeaver/data-sources.json` —
//! открытый JSON, и `credentials-config.json` рядом — те же данные, но
//! зашифрованные AES-128-CBC на ключе, зашитом в саму программу (ключ один
//! для всех установок, это не защита от чтения, а обфускация).
//!
//! Формат шифротекста версии различают: у одних первый блок — случайный
//! мусор при нулевом IV, у других — сам IV. Пробуем оба и берём тот, что
//! разобрался в JSON.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use serde_json::Value;
use sql_kai_lib::error::AppError;

use super::{ImportProfile, ImportSsh, Imported};

/// Ключ credentials-config.json, одинаковый во всех сборках DBeaver.
const CRED_KEY: [u8; 16] = [
    0xba, 0xbb, 0x4a, 0x9f, 0x77, 0x4a, 0xb8, 0x53, 0xc9, 0x6c, 0x2d, 0x65, 0x3d, 0xfe, 0x54, 0x4a,
];

fn decrypt_cbc(ct: &[u8], iv: &[u8]) -> Option<Vec<u8>> {
    if ct.is_empty() || !ct.len().is_multiple_of(16) || iv.len() != 16 {
        return None;
    }
    let mut buf = ct.to_vec();
    cbc::Decryptor::<aes::Aes128>::new(CRED_KEY.as_slice().into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .ok()?;
    Some(buf)
}

/// JSON внутри дополнен мусором с обеих сторон: спереди — блок соли, сзади —
/// добивка до размера блока. Режем по фигурным скобкам, а не по длине.
fn carve_json(plain: &[u8]) -> Option<Value> {
    let start = plain.iter().position(|b| *b == b'{')?;
    let end = plain.iter().rposition(|b| *b == b'}')?;
    if end < start {
        return None;
    }
    serde_json::from_slice(&plain[start..=end]).ok()
}

fn decode_credentials(raw: &[u8]) -> Option<Value> {
    let salted = decrypt_cbc(raw, &[0u8; 16]).and_then(|p| carve_json(&p));
    if salted.is_some() {
        return salted;
    }
    if raw.len() <= 16 {
        return None;
    }
    let (iv, body) = raw.split_at(16);
    decrypt_cbc(body, iv).and_then(|p| carve_json(&p))
}

fn read_credentials(path: &Path) -> Option<Value> {
    decode_credentials(&std::fs::read(path).ok()?)
}

/// Строковое поле, которое DBeaver может хранить и числом ("port": 5432).
fn s(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// jdbc:postgresql://host:port/db — запасной источник адреса, когда поля
/// configuration пустые (подключение заводили строкой).
fn parse_jdbc(url: &str) -> Option<(String, Option<u16>, Option<String>)> {
    let rest = url.strip_prefix("jdbc:postgresql://")?;
    let (authority, db) = match rest.split_once('/') {
        Some((a, d)) => (a, Some(d.split(['?', ';']).next().unwrap_or(d).to_string())),
        None => (rest, None),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()),
        None => (authority.to_string(), None),
    };
    Some((host, port, db.filter(|d| !d.is_empty())))
}

/// Файлы data-sources.json: по --file (сам файл или каталог, внутри которого
/// искать) либо в DBeaverData — рабочих пространств и проектов бывает много.
fn find_sources(explicit: Option<&Path>) -> Result<Vec<PathBuf>, AppError> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Ok(vec![p.to_path_buf()]);
        }
        let mut found = Vec::new();
        scan(p, 3, &mut found);
        if found.is_empty() {
            return Err(AppError::Msg(format!(
                "в {} не найден .dbeaver/data-sources.json",
                p.display()
            )));
        }
        return Ok(found);
    }

    let home = dirs::home_dir();
    let roots = [
        home.as_ref().map(|h| h.join("Library/DBeaverData")),
        dirs::data_dir().map(|d| d.join("DBeaverData")),
        home.as_ref().map(|h| h.join(".local/share/DBeaverData")),
    ];
    let mut found = Vec::new();
    for root in roots.into_iter().flatten() {
        if root.is_dir() {
            scan(&root, 3, &mut found);
        }
    }
    found.sort();
    found.dedup();
    if found.is_empty() {
        return Err(AppError::Msg(
            "не найдены данные DBeaver (укажи каталог workspace или сам data-sources.json через --file)"
                .into(),
        ));
    }
    Ok(found)
}

fn scan(dir: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    let ds = dir.join(".dbeaver").join("data-sources.json");
    if ds.is_file() {
        out.push(ds);
    }
    if depth == 0 {
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() && entry.file_name() != ".dbeaver" {
            scan(&path, depth - 1, out);
        }
    }
}

fn ssh_handler(config: &Value) -> Option<&Value> {
    let handler = config.get("handlers")?.get("ssh_tunnel")?;
    flag(handler, "enabled").then_some(handler)
}

pub fn read(explicit: Option<&Path>, with_passwords: bool) -> Result<Imported, AppError> {
    let mut out = Imported::default();
    let mut foreign: HashMap<String, u32> = HashMap::new();

    for sources in find_sources(explicit)? {
        let raw = std::fs::read_to_string(&sources)?;
        let root: Value = serde_json::from_str(&raw)
            .map_err(|e| AppError::Msg(format!("не разобрать {}: {e}", sources.display())))?;
        // Читаем всегда, даже с --no-passwords: в этом же файле лежит имя
        // пользователя, а оно не секрет и нужно профилю.
        let creds_path = sources.with_file_name("credentials-config.json");
        let creds = creds_path.is_file().then(|| read_credentials(&creds_path)).flatten();
        if creds.is_none() && creds_path.is_file() {
            out.notes.push(format!(
                "не расшифрован {} — пользователя и пароль задай в профиле сам",
                creds_path.display()
            ));
        }

        let Some(connections) = root.get("connections").and_then(Value::as_object) else {
            continue;
        };
        for (id, conn) in connections {
            let provider = s(conn, "provider").unwrap_or_default();
            if provider != "postgresql" {
                *foreign.entry(provider).or_default() += 1;
                continue;
            }
            let name = s(conn, "name").unwrap_or_else(|| id.clone());
            let empty = Value::Null;
            let config = conn.get("configuration").unwrap_or(&empty);
            let cred = creds.as_ref().and_then(|c| c.get(id));
            let db_cred = cred.and_then(|c| c.get("#connection"));

            let from_url = s(config, "url").as_deref().and_then(parse_jdbc);
            let host = s(config, "host")
                .or_else(|| from_url.as_ref().map(|u| u.0.clone()))
                .unwrap_or_default();
            if host.is_empty() {
                out.notes.push(format!("{name}: не указан хост, пропущено"));
                continue;
            }

            let ssh = ssh_handler(config).map(|handler| {
                let props = handler.get("properties").unwrap_or(&empty);
                let auth = s(props, "authType").unwrap_or_default();
                if auth.eq_ignore_ascii_case("password") {
                    out.notes.push(format!(
                        "{name}: ssh по паролю не поддерживается — пропиши ключ в профиле"
                    ));
                }
                ImportSsh {
                    host: s(props, "host").unwrap_or_default(),
                    user: s(handler, "user").or_else(|| s(props, "user")),
                    port: s(props, "port").and_then(|p| p.parse().ok()),
                    key_path: auth
                        .eq_ignore_ascii_case("public_key")
                        .then(|| s(props, "keyPath"))
                        .flatten(),
                    keepalive_interval: s(props, "aliveInterval").and_then(|v| v.parse().ok()),
                }
            });
            let ssh = ssh.filter(|s| !s.host.is_empty());
            let ssh_passphrase = ssh
                .as_ref()
                .filter(|s| with_passwords && s.key_path.is_some())
                .and_then(|_| cred?.get("network/ssh_tunnel")?.get("password")?.as_str())
                .map(str::to_string);

            if config.get("handlers").and_then(|h| h.get("ssl")).is_some_and(|h| flag(h, "enabled")) {
                out.notes
                    .push(format!("{name}: настройки SSL не переносятся, задай их в профиле"));
            }

            out.profiles.push(ImportProfile {
                name,
                host,
                port: s(config, "port")
                    .and_then(|p| p.parse().ok())
                    .or_else(|| from_url.as_ref().and_then(|u| u.1))
                    .unwrap_or(5432),
                database: s(config, "database")
                    .or_else(|| from_url.as_ref().and_then(|u| u.2.clone()))
                    .unwrap_or_else(|| "postgres".into()),
                user: db_cred
                    .and_then(|c| s(c, "user"))
                    .or_else(|| s(config, "user"))
                    .unwrap_or_default(),
                password: with_passwords.then(|| db_cred.and_then(|c| s(c, "password"))).flatten(),
                ssh,
                ssh_passphrase,
                group: s(conn, "folder"),
                // Цвет в DBeaver задаётся типом подключения (произвольный hex),
                // в палитру sql-kai не ложится — переносим только "прод".
                color: None,
                production: flag(conn, "read-only")
                    || s(config, "type").is_some_and(|t| t.eq_ignore_ascii_case("prod")),
                ssl: None,
            });
        }
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
    use super::{decode_credentials, parse_jdbc};

    // Оба варианта раскладки, зашифрованные сторонним кодом (node) по
    // алгоритму DBeaver: без IV в файле и с IV первым блоком.
    const ZERO_IV: &str = "10a873c7627e7569ab712aaaf824c359e54a74bcaf624b3eb952a3cdcbfce5763ada034db2adc44bb7502f7158f5ad6004e8b7aebafb7b1e59fc892cf27efa461602903e66dc7e14475049c9b79093b8";
    const IV_PREFIX: &str = "09090909090909090909090909090909501f623284e8205466d9ede71903e6aeb3e4ec434a3f1ad5ee6fb74f1eb96ad389d692cd720cae04c4605b749a0186fe8a088ce6bb81e35e8eb27418852ca831138c0f14eef0e29e8a3e588b5cee9632";

    #[test]
    fn decodes_both_credential_layouts() {
        for blob in [ZERO_IV, IV_PREFIX] {
            let v = decode_credentials(&hex::decode(blob).unwrap()).unwrap();
            assert_eq!(v["pg-1"]["#connection"]["user"], "app");
            assert_eq!(v["pg-1"]["#connection"]["password"], "p@ss");
        }
    }

    #[test]
    fn rejects_credentials_that_are_not_ours() {
        assert!(decode_credentials(b"").is_none());
        assert!(decode_credentials(&[0u8; 64]).is_none());
        // не кратно блоку — ни один из вариантов даже не запустится
        assert!(decode_credentials(&[0u8; 33]).is_none());
    }

    #[test]
    fn parses_jdbc_url() {
        assert_eq!(
            parse_jdbc("jdbc:postgresql://db.local:6432/shop?ssl=true"),
            Some(("db.local".into(), Some(6432), Some("shop".into())))
        );
        assert_eq!(
            parse_jdbc("jdbc:postgresql://db.local/shop"),
            Some(("db.local".into(), None, Some("shop".into())))
        );
        assert_eq!(
            parse_jdbc("jdbc:postgresql://db.local:5432/"),
            Some(("db.local".into(), Some(5432), None))
        );
        assert_eq!(parse_jdbc("jdbc:mysql://db.local:3306/shop"), None);
    }
}
