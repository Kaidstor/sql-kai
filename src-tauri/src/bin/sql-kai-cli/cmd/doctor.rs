//! `sql-kai doctor` — здоровье соединений: для каждого профиля проверяет, что
//! сохранённый пароль ещё аутентифицируется, и, если пароль дублируется в sec,
//! ловит дрейф (в vault одно, в sec другое, в БД работает третье).
//!
//! Здесь же — «чем поставлен сам CLI»: правильный совет по обновлению у каска,
//! бандла и сборки из исходников разный, а `brew upgrade` для не-brew установки
//! (и наоборот) только путает.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store::{self, Profile};

use crate::output::{self, Format, FormatArgs};
use crate::{sec, session};

#[derive(Args)]
pub struct DoctorArgs {
    /// Проверить один профиль (имя или id); без него — все
    alias: Option<String>,
    #[command(flatten)]
    fmt: FormatArgs,
    /// Только способ установки и путь обновления (без проверки паролей)
    #[arg(long)]
    install_info: bool,
}

/// Имя каска и бандла — одно и то же во всех вариантах установки.
const APP_NAME: &str = "sql-kai";
/// Куда каск и .dmg кладут приложение.
const APP_DIR: &str = "/Applications";

/// Чем в систему попал работающий бинарь. Выводится только из фактов
/// файловой системы (путь запуска, куда ведёт симлинк, каталог каска):
/// своих маркеров установки sql-kai никуда не пишет, так что гадать не по чему,
/// и каждый вариант ниже подкреплён путём, который видно в выводе.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallKind {
    /// Бинарь из того самого бандла .app, который поставил каск Homebrew.
    BrewCask,
    /// Бинарь из бандла .app, который каску не принадлежит: .dmg вручную
    /// (симлинк сделан руками или кнопкой «Install CLI» в GUI).
    Bundle,
    /// `~/.cargo/bin` — поставлен `cargo install`.
    Cargo,
    /// `target/debug|release` — сборка из исходников (в т.ч. .app из
    /// `target/release/bundle`).
    Dev,
    Unknown,
}

pub(crate) struct InstallInfo {
    pub kind: InstallKind,
    /// Путь, которым бинарь запущен (обычно симлинк из PATH).
    pub invoked: PathBuf,
    /// Куда этот путь ведёт на самом деле.
    pub real: PathBuf,
    /// Бандл .app, внутри которого лежит `real`.
    pub bundle: Option<PathBuf>,
    /// Каталог каска — тот самый признак, по которому выбран BrewCask.
    pub caskroom: Option<PathBuf>,
}

/// Ближайший предок-бандл: `…/sql-kai.app/Contents/MacOS/sql-kai-cli` → `….app`.
fn bundle_of(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .map(Path::to_path_buf)
}

/// `<префикс>/Caskroom/sql-kai` — метаданные установленного каска. Префикс
/// берём из окружения brew (нестандартная установка) и из двух штатных.
fn caskroom_dir() -> Option<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    if let Ok(p) = std::env::var("HOMEBREW_PREFIX") {
        if !p.is_empty() {
            prefixes.push(PathBuf::from(p));
        }
    }
    prefixes.push(PathBuf::from("/opt/homebrew"));
    prefixes.push(PathBuf::from("/usr/local"));
    prefixes
        .into_iter()
        .map(|p| p.join("Caskroom").join(APP_NAME))
        .find(|p| p.is_dir())
}

/// Путь внутри `…/target/{debug,release}/…` — рабочая сборка из дерева
/// исходников (в т.ч. с общим CARGO_TARGET_DIR). Смотрим всех предков, а не
/// только каталог бинаря: у `cargo tauri build` бинарь лежит глубже —
/// `target/release/bundle/macos/sql-kai.app/Contents/MacOS/sql-kai-cli`.
fn in_target_dir(path: &Path) -> bool {
    path.ancestors().any(|p| {
        let profile = p.file_name().and_then(|n| n.to_str());
        let parent = p
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str());
        matches!(profile, Some("debug") | Some("release")) && parent == Some("target")
    })
}

/// Куда каск кладёт .app: штатный /Applications и пользовательский
/// ~/Applications (`brew --appdir`). Других мест без явной настройки нет.
fn cask_app_dirs() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from(APP_DIR)];
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Applications"));
    }
    out
}

