//! `sql-kai completion <shell>` — скрипты автодополнения для zsh/bash/fish.
//!
//! Поверх статического скрипта clap_complete идёт эпилог с динамическим
//! дополнением имён профилей. Оно нужно, потому что первый позиционный
//! аргумент sql-kai — это алиас профиля (`sql-kai domainator -c "…"`), а для
//! clap его не существует: шорткат разворачивается в `q` ещё до разбора
//! (см. `preprocess`). Список профилей к тому же меняется между
//! вызовами, поэтому вшить его в скрипт нельзя — эпилог зовёт
//! `sql-kai completion --profiles` в момент нажатия Tab. Движок
//! `unstable-dynamic` из clap_complete делает то же самое, но его API
//! нестабилен и тянет за собой переменную окружения-переключатель — цена
//! выше, чем десяток строк на шелл.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::{Args, CommandFactory, ValueEnum};
use clap_complete::aot;
use sql_kai_lib::error::AppError;
use sql_kai_lib::store;

#[derive(Args)]
pub struct CompletionArgs {
    /// Шелл: zsh, bash или fish (по умолчанию — из $SHELL)
    #[arg(value_enum)]
    shell: Option<CompletionShell>,
    /// Напечатать алиасы профилей по одному в строке — это вызывает сам
    /// сгенерированный скрипт, руками не нужно
    #[arg(long, conflicts_with = "shell")]
    profiles: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    Zsh,
    Bash,
    Fish,
}

impl CompletionShell {
    pub fn as_str(self) -> &'static str {
        match self {
            CompletionShell::Zsh => "zsh",
            CompletionShell::Bash => "bash",
            CompletionShell::Fish => "fish",
        }
    }
}

/// Шелл из `$SHELL` — чтобы `sql-kai completion` и `sql-kai init` не
/// переспрашивали очевидное. Незнакомый шелл — None, а не молчаливый zsh.
pub fn detect_shell() -> Option<CompletionShell> {
    let shell = std::env::var("SHELL").ok()?;
    match shell.rsplit('/').next()? {
        "zsh" => Some(CompletionShell::Zsh),
        "bash" => Some(CompletionShell::Bash),
        "fish" => Some(CompletionShell::Fish),
        _ => None,
    }
}

