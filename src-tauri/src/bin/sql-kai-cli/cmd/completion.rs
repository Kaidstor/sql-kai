//! `sql-kai completion <shell>` — скрипты автодополнения для zsh/bash/fish.
//!
//! Поверх статического скрипта clap_complete идёт эпилог с динамическим
//! дополнением имён профилей. Оно нужно, потому что первый позиционный
//! аргумент sql-kai — это алиас профиля (`sql-kai domainator -c "…"`), а для
//! clap его не существует: шорткат разворачивается в `q` ещё до разбора
//! (см. `preprocess_args`). Список профилей к тому же меняется между
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
    (( ${#profiles} )) && _describe -t sql-kai-profiles 'профиль' profiles
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
const BASH_PROFILES: &str = r#"
# --- профили sql-kai (динамическое дополнение) --------------------------------
_sql_kai_with_profiles() {
    _sql-kai "$@"
    if [[ ${COMP_CWORD} -eq 1 ]]; then
        local cur="${COMP_WORDS[COMP_CWORD]}" name
        while IFS= read -r name; do
            [[ -n "${name}" && "${name}" == "${cur}"* ]] && COMPREPLY+=( "${name// /\\ }" )
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
}