/// Этот ли бандл поставил каск. Одного факта «каск установлен» мало: рядом
/// может лежать другой .app — dev-сборка из `target/release/bundle/macos` или
/// копия, принесённая руками. Им `brew upgrade --cask` ничего не обновит, а
/// ложный диагноз уезжает ещё и в issue из `sql-kai feedback`.
/// Признак принадлежности — путь: либо каталог назначения каска, либо
/// staged-копия внутри самого Caskroom.
fn cask_owns_bundle(bundle: &Path, caskroom: &Path, app_dirs: &[PathBuf]) -> bool {
    bundle.starts_with(caskroom)
        || app_dirs
            .iter()
            .any(|d| bundle == d.join(format!("{APP_NAME}.app")))
}

/// `$CARGO_HOME/bin` (по умолчанию `~/.cargo/bin`) — результат `cargo install`.
fn in_cargo_bin(path: &Path) -> bool {
    let home = std::env::var("CARGO_HOME")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".cargo")));
    home.is_some_and(|h| path.starts_with(h.join("bin")))
}

pub(crate) fn detect_install() -> InstallInfo {
    let invoked = std::env::current_exe().unwrap_or_default();
    // current_exe на macOS отдаёт путь запуска — симлинк из PATH так и остаётся
    // симлинком; настоящее место бинаря даёт только canonicalize.
    let real = std::fs::canonicalize(&invoked).unwrap_or_else(|_| invoked.clone());
    let bundle = bundle_of(&real);
    // Каталог каска показываем только когда он и правда объясняет установку:
    // в выводе это строка «признак», по которой выбран BrewCask.
    let caskroom = match (&bundle, caskroom_dir()) {
        (Some(b), Some(c)) if cask_owns_bundle(b, &c, &cask_app_dirs()) => Some(c),
        _ => None,
    };
    // Бандл внутри target/ — всегда своя сборка, чей бы каск ни стоял рядом.
    let kind = if in_target_dir(&real) {
        InstallKind::Dev
    } else if caskroom.is_some() {
        InstallKind::BrewCask
    } else if bundle.is_some() {
        InstallKind::Bundle
    } else if in_cargo_bin(&real) {
        InstallKind::Cargo
    } else {
        InstallKind::Unknown
    };
    InstallInfo {
        kind,
        invoked,
        real,
        bundle,
        caskroom,
    }
}

impl InstallInfo {
    /// Стабильный идентификатор для машинных форматов.
    pub(crate) fn id(&self) -> &'static str {
        match self.kind {
            InstallKind::BrewCask => "brew-cask",
            InstallKind::Bundle => "bundle",
            InstallKind::Cargo => "cargo",
            InstallKind::Dev => "dev",
            InstallKind::Unknown => "unknown",
        }
    }

    pub(crate) fn label(&self) -> &'static str {
        match self.kind {
            InstallKind::BrewCask => "Homebrew cask",
            InstallKind::Bundle => "приложение из .dmg (бинарь в бандле)",
            InstallKind::Cargo => "cargo install",
            InstallKind::Dev => "dev-сборка из исходников",
            InstallKind::Unknown => "неизвестно",
        }
    }

    /// Единственный путь обновления, который для этой установки сработает.
    pub(crate) fn upgrade(&self) -> &'static str {
        match self.kind {
            // auto_updates в каске значит, что обычный `brew upgrade` его
            // пропускает, — без --greedy совет был бы бесполезным.
            InstallKind::BrewCask => {
                "brew upgrade --cask --greedy sql-kai (каск с auto_updates, без --greedy \
                 brew его пропустит); CLI — симлинк в бандл, обновится вместе с приложением"
            }
            InstallKind::Bundle => {
                "приложение обновляет себя само (кнопка в статус-баре GUI) либо свежий .dmg \
                 из релизов; CLI — симлинк в бандл, обновится вместе с ним"
            }
            InstallKind::Cargo => {
                "cargo install --path src-tauri --features cli --bin sql-kai-cli из свежего чекаута"
            }
            InstallKind::Dev => {
                "git pull && cargo build --release --features cli --bin sql-kai-cli"
            }
            InstallKind::Unknown => {
                "источник бинаря не опознан — поставь заново: brew install kaidstor/tap/sql-kai \
                 либо симлинк на sql-kai-cli внутри бандла приложения"
            }
        }
    }

    /// Приложение установлено, а CLI собран отдельно — версии разъедутся первым
    /// же обновлением GUI (конфиги общие, набор команд — нет).
    pub(crate) fn note(&self) -> Option<String> {
        let app = PathBuf::from(APP_DIR).join(format!("{APP_NAME}.app"));
        let separate = matches!(
            self.kind,
            InstallKind::Cargo | InstallKind::Dev | InstallKind::Unknown
        );
        (separate && app.exists()).then(|| {
            format!(
                "рядом стоит {}, но в PATH — отдельная сборка CLI: версии GUI и CLI будут \
                 расходиться. Вернуть связку: ln -sf {}/Contents/MacOS/sql-kai-cli <путь-в-PATH>/sql-kai",
                app.display(),
                app.display()
            )
        })
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": self.id(),
            "label": self.label(),
            "version": env!("CARGO_PKG_VERSION"),
            "bin": self.invoked.display().to_string(),
            "realBin": self.real.display().to_string(),
            "bundle": self.bundle.as_ref().map(|p| p.display().to_string()),
            "caskroom": self.caskroom.as_ref().map(|p| p.display().to_string()),
            "upgrade": self.upgrade(),
            "note": self.note(),
        })
    }

    /// Ключи те же, что в JSON, — csv/tuples остаются разбираемыми теми же полями.
    fn kv(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            ("kind", self.id().to_string()),
            ("version", env!("CARGO_PKG_VERSION").to_string()),
            ("bin", self.invoked.display().to_string()),
            ("realBin", self.real.display().to_string()),
        ];
        if let Some(b) = &self.bundle {
            rows.push(("bundle", b.display().to_string()));
        }
        if let Some(c) = &self.caskroom {
            rows.push(("caskroom", c.display().to_string()));
        }
        rows.push(("upgrade", self.upgrade().to_string()));
        if let Some(n) = self.note() {
            rows.push(("note", n));
        }
        rows
    }
}

