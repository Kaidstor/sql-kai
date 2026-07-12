//! `sql-kai tunnel` — персистентные ssh-туннели (ControlMaster), переиспользуемые
//! между вызовами: список живых мастеров и их закрытие.

use std::process::ExitCode;

use clap::Subcommand;
use sql_kai_lib::error::AppError;
use sql_kai_lib::tunnel;

use crate::output;

#[derive(Subcommand)]
pub enum TunnelCmd {
    /// Показать живые ssh-мастера (по умолчанию)
    List,
    /// Закрыть мастер(а): по хосту/имени сокета или все
    Close {
        /// Хост или имя сокета; без него — все
        target: Option<String>,
        /// Закрыть все
        #[arg(long)]
        all: bool,
    },
}

pub fn run(cmd: TunnelCmd) -> Result<ExitCode, AppError> {
    match cmd {
        TunnelCmd::List => {
            let masters = tunnel::list_masters();
            if masters.is_empty() {
                println!("нет живых ssh-мастеров");
                return Ok(ExitCode::SUCCESS);
            }
            let rows: Vec<Vec<Option<String>>> = masters
                .iter()
                .map(|m| {
                    vec![
                        Some(m.target.clone()),
                        Some(if m.alive { "alive" } else { "stale" }.into()),
                    ]
                })
                .collect();
            output::print_rows(&["target", "state"], &rows, output::Format::Table);
        }
        TunnelCmd::Close { target, all } => {
            let only = if all { None } else { target.as_deref() };
            if only.is_none() && !all {
                return Err(AppError::Msg(
                    "укажи хост/имя сокета или --all".into(),
                ));
            }
            let n = tunnel::close_masters(only);
            println!("закрыто мастеров: {n}");
        }
    }
    Ok(ExitCode::SUCCESS)
}
