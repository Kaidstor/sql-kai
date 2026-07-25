//! Жёсткий барьер на запись в профили с меткой `production`.
//!
//! В GUI прод защищён диалогом («точно выполняем этот UPDATE?»), а у CLI и MCP
//! такой защиты не было: одного `--write` (или `"write": true` в MCP-tool)
//! хватало, чтобы агент изменил боевую базу. Барьер живёт в слое сессии, а не
//! в отдельных командах, — так его наследуют сразу оба пути записи: автономная
//! сессия ([`super::open_for`]) и запрос через сервер сессий
//! ([`crate::broker_client::BrokerClient::query`], которым ходит MCP-tool
//! `query`).
//!
//! Разрешение всегда исходит от человека и выдаётся тремя способами:
//! флаг `--prod-write` (виден в команде, которую пользователь подтверждает
//! агенту), ввод имени профиля в терминале и env [`ALLOW_PROD_WRITE`] —
//! единственный вариант для неинтерактивных потребителей. Для MCP это
//! принципиально: окружение серверу задаёт клиент в своём конфиге, модель в
//! него не пишет, а stdin занят протоколом — значит по умолчанию запись в
//! прод оттуда просто не проходит.
//!
//! Спрашивает человека всегда клиент — у сервера сессий нет tty. Поэтому этот
//! барьер парный: пройдя его, [`crate::broker_client`] помечает запрос
//! `prodWriteAuthorized`, а сервер без такой пометки (или без своего
//! `SQL_KAI_ALLOW_PROD_WRITE`) в prod-профиль не пишет — см.
//! `broker::server::guard_prod_write`. Серверный чек ловит запись мимо этого
//! файла: старый sql-kai, который про барьер не знает, и любого другого клиента
//! сокета. Границей против враждебного процесса того же пользователя он не
//! является и не может быть — пометку такой процесс поставит сам, как сам
//! прочитает и профили; барьер защищает от нечаянной записи, а не от подмены.

use std::sync::Mutex;

use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

use crate::envvar::{self, ALLOW_PROD_DUMP, ALLOW_PROD_WRITE};
use crate::input;

/// Профили (id), которым разрешение уже выдано в этом процессе. Путь записи
/// проходит барьер дважды (команда → сессия/брокер), а спросить человека
/// нужно один раз — второй чек-пойнт видит выданное разрешение.
static GRANTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn is_granted(id: &str) -> bool {
    GRANTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|g| g == id)
}

fn grant(id: &str) {
    let mut granted = GRANTED.lock().unwrap_or_else(|e| e.into_inner());
    if !granted.iter().any(|g| g == id) {
        granted.push(id.to_string());
    }
}

/// Значение `SQL_KAI_ALLOW_PROD_WRITE`: `1` (любой prod-профиль) или список
/// имён/id профилей через запятую — разрешение точечное, чтобы «открыть» один
/// сервис агенту, не открывая остальные.
fn allowlist_matches(raw: &str, name: &str, id: &str) -> bool {
    raw.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .any(|item| {
            matches!(
                item.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on" | "all"
            ) || item.eq_ignore_ascii_case(name)
                || item == id
        })
}

fn env_allows(profile: &Profile) -> bool {
    envvar::value(ALLOW_PROD_WRITE)
        .is_some_and(|raw| allowlist_matches(&raw, &profile.name, &profile.id))
}

/// Куда именно смотреть, когда запись отвергнута: без этого текста отказ
/// выглядит как поломка, а не как политика.
fn denied(profile: &Profile, why: &str) -> AppError {
    AppError::Msg(format!(
        "запись в production-профиль '{}' ({}@{}:{}/{}) заблокирована: {}\n\
         разрешить запись:\n  \
         • в терминале — повтори команду и подтверди, введя имя профиля;\n  \
         • флагом — добавь в команду --prod-write;\n  \
         • неинтерактивно (MCP, CI, пайп) — {ALLOW_PROD_WRITE}={} в окружении процесса \
         (или {ALLOW_PROD_WRITE}=1 для всех prod-профилей); для MCP это env сервера \
         sql-kai в конфиге клиента.\n  \
         Метка production снимается в карточке профиля в GUI.",
        profile.name, profile.user, profile.host, profile.port, profile.database, why, profile.name,
    ))
}