/// Человеческий блок про установку: что стоит, откуда запущено, чем обновлять.
fn print_install_table(i: &InstallInfo) {
    println!(
        "установка:  {} (sql-kai {})",
        i.label(),
        env!("CARGO_PKG_VERSION")
    );
    if i.real == i.invoked {
        println!("бинарь:     {}", i.invoked.display());
    } else {
        println!("бинарь:     {} → {}", i.invoked.display(), i.real.display());
    }
    if let Some(c) = &i.caskroom {
        println!("признак:    {}", c.display());
    }
    println!("обновление: {}", i.upgrade());
    if let Some(n) = i.note() {
        println!("внимание:   {n}");
    }
}

fn print_install(i: &InstallInfo, fmt: Format) {
    match fmt {
        Format::Json => println!("{}", serde_json::to_string_pretty(&i.json()).unwrap()),
        Format::Table => print_install_table(i),
        Format::Csv | Format::Tuples => {
            let rows: Vec<Vec<Option<String>>> = i
                .kv()
                .into_iter()
                .map(|(k, v)| vec![Some(k.to_string()), Some(v)])
                .collect();
            output::print_rows(&["key", "value"], &rows, fmt);
        }
    }
}

/// Итог проверки одного источника пароля.
fn probe_label(res: &Result<(), AppError>) -> &'static str {
    if res.is_ok() {
        "ok"
    } else {
        "fail"
    }
}

async fn can_connect(profile: &Profile, password: Option<String>) -> Result<(), AppError> {
    let c = db::connect(
        profile,
        db::ConnectOptions {
            password_override: password,
            ssh_mux_ttl: Some(session::mux_ttl()),
            ..Default::default()
        },
    )
    .await?;
    drop(c);
    Ok(())
}

