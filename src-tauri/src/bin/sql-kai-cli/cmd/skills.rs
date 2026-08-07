//! `sql-kai skills` — раскладка агентского скилла (SKILL.md вкомпилирован в
//! бинарь) по каталогам агентов. Обновляется дальше сам: постфлайт каска,
//! чек на старте GUI и после любого вызова CLI перезаписывают устаревшие копии.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};
use sql_kai_lib::skills_sync as sync;

#[derive(Args)]
pub struct SkillsArgs {
    #[command(subcommand)]
    pub cmd: SkillsCmd,
}

#[derive(Subcommand)]
pub enum SkillsCmd {
    /// Установить скилл во все найденные каталоги агентов
    Install(InstallArgs),
    /// Где установлен, какой версии, обновляется ли сам
    Status,
}

#[derive(Args)]
pub struct InstallArgs {
    /// Только один агент: claude | codex
    #[arg(long, value_name = "АГЕНТ")]
    pub target: Option<String>,

    /// Точный каталог скилла вместо автопоиска агентов
    #[arg(long, value_name = "ПУТЬ", conflicts_with = "target")]
    pub dir: Option<PathBuf>,

    /// Симлинк на каталог скилла в рабочем дереве вместо копии (dev);
    /// без значения — путь репо, из которого собран бинарь
    // Option<Option<…>>, а не default_missing_value = "": парсер PathBuf в clap
    // отвергает пустую строку как «значение не передано», и голый --link падает
    #[arg(long, value_name = "ПУТЬ")]
    pub link: Option<Option<PathBuf>>,

    /// Заменить и симлинк / чужую копию тоже
    #[arg(long)]
    pub force: bool,
}

fn dirs_for(args: &InstallArgs) -> Result<Vec<(&'static str, PathBuf)>, String> {
    if let Some(dir) = &args.dir {
        return Ok(vec![("dir", dir.clone())]);
    }
    let all = sync::agent_dirs();
    if all.is_empty() {
        return Err("не найдено ни ~/.claude, ни ~/.codex — агентов на машине нет".into());
    }
    match args.target.as_deref() {
        None => Ok(all),
        Some(want) => {
            let found: Vec<_> = all.iter().filter(|(n, _)| *n == want).cloned().collect();
            if found.is_empty() {
                let known: Vec<&str> = all.iter().map(|(n, _)| *n).collect();
                return Err(format!(
                    "агент «{want}» не найден. Есть: {}",
                    known.join(", ")
                ));
            }
            Ok(found)
        }
    }
}

fn install(args: &InstallArgs) -> Result<ExitCode, String> {
    let link_target = match &args.link {
        None => None,
        Some(None) => Some(PathBuf::from(sync::REPO_SKILL_DIR)),
        Some(Some(p)) => Some(p.clone()),
    };
    if let Some(t) = &link_target {
        if !t.join("SKILL.md").is_file() {
            return Err(format!(
                "в {} нет SKILL.md — укажи каталог скилла явно: --link ПУТЬ",
                t.display()
            ));
        }
    }

    let mut failed = false;
    for (name, dir) in dirs_for(args)? {
        let res = match &link_target {
            Some(t) => {
                sync::link(&dir, t, args.force).map(|_| format!("симлинк → {}", t.display()))
            }
            None => sync::install(&dir, args.force).map(|_| format!("копия v{}", sync::VERSION)),
        };
        match res {
            Ok(what) => println!("✓ {name}: {what} — {}", dir.display()),
            Err(e) => {
                eprintln!("sql-kai: {name}: {e}");
                failed = true;
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn status() -> ExitCode {
    let all = sync::agent_dirs();
    if all.is_empty() {
        println!("агентов не найдено (нет ни ~/.claude, ни ~/.codex)");
        return ExitCode::SUCCESS;
    }
    for (name, dir) in all {
        let line = match sync::state(&dir) {
            sync::State::Missing => "не установлен (sql-kai skills install)".to_string(),
            sync::State::Symlink(target) => {
                format!("симлинк → {} (обновляется сам)", target.display())
            }
            sync::State::Managed { version } if version == sync::VERSION => {
                format!("копия v{version}, актуальна")
            }
            sync::State::Managed { version } => format!(
                "копия v{version}, устарела (текущая v{}) — обновится сама при следующем вызове CLI",
                sync::VERSION
            ),
            sync::State::Manual => {
                "копия без стампа — не обновляется; sql-kai skills install возьмёт под управление"
                    .to_string()
            }
        };
        println!("{name}: {line} — {}", dir.display());
    }
    ExitCode::SUCCESS
}

pub fn run(args: SkillsArgs) -> Result<ExitCode, String> {
    match args.cmd {
        SkillsCmd::Install(a) => install(&a),
        SkillsCmd::Status => Ok(status()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(flatten)]
        install: InstallArgs,
    }

    #[test]
    fn bare_link_means_repo_path() {
        let t = T::try_parse_from(["t", "--link"]).expect("голый --link должен парситься");
        assert_eq!(t.install.link, Some(None));
    }

    #[test]
    fn link_with_path() {
        let t = T::try_parse_from(["t", "--link", "/tmp/x"]).unwrap();
        assert_eq!(t.install.link, Some(Some(PathBuf::from("/tmp/x"))));
    }

    #[test]
    fn no_link_by_default() {
        let t = T::try_parse_from(["t"]).unwrap();
        assert_eq!(t.install.link, None);
    }
}