/// Барьер перед пишущей сессией. Для не-prod профиля — мгновенный `Ok`,
/// поэтому вызывать можно безусловно на любом пути записи.
pub fn guard_prod_write(profile: &Profile) -> Result<(), AppError> {
    if !profile.production || is_granted(&profile.id) {
        return Ok(());
    }
    if env_allows(profile) {
        grant(&profile.id);
        return Ok(());
    }
    // Спрашивать некого: пайп/CI (нет TTY) или MCP, где stdin занят JSON-RPC —
    // и это не выводится из TTY, потому что сервер бывает запущен руками в
    // терминале. Строгий отказ без вопросов.
    if !input::is_interactive() {
        return Err(denied(
            profile,
            "подтвердить некому (stdin занят протоколом или не терминал), \
             а одного --write для прода мало",
        ));
    }
    eprintln!(
        "⚠ ЗАПИСЬ В PRODUCTION: профиль '{}' ({}@{}:{}/{})",
        profile.name, profile.user, profile.host, profile.port, profile.database
    );
    eprint!(
        "введи имя профиля ('{}') для подтверждения, Enter — отмена: ",
        profile.name
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case(&profile.name) {
        return Err(denied(profile, "подтверждение не совпало с именем профиля"));
    }
    grant(&profile.id);
    Ok(())
}

/// Барьер + разрешение, заявленное аргументами команды (`--prod-write`).
/// Команды зовут её до подключения: вопрос задаётся один раз и до того, как
/// поднимутся туннель и сессия.
pub fn authorize_prod_write(profile: &Profile, explicit: bool) -> Result<(), AppError> {
    if explicit && profile.production {
        grant(&profile.id);
    }
    guard_prod_write(profile)
}

/// Барьер на выгрузку данных production-профиля (`fork --data`). Отдельный от
/// записи, потому что риск другой: запись меняет боевую базу, а выгрузка кладёт
/// её полную копию на эту машину — дамп во временном файле, том контейнера,
/// потом профиль без метки `production`. Асимметрия «писать нельзя, а вынести
/// всё можно по --yes» и была тем, что здесь закрывается: `--yes`, которым
/// снимают обычный вопрос про форк, это подтверждение не снимает.
pub fn authorize_prod_dump(profile: &Profile, explicit: bool) -> Result<(), AppError> {
    if !profile.production || explicit {
        return Ok(());
    }
    if envvar::value(ALLOW_PROD_DUMP)
        .is_some_and(|raw| allowlist_matches(&raw, &profile.name, &profile.id))
    {
        return Ok(());
    }
    let deny = |why: &str| {
        AppError::Msg(format!(
            "выгрузка данных production-профиля '{}' ({}@{}:{}/{}) заблокирована: {}\n\
             разрешить:\n  \
             • в терминале — повтори команду и подтверди, введя имя профиля;\n  \
             • флагом — добавь в команду --prod-data;\n  \
             • неинтерактивно — {ALLOW_PROD_DUMP}={} в окружении процесса.\n  \
             Схему можно снять и без этого: то же самое без --data.",
            profile.name, profile.user, profile.host, profile.port, profile.database, why,
            profile.name,
        ))
    };
    if !input::is_interactive() {
        return Err(deny("подтвердить некому (stdin занят протоколом или не терминал)"));
    }
    eprintln!(
        "⚠ ВЫГРУЗКА БОЕВЫХ ДАННЫХ: профиль '{}' ({}@{}:{}/{})",
        profile.name, profile.user, profile.host, profile.port, profile.database
    );
    eprintln!(
        "  полная копия данных окажется на этой машине: дамп во временном файле \
         и том docker-контейнера (тома до `docker rm -f` не удаляются)."
    );
    eprint!(
        "введи имя профиля ('{}') для подтверждения, Enter — отмена: ",
        profile.name
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if !line.trim().eq_ignore_ascii_case(&profile.name) {
        return Err(deny("подтверждение не совпало с именем профиля"));
    }
    Ok(())
}

/// Тот же барьер, когда на руках только id профиля: через сервер сессий
/// (GUI-брокер/holder) запрос уходит с `profileId`, и это единственная точка,
/// общая для `sql-kai q` и MCP.
pub fn guard_prod_write_by_id(profile_id: &str) -> Result<(), AppError> {
    // Список профилей нечитаем — отказ: чего не видим, то не считаем не-прод.
    let all = store::load_profiles()?;
    match all.iter().find(|p| p.id == profile_id) {
        Some(profile) => guard_prod_write(profile),
        // Профиля нет — метке production взяться неоткуда, а сам запрос
        // сервер сессий отвергнет своей ошибкой.
        None => Ok(()),
    }
}

/// Куда ssh-алиас ведёт на самом деле: (hostname, port, user) по `ssh -G`.
/// `~/.ssh/config` разрешает называть один хост сколькими угодно именами —
/// второй `Host`-блок, FQDN, голый IP, — поэтому сравнения алиасов строкой мало:
/// под другим именем того же прода запись проходила мимо барьера. `ssh -G`
/// только печатает итоговую конфигурацию и никуда не подключается.
fn ssh_endpoint(alias: &str) -> Option<(String, String, String)> {
    let out = std::process::Command::new("ssh")
        .args(["-G", "--", alias])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut host, mut port, mut user) = (None, None, None);
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(' ') {
            let value = value.trim().to_ascii_lowercase();
            match key {
                "hostname" => host = Some(value),
                "port" => port = Some(value),
                "user" => user = Some(value),
                _ => {}
            }
        }
    }
    Some((host?, port?, user?))
}