/// Алиасы, которые примет `resolve_profile`: имена профилей и группы.
/// Дополнение обязано работать без сети и без vault и молчать (а не падать),
/// когда profiles.json ещё нет — иначе Tab начнёт сыпать ошибками в шелл.
pub fn profile_aliases() -> Vec<String> {
    let profiles = store::load_profiles().unwrap_or_default();
    let mut names: Vec<String> = profiles
        .iter()
        .map(|p| p.name.clone())
        .chain(profiles.iter().filter_map(|p| p.group.clone()))
        .filter(|s| !s.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// Скрипт автодополнения целиком: clap_complete + эпилог с профилями.
pub fn script(shell: CompletionShell) -> String {
    let mut cmd = crate::Cli::command();
    let mut out: Vec<u8> = Vec::new();
    match shell {
        CompletionShell::Zsh => aot::generate(aot::Zsh, &mut cmd, "sql-kai", &mut out),
        CompletionShell::Bash => aot::generate(aot::Bash, &mut cmd, "sql-kai", &mut out),
        CompletionShell::Fish => aot::generate(aot::Fish, &mut cmd, "sql-kai", &mut out),
    }
    let mut script = String::from_utf8_lossy(&out).into_owned();
    script.push_str(match shell {
        CompletionShell::Zsh => ZSH_PROFILES,
        CompletionShell::Bash => BASH_PROFILES,
        CompletionShell::Fish => FISH_PROFILES,
    });
    script
}

/// Имя файла, под которым скрипт принято класть на диск.
pub fn file_name(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Zsh => "completion.zsh",
        CompletionShell::Bash => "completion.bash",
        CompletionShell::Fish => "sql-kai.fish",
    }
}

/// zsh: подменяем сгенерированную `_sql-kai` обёрткой, которая на первом слове
/// сперва предлагает профили, а потом отдаёт управление clap-версии (так в
/// списке остаются и подкоманды). Копия старой функции — через `functions[…]`;
/// повторный source файла безопасен: clap-часть каждый раз переопределяет
/// `_sql-kai` заново, так что обёртка не начнёт звать сама себя.
const ZSH_PROFILES: &str = r#"
# --- профили sql-kai (динамическое дополнение) --------------------------------
_sql_kai_profiles() {
    local -a profiles
    profiles=(${(f)"$(command sql-kai completion --profiles 2>/dev/null)"})
    (( ${#profiles} )) || return
    # _describe читает элемент как 'значение:описание', поэтому двоеточие в
    # самом имени обрезало бы алиас: профиль `prod:main` дополнялся как `prod`,
    # и запрос уходил в другой профиль. Экранируем — _describe снимет обратно.
    profiles=("${profiles[@]//:/\\:}")
    _describe -t sql-kai-profiles 'профиль' profiles
}

if (( $+functions[_sql-kai] )); then
    functions[_sql_kai_clap]=$functions[_sql-kai]
    _sql-kai() {
        (( CURRENT == 2 )) && _sql_kai_profiles
        _sql_kai_clap "$@"
    }
fi
"#;

/// bash: своя функция поверх сгенерированной — clap заполняет COMPREPLY
/// подкомандами, мы дописываем профили. `complete` в конце перекрывает
/// регистрацию из clap-части (она идёт выше по файлу). Отбор префикса руками,
/// а не через `compgen -W`: имена профилей бывают с пробелами («white label»),
/// а их надо и разделить по строкам, и заэкранировать при подстановке.
///
/// Квотирование обязательно и не косметическое: bash вставляет элемент
/// COMPREPLY в командную строку как есть, а имя профиля приходит из
/// profiles.json — в т.ч. чужого, принятого через `sql-kai import`. Имя вида
/// `db$(…)` после Tab и Enter исполнилось бы как подстановка команды. В zsh
/// (compadd квотирует сам) и fish этой дыры нет, поэтому чинится только bash.
const BASH_PROFILES: &str = r#"
# --- профили sql-kai (динамическое дополнение) --------------------------------
_sql_kai_with_profiles() {
    _sql-kai "$@"
    if [[ ${COMP_CWORD} -eq 1 ]]; then
        local cur="${COMP_WORDS[COMP_CWORD]}" name quoted
        while IFS= read -r name; do
            [[ -n "${name}" && "${name}" == "${cur}"* ]] || continue
            # printf %q, а не подмена одних пробелов: $( ), `, ;, кавычки в
            # имени иначе попадают в командную строку живыми метасимволами.
            printf -v quoted '%q' "${name}"
            COMPREPLY+=( "${quoted}" )
        done <<< "$(command sql-kai completion --profiles 2>/dev/null)"
    fi
}

complete -F _sql_kai_with_profiles -o bashdefault -o default sql-kai
"#;

/// fish: отдельное правило на позицию «подкоманда ещё не названа».
/// `__fish_use_subcommand` — штатный хелпер fish, а не внутренняя функция
/// генератора, поэтому не сломается при обновлении clap_complete.
const FISH_PROFILES: &str = r#"
# --- профили sql-kai (динамическое дополнение) --------------------------------
complete -c sql-kai -n "__fish_use_subcommand" -f -a "(command sql-kai completion --profiles 2>/dev/null)" -d "профиль"
"#;

pub fn run(a: CompletionArgs) -> Result<ExitCode, AppError> {
    if a.profiles {
        for alias in profile_aliases() {
            println!("{alias}");
        }
        return Ok(ExitCode::SUCCESS);
    }
    let shell = a.shell.or_else(detect_shell).ok_or_else(|| {
        AppError::Msg(
            "не понял шелл из $SHELL — укажи явно: sql-kai completion zsh|bash|fish".into(),
        )
    })?;
    io::stdout().write_all(script(shell).as_bytes())?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    const SHELLS: [CompletionShell; 3] = [
        CompletionShell::Zsh,
        CompletionShell::Bash,
        CompletionShell::Fish,
    ];

    /// Эпилог с профилями — единственное, ради чего мы не отдаём голый вывод
    /// clap_complete; заодно ловит переезд команды `completion --profiles`.
    #[test]
    fn every_script_calls_back_for_profiles() {
        for shell in SHELLS {
            let script = script(shell);
            assert!(
                script.contains("sql-kai completion --profiles"),
                "{}: нет вызова за профилями",
                shell.as_str()
            );
        }
    }

    /// clap-часть должна остаться на месте: эпилог zsh переопределяет её
    /// функцию и без неё молча ничего не дополнит.
    #[test]
    fn zsh_script_wraps_generated_function() {
        let script = script(CompletionShell::Zsh);
        assert!(script.contains("#compdef sql-kai"));
        assert!(script.contains("functions[_sql_kai_clap]=$functions[_sql-kai]"));
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("sql-kai-completion-{tag}-{}", std::process::id()))
    }

    /// Подставной `sql-kai` в PATH: эпилоги спрашивают имена профилей у команды
    /// (`command sql-kai completion --profiles`), поэтому иначе их не проверить.
    fn fake_cli(dir: &Path, names: &[&str]) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).unwrap();
        let bin = dir.join("sql-kai");
        // heredoc с закавыченным маркером: имена печатаются буквально, как их
        // отдал бы настоящий `completion --profiles`.
        std::fs::write(
            &bin,
            format!("#!/bin/sh\ncat <<'NAMES'\n{}\nNAMES\n", names.join("\n")),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// stdout скрипта; None — такого шелла в системе нет и проверять нечего.
    fn run_shell(shell: &str, args: &[&str], dir: &Path, script: &str) -> Option<String> {
        let file = dir.join(format!("driver.{shell}"));
        std::fs::write(&file, script).unwrap();
        let path = format!(
            "{}:{}",
            dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = Command::new(shell)
            .args(args)
            .arg(&file)
            .env("PATH", path)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn lines_with<'a>(out: &'a str, prefix: &str) -> Vec<&'a str> {
        out.lines().filter_map(|l| l.strip_prefix(prefix)).collect()
    }

    /// bash вставляет элемент COMPREPLY в командную строку как есть, а имя
    /// профиля приходит из profiles.json — в том числе чужого, принятого через
    /// `sql-kai import`. Гоняем настоящий bash: проверяется поведение
    /// `printf %q`, а не наличие строки в скрипте.
    #[test]
    fn bash_completion_quotes_shell_metacharacters() {
        let dir = tmp_dir("bash");
        let marker = dir.join("pwned");
        let hostile = format!("db$(touch {})", marker.display());
        let names = [hostile.as_str(), "white label", "prod;rm -rf /", "plain"];
        fake_cli(&dir, &names);
        let script = format!(
            "_sql-kai() {{ :; }}\n{BASH_PROFILES}\n\
             COMP_CWORD=1\nCOMP_WORDS=(sql-kai '')\nCOMPREPLY=()\n\
             _sql_kai_with_profiles sql-kai '' sql-kai\n\
             printf 'raw %s\\n' \"${{COMPREPLY[@]}}\"\n\
             for c in \"${{COMPREPLY[@]}}\"; do eval \"printf 'word %s\\n' $c\"; done\n"
        );
        let Some(out) = run_shell("bash", &[], &dir, &script) else {
            return;
        };
        // подстановка команды не исполнилась — Tab не запускает чужой код
        assert!(!marker.exists(), "$(…) из имени профиля исполнился");
        // кандидат уходит в строку заэкранированным…
        let raw = lines_with(&out, "raw ");
        assert_eq!(raw.len(), names.len(), "вывод: {out}");
        assert!(
            !raw.contains(&hostile.as_str()),
            "метасимволы не заэкранированы: {out}"
        );
        // …но после разбора шеллом это ровно исходное имя
        assert_eq!(lines_with(&out, "word "), names, "вывод: {out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `_describe` читает элемент как `значение:описание`: без экранирования
    /// профиль `prod:main` дополнялся бы до `prod` — то есть в другой профиль.
    #[test]
    fn zsh_completion_escapes_colons_in_names() {
        let dir = tmp_dir("zsh");
        fake_cli(&dir, &["prod:main", "a:b:c", "plain"]);
        // заглушка вместо _describe печатает массив, имя которого ей передали
        // последним аргументом
        let script = format!(
            "_describe() {{ print -rl -- \"${{(@P)argv[-1]}}\" }}\n{ZSH_PROFILES}\n_sql_kai_profiles\n"
        );
        let Some(out) = run_shell("zsh", &["-f"], &dir, &script) else {
            return;
        };
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            vec![r"prod\:main", r"a\:b\:c", "plain"],
            "вывод: {out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
