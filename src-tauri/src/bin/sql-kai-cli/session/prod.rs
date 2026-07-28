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

use std::collections::BTreeSet;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::Mutex;

use sql_kai_lib::error::AppError;
use sql_kai_lib::prod as prodenv;
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

fn env_allows(profile: &Profile) -> bool {
    envvar::value(ALLOW_PROD_WRITE)
        .is_some_and(|raw| prodenv::allowlist_matches(&raw, &profile.name, &profile.id))
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
        .is_some_and(|raw| prodenv::allowlist_matches(&raw, &profile.name, &profile.id))
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

/// Куда ssh-алиас ведёт на самом деле — итоговый `hostname` из `ssh -G`.
/// `~/.ssh/config` разрешает называть один хост сколькими угодно именами —
/// второй `Host`-блок, FQDN, голый IP, — поэтому сравнения алиасов строкой мало:
/// под другим именем того же прода запись проходила мимо барьера. `ssh -G`
/// только печатает итоговую конфигурацию и никуда не подключается.
///
/// Ни порт, ни пользователь в ключ не входят: `deploy@prod` и `root@prod` — одна
/// и та же машина с одними и теми же контейнерами, а лишнее поле в ключе стоит
/// не лишнего вопроса, а несработавшего барьера.
fn ssh_hostname(alias: &str) -> Option<String> {
    let out = std::process::Command::new("ssh")
        .args(["-G", "--", alias])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("hostname "))
        .map(|v| v.trim().to_ascii_lowercase())
}

/// Адреса, в которые разрешается хост. Порт для резолва любой — на выбор
/// адресов он не влияет.
fn host_addrs(host: &str) -> Option<BTreeSet<IpAddr>> {
    let addrs: BTreeSet<IpAddr> = (host, 22u16)
        .to_socket_addrs()
        .ok()?
        .map(|a| a.ip())
        .collect();
    (!addrs.is_empty()).then_some(addrs)
}

/// Один ли это ssh-хост. Сначала имена, потом адреса: тот же прод, записанный
/// FQDN в `~/.ssh/config` и голым IP в командной строке, даёт разные строки, и
/// сравнение имён его пропускает. Резолв стоит похода в DNS, поэтому он второй
/// ступенью; когда резолва нет (нет сети, имя живёт только внутри VPN), барьер
/// остаётся ровно таким, каким был на сравнении имён.
fn same_ssh_host(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (host_addrs(a), host_addrs(b)) {
        (Some(x), Some(y)) => !x.is_disjoint(&y),
        _ => false,
    }
}

/// Барьер для `sql-kai exec <ssh-alias>`: профиля в аргументах нет, но если на
/// тот же ssh-хост смотрит production-профиль, то `docker exec psql` — такая
/// же запись в прод, как обычная сессия, и мимо барьера идти не должна.
///
/// Сначала дешёвое совпадение по имени, затем — резолв через [`ssh_hostname`]
/// и [`same_ssh_host`] для алиасов-синонимов. Если `ssh -G` недоступен, остаётся
/// только первый уровень: барьер не строже прежнего, но и не слабее.
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
    let resolved = ssh_hostname(alias).and_then(|target| {
        all.iter().find(|p| {
            p.production
                && p.ssh
                    .as_ref()
                    .and_then(|s| ssh_hostname(&s.host))
                    .is_some_and(|host| same_ssh_host(&host, &target))
        })
    });
    match resolved {
        Some(profile) => authorize_prod_write(profile, explicit),
        None => Ok(()),
    }
}