/// Барьер для `sql-kai exec <ssh-alias>`: профиля в аргументах нет, но если на
/// тот же ssh-хост смотрит production-профиль, то `docker exec psql` — такая
/// же запись в прод, как обычная сессия, и мимо барьера идти не должна.
///
/// Сначала дешёвое совпадение по имени, затем — резолв через [`ssh_endpoint`]
/// для алиасов-синонимов. Если `ssh -G` недоступен, остаётся только первый
/// уровень: барьер не строже прежнего, но и не слабее.
pub fn authorize_prod_write_ssh(alias: &str, explicit: bool) -> Result<(), AppError> {
    let all = store::load_profiles()?;
    let named = all.iter().find(|p| {
        p.production
            && (p
                .ssh
                .as_ref()
                .is_some_and(|s| s.host.eq_ignore_ascii_case(alias))
                || p.name.eq_ignore_ascii_case(alias)
                || p.id == alias)
    });
    if let Some(profile) = named {
        return authorize_prod_write(profile, explicit);
    }
    let target = ssh_endpoint(alias);
    let resolved = target.and_then(|target| {
        all.iter().find(|p| {
            p.production
                && p.ssh
                    .as_ref()
                    .and_then(|s| ssh_endpoint(&s.host))
                    .is_some_and(|endpoint| endpoint == target)
        })
    });
    match resolved {
        Some(profile) => authorize_prod_write(profile, explicit),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::allowlist_matches;

    #[test]
    fn wildcard_values_allow_any_profile() {
        for raw in ["1", "true", "YES", "on", "all"] {
            assert!(allowlist_matches(raw, "domainator", "id-1"));
        }
    }

    #[test]
    fn allowlist_is_per_profile() {
        assert!(allowlist_matches("vuln, domainator", "domainator", "id-1"));
        assert!(allowlist_matches("DOMAINATOR", "domainator", "id-1"));
        assert!(allowlist_matches("id-1", "domainator", "id-1"));
        assert!(!allowlist_matches("vuln", "domainator", "id-1"));
        assert!(!allowlist_matches("", "domainator", "id-1"));
        // id — точное совпадение: регистр в uuid значим
        assert!(!allowlist_matches("ID-1", "domainator", "id-1"));
    }
}
