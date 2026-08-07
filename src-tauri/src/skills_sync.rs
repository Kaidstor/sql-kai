//! Агентский скилл sql-kai (skills/sql-kai/SKILL.md) вкомпилирован в бинарь и
//! раскладывается по каталогам агентов (~/.claude/skills, ~/.codex/skills) —
//! версия скилла по построению совпадает с версией бинаря.
//!
//! Наша копия помечена стампом `.sql-kai-version`. Самолечение
//! (`sync_all_stale` на старте GUI и после любого вызова CLI) трогает
//! **только** каталоги со стампом: симлинки (`--link`, dev) и разложенные
//! руками копии без стампа не перезаписываются — иначе апдейт молча затирал
//! бы чужое.

use std::fs;
use std::path::{Path, PathBuf};

pub const SKILL_MD: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../skills/sql-kai/SKILL.md"
));
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const STAMP: &str = ".sql-kai-version";

/// Путь к скиллу в рабочем дереве репо на момент сборки — дефолт для
/// `skills install --link` на dev-машине.
pub const REPO_SKILL_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../skills/sql-kai");

/// Каталоги скилла у агентов, найденных на машине. Агент считается
/// установленным по наличию его домашнего каталога (~/.claude, ~/.codex);
/// самого skills/ может ещё не быть — install создаст.
pub fn agent_dirs() -> Vec<(&'static str, PathBuf)> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    [("claude", ".claude"), ("codex", ".codex")]
        .into_iter()
        .filter(|(_, dot)| home.join(dot).is_dir())
        .map(|(name, dot)| (name, home.join(dot).join("skills").join("sql-kai")))
        .collect()
}

pub enum State {
    Missing,
    /// Симлинк — dev-режим или ручной; обновляется сам, не трогаем.
    Symlink(PathBuf),
    /// Наша копия со стампом — её и обновляем.
    Managed {
        version: String,
    },
    /// Каталог без стампа — разложен руками, самолечение обходит стороной.
    Manual,
}

pub fn state(dir: &Path) -> State {
    if dir.is_symlink() {
        let target = fs::read_link(dir).unwrap_or_else(|_| dir.to_path_buf());
        return State::Symlink(target);
    }
    if !dir.is_dir() {
        return State::Missing;
    }
    match fs::read_to_string(dir.join(STAMP)) {
        Ok(v) => State::Managed {
            version: v.trim().to_string(),
        },
        Err(_) => State::Manual,
    }
}

/// Пишет копию скилла со стампом. Существующие копии (наши и ручные)
/// перезаписываются — явный install и есть согласие; симлинк — только с force.
pub fn install(dir: &Path, force: bool) -> Result<(), String> {
    let ctx = |e: std::io::Error| format!("{}: {e}", dir.display());
    if dir.is_symlink() {
        if !force {
            return Err(format!(
                "{} — симлинк, оставлен как есть (--force — заменить копией)",
                dir.display()
            ));
        }
        fs::remove_file(dir).map_err(ctx)?;
    }
    fs::create_dir_all(dir).map_err(ctx)?;
    fs::write(dir.join("SKILL.md"), SKILL_MD).map_err(ctx)?;
    fs::write(dir.join(STAMP), format!("{VERSION}\n")).map_err(ctx)?;
    Ok(())
}

/// Симлинк на каталог скилла в рабочем дереве (dev): правка SKILL.md живая
/// сразу, самолечение такой каталог не трогает.
#[cfg(unix)]
pub fn link(dir: &Path, target: &Path, force: bool) -> Result<(), String> {
    if !target.join("SKILL.md").is_file() {
        return Err(format!("в {} нет SKILL.md", target.display()));
    }
    match state(dir) {
        State::Missing => {}
        State::Symlink(_) | State::Managed { .. } => {
            // свой симлинк и своя копия пересоздаются без force
            if dir.is_symlink() {
                fs::remove_file(dir)
            } else {
                fs::remove_dir_all(dir)
            }
            .map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        State::Manual if force => {
            fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        State::Manual => {
            return Err(format!(
                "{} — копия без стампа (не наша), --force — заменить",
                dir.display()
            ));
        }
    }
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::os::unix::fs::symlink(target, dir).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Наша копия устарела — молча перезаписать. true, если обновил.
pub fn sync_if_stale(dir: &Path) -> bool {
    match state(dir) {
        State::Managed { version } if version != VERSION => install(dir, false).is_ok(),
        _ => false,
    }
}

/// Самолечение по всем агентам: бест-эффорт, ошибки глотаются — вызывается
/// фоном на старте GUI и после CLI-команд, мешать основной работе нельзя.
pub fn sync_all_stale() -> Vec<PathBuf> {
    agent_dirs()
        .into_iter()
        .filter(|(_, dir)| sync_if_stale(dir))
        .map(|(_, dir)| dir)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sql-kai-skills-{}-{}",
                name,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            let _ = fs::remove_file(&dir);
            Self(dir)
        }
        fn skill(&self) -> PathBuf {
            self.0.join("sql-kai")
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn install_writes_copy_with_stamp() {
        let t = TmpDir::new("install");
        let dir = t.skill();
        install(&dir, false).unwrap();
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), SKILL_MD);
        assert_eq!(fs::read_to_string(dir.join(STAMP)).unwrap().trim(), VERSION);
        assert!(matches!(state(&dir), State::Managed { .. }));
    }

    #[test]
    fn sync_updates_stale_managed_copy() {
        let t = TmpDir::new("stale");
        let dir = t.skill();
        install(&dir, false).unwrap();
        fs::write(dir.join(STAMP), "0.0.1\n").unwrap();
        fs::write(dir.join("SKILL.md"), "старьё").unwrap();
        assert!(sync_if_stale(&dir));
        assert_eq!(fs::read_to_string(dir.join("SKILL.md")).unwrap(), SKILL_MD);
        // повторный вызов уже ничего не делает
        assert!(!sync_if_stale(&dir));
    }

    #[test]
    fn sync_skips_manual_copy_without_stamp() {
        let t = TmpDir::new("manual");
        let dir = t.skill();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "ручная копия").unwrap();
        assert!(!sync_if_stale(&dir));
        assert_eq!(
            fs::read_to_string(dir.join("SKILL.md")).unwrap(),
            "ручная копия"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_untouched_without_force() {
        let t = TmpDir::new("symlink");
        let target = t.0.join("repo-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "живой из репо").unwrap();
        let dir = t.skill();
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        assert!(!sync_if_stale(&dir));
        assert!(install(&dir, false).is_err());
        assert!(dir.is_symlink(), "симлинк должен остаться");

        install(&dir, true).unwrap();
        assert!(!dir.is_symlink());
        assert!(matches!(state(&dir), State::Managed { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn link_replaces_own_copy_but_not_manual() {
        let t = TmpDir::new("link");
        let target = t.0.join("repo-skill");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("SKILL.md"), "живой из репо").unwrap();
        let dir = t.skill();

        install(&dir, false).unwrap();
        link(&dir, &target, false).unwrap();
        assert!(dir.is_symlink());

        fs::remove_file(&dir).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "ручная копия").unwrap();
        assert!(link(&dir, &target, false).is_err());
        assert!(link(&dir, &target, true).is_ok());
    }
}