pub async fn run(a: DoctorArgs) -> Result<ExitCode, AppError> {
    let fmt = a.fmt.pick();
    let install = detect_install();
    if a.install_info {
        print_install(&install, fmt);
        return Ok(ExitCode::SUCCESS);
    }
    // В машинных форматах вывод команды — это таблица профилей; блок про
    // установку в них сломал бы разбор, за ним ходят через --install-info.
    if fmt == Format::Table {
        print_install_table(&install);
        println!();
    }

    let mut profiles = store::load_profiles()?;
    if let Some(alias) = &a.alias {
        // Единый резолв с `sql-kai q`: id, имя или группа.
        profiles = session::filter_profiles(&profiles, alias);
        if profiles.is_empty() {
            return Err(AppError::Msg(format!("профиль '{alias}' не найден")));
        }
    }
    // Разлочим vault best-effort, чтобы проверить vault-пароли; не вышло — просто
    // отметим их как locked.
    let vault_ok = session::unlock_vault().is_ok();
    let sec_ok = sec::available().is_ok();

    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut any_problem = false;

    for p in &profiles {
        // vault
        let vault_state = if !p.has_password {
            "no-pw".to_string()
        } else if !vault_ok {
            "locked".to_string()
        } else {
            probe_label(&can_connect(p, None).await).to_string()
        };

        // sec (по конвенционному ключу)
        let key = sec::default_key(p);
        let sec_value = if sec_ok {
            sec::get(&key).ok().flatten()
        } else {
            None
        };
        let sec_state = match &sec_value {
            None => "absent".to_string(),
            Some(v) => probe_label(&can_connect(p, Some(v.clone())).await).to_string(),
        };

        // "fail" бывает только когда пароль реально был и не подошёл — это и есть
        // проблема (дрейф/протухший креденшел); absent/no-pw/locked/ok — нет.
        let problem = vault_state == "fail" || sec_state == "fail";
        if problem {
            any_problem = true;
        }
        let note = match (vault_state.as_str(), sec_state.as_str()) {
            ("ok", _) => "",
            ("fail", "ok") => "дрейф: vault не подходит, работает sec",
            ("fail", _) => "vault-пароль не подходит",
            ("no-pw", "ok") => "sec-only ok",
            ("no-pw", "fail") => "sec-пароль не подходит",
            ("no-pw", "absent") => "нет сохранённого пароля — только --password-env",
            ("locked", "ok") => "vault заблокирован, работает sec",
            ("locked", _) => "vault заблокирован — проверить нельзя",
            _ => "",
        };

        rows.push(serde_json::json!({
            "name": p.name,
            "vault": vault_state,
            "sec": sec_state,
            "note": note,
        }));
    }

    if fmt == Format::Json {
        println!("{}", serde_json::to_string_pretty(&rows).unwrap());
    } else {
        let table: Vec<Vec<Option<String>>> = rows
            .iter()
            .map(|r| {
                vec![
                    r["name"].as_str().map(str::to_string),
                    r["vault"].as_str().map(str::to_string),
                    r["sec"].as_str().map(str::to_string),
                    r["note"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                ]
            })
            .collect();
        output::print_rows(&["profile", "vault", "sec", "note"], &table, fmt);
        if sec_ok && fmt == Format::Table {
            eprintln!("подсказка: что пора ротировать по срокам — `sec stale`");
        }
    }

    Ok(if any_problem {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::{bundle_of, cask_owns_bundle, in_target_dir};
    use std::path::{Path, PathBuf};

    #[test]
    fn bundle_of_finds_the_app() {
        assert_eq!(
            bundle_of(Path::new(
                "/Applications/sql-kai.app/Contents/MacOS/sql-kai-cli"
            )),
            Some(PathBuf::from("/Applications/sql-kai.app"))
        );
        assert_eq!(bundle_of(Path::new("/opt/homebrew/bin/sql-kai")), None);
        // .app в имени самого бинаря бандлом не делает
        assert_eq!(bundle_of(Path::new("/tmp/sql-kai-cli")), None);
    }

    #[test]
    fn in_target_dir_matches_cargo_layout() {
        assert!(in_target_dir(Path::new(
            "/repo/src-tauri/target/debug/sql-kai-cli"
        )));
        assert!(in_target_dir(Path::new(
            "/repo/src-tauri/target/release/sql-kai-cli"
        )));
        // бандл tauri лежит глубже, но это всё та же сборка из исходников
        assert!(in_target_dir(Path::new(
            "/repo/src-tauri/target/release/bundle/macos/sql-kai.app/Contents/MacOS/sql-kai-cli"
        )));
        // чужой каталог с тем же именем профиля — не сборка
        assert!(!in_target_dir(Path::new("/opt/release/sql-kai-cli")));
        assert!(!in_target_dir(Path::new("/opt/homebrew/bin/sql-kai")));
        assert!(!in_target_dir(Path::new(
            "/Applications/sql-kai.app/Contents/MacOS/sql-kai-cli"
        )));
    }

    /// Каск объясняет только свой бандл. Иначе dev-сборка на машине, где стоит
    /// каск, получала бы диагноз «Homebrew cask» и совет `brew upgrade`,
    /// который её не обновит, — и тот же диагноз уезжал бы в issue.
    #[test]
    fn cask_owns_only_its_own_bundle() {
        let caskroom = PathBuf::from("/opt/homebrew/Caskroom/sql-kai");
        let app_dirs = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/Users/kai/Applications"),
        ];
        for owned in [
            "/Applications/sql-kai.app",
            "/Users/kai/Applications/sql-kai.app",
            "/opt/homebrew/Caskroom/sql-kai/1.20.1/sql-kai.app",
        ] {
            assert!(
                cask_owns_bundle(Path::new(owned), &caskroom, &app_dirs),
                "{owned}"
            );
        }
        for alien in [
            "/Users/kai/sql-kai/src-tauri/target/release/bundle/macos/sql-kai.app",
            "/Users/kai/Desktop/sql-kai.app",
            "/Applications/sql-kai-old.app",
        ] {
            assert!(
                !cask_owns_bundle(Path::new(alien), &caskroom, &app_dirs),
                "{alien}"
            );
        }
    }
}
