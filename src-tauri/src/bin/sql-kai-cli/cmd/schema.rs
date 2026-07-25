//! `sql-kai schema <alias>` — вся схема базы одним дампом: таблицы (обычные,
//! партиционированные, foreign), вьюхи и матвьюхи с колонками, ключами,
//! индексами и триггерами, плюс enum-типы, функции и процедуры.
//!
//! Команда существует ради агента: без неё он делает `tables`, а затем
//! `columns`/`indexes`/`ddl` на каждую таблицу — десятки round-trip и рваная
//! картина каталога. Здесь весь каталог приходит одним батчем (см.
//! `db::schema_dump_sql`), то есть за фиксированное число запросов независимо
//! от размера базы и из одного снимка.
//!
//! По умолчанию вывод чистится до того, что человек (и модель) реально читает:
//! без системных схем, без объектов расширений (postgis/timescale раздувают
//! дамп на порядки), без листовых партиций, без тел вьюх/функций и без
//! COMMENT ON. Всё это возвращают `--internal`, `--definitions`, `--comments`.

use std::collections::BTreeMap;
use std::process::ExitCode;

use clap::Args;
use serde::Serialize;
use sql_kai_lib::db;
use sql_kai_lib::error::AppError;

use crate::session;

/// Дамп отдаёт строку на каждую колонку/индекс/констрейнт, поэтому
/// `db::query_rows` с его 10k упёрся бы уже на паре сотен таблиц.
///
/// Ровно 100k, а не больше: столько же брокер разрешает на запрос
/// (`max_rows.clamp(1, 100_000)`), а через него идёт MCP-tool `schema`. Возьми
/// константа больше — CLI и MCP обрезали бы дамп на разной глубине, и «та же
/// команда» отдавала бы разное.
pub(crate) const MAX_ROWS: usize = 100_000;

#[derive(Args)]
pub struct SchemaArgs {
    /// Профиль: имя, id или группа
    alias: String,
    /// Только эта схема (можно и системную, например pg_catalog)
    #[arg(long, value_name = "NAME")]
    schema: Option<String>,
    /// Показать всё: системные схемы, объекты расширений, листовые партиции
    #[arg(long)]
    internal: bool,
    /// Тела вьюх и исходники функций
    #[arg(long)]
    definitions: bool,
    /// Тексты COMMENT ON: таблиц, колонок, типов, функций
    #[arg(long)]
    comments: bool,
    /// JSON вместо текста (дерево схем, а не таблица — --csv/-t тут не бывает)
    #[arg(long)]
    json: bool,
    #[arg(long, value_name = "VAR")]
    password_env: Option<String>,
    #[arg(short, long)]
    verbose: bool,
}

/// Дерево дампа. `pub(crate)`, потому что тем же деревом отвечает MCP-tool
/// `schema` (см. `cmd::mcp`) — рендер и разбор каталога у них общие.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchemaDump {
    database: String,
    server_version: String,
    /// Каталог не влез в лимит строк, дерево ниже неполное. Для текстового
    /// вывода про это пишет вызывающий (в stderr, а MCP — припиской к тексту),
    /// а потребителю JSON сказать больше некому — без этого поля он молча
    /// примет обрезок за весь каталог.
    pub(crate) truncated: bool,
    schemas: Vec<SchemaInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaInfo {
    name: String,
    tables: Vec<RelationInfo>,
    views: Vec<RelationInfo>,
    materialized_views: Vec<RelationInfo>,
    foreign_tables: Vec<RelationInfo>,
    sequences: Vec<SequenceInfo>,
    enums: Vec<EnumInfo>,
    domains: Vec<DomainInfo>,
    composite_types: Vec<CompositeTypeInfo>,
    routines: Vec<RoutineInfo>,
}

impl SchemaInfo {
    fn new(name: &str) -> Self {
        SchemaInfo {
            name: name.to_string(),
            tables: Vec::new(),
            views: Vec::new(),
            materialized_views: Vec::new(),
            foreign_tables: Vec::new(),
            sequences: Vec::new(),
            enums: Vec::new(),
            domains: Vec::new(),
            composite_types: Vec::new(),
            routines: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.tables.is_empty()
            && self.views.is_empty()
            && self.materialized_views.is_empty()
            && self.foreign_tables.is_empty()
            && self.sequences.is_empty()
            && self.enums.is_empty()
            && self.domains.is_empty()
            && self.composite_types.is_empty()
            && self.routines.is_empty()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RelationInfo {
    schema: String,
    name: String,
    /// table | partitioned | view | matview | foreign
    kind: String,
    comment: Option<String>,
    partition_key: Option<String>,
    partition_of: Option<String>,
    /// `FOR VALUES FROM … TO …` / `DEFAULT` — за какой кусок данных отвечает
    /// эта партиция (есть только у листовых, т.е. под `--internal`).
    partition_bound: Option<String>,
    partitions: Option<i64>,
    row_security: bool,
    definition: Option<String>,
    columns: Vec<ColumnInfo>,
    constraints: Vec<ConstraintInfo>,
    indexes: Vec<IndexInfo>,
    triggers: Vec<TriggerInfo>,
    policies: Vec<PolicyInfo>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnInfo {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    nullable: bool,
    default: Option<String>,
    identity: Option<String>,
    generated: Option<String>,
    /// stored | virtual — у virtual (PG 18) значение не хранится и индекс по
    /// нему не построить, так что назвать её stored значит соврать о схеме.
    generated_kind: Option<String>,
    comment: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConstraintInfo {
    name: String,
    kind: String,
    definition: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexInfo {
    name: String,
    unique: bool,
    definition: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TriggerInfo {
    name: String,
    timing: String,
    events: String,
    function: String,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PolicyInfo {
    name: String,
    /// SELECT | INSERT | UPDATE | DELETE | ALL
    command: String,
    /// false — RESTRICTIVE: политика не добавляет доступ, а сужает общий
    permissive: bool,
    roles: String,
    using: Option<String>,
    with_check: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EnumInfo {
    schema: String,
    name: String,
    /// Пусто у `CREATE TYPE … AS ENUM ()` — легального шага миграции
    values: Vec<String>,
    comment: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SequenceInfo {
    schema: String,
    name: String,
    #[serde(rename = "type")]
    ty: String,
    start: String,
    increment: String,
    min: String,
    max: String,
    cycle: bool,
    comment: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DomainInfo {
    schema: String,
    name: String,
    base_type: String,
    /// `NOT NULL DEFAULT … CHECK (…)` одной строкой, как это печатает сервер
    constraints: Option<String>,
    comment: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompositeTypeInfo {
    schema: String,
    name: String,
    /// «поле тип, поле тип» в порядке объявления
    attributes: String,
    comment: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoutineInfo {
    schema: String,
    name: String,
    /// function | procedure
    kind: String,
    arguments: String,
    returns: Option<String>,
    language: String,
    volatility: String,
    definition: Option<String>,
    comment: Option<String>,
}

/// Значение ячейки как Option (в отличие от `db::cell`, который схлопывает
/// NULL в пустую строку — здесь разница между «нет default» и «default ''»).
fn cell_opt(row: &[Option<String>], i: usize) -> Option<String> {
    row.get(i).cloned().flatten()
}

pub async fn run(a: SchemaArgs) -> Result<ExitCode, AppError> {
    let opts = db::SchemaOptions {
        schema: a.schema.clone(),
        internal: a.internal,
        definitions: a.definitions,
        comments: a.comments,
    };
    let (profile, connected) = session::open_for(
        &a.alias,
        session::PwSource {
            env: a.password_env.as_deref(),
            ..Default::default()
        },
        false,
        a.verbose,
        true,
    )
    .await?;

    check_server_version(&connected.server_version)?;
    let sql = db::schema_dump_sql(&opts);
    let exec = db::execute(&connected.session.client, &sql, MAX_ROWS).await?;
    check_parts(&exec.results)?;

    let dump = build_dump(&profile.database, &connected.server_version, &exec.results);
    if dump.truncated {
        eprintln!("sql-kai: каталог обрезан на {MAX_ROWS} строк — сузь вывод через --schema");
    }
    if a.json {
        println!("{}", serde_json::to_string_pretty(&dump).unwrap());
    } else {
        print!("{}", render_text(&dump, &opts));
    }
    Ok(ExitCode::SUCCESS)
}

/// Минимальный сервер: дамп читает `pg_attribute.attgenerated`, которого нет
/// до PG 12. Батч атомарен — на старом сервере падает целиком, поэтому версию
/// проверяем до запроса, а не разбираем сырое `column a.attgenerated does not
/// exist`. Больше нигде минимальная версия не заявлена, так что заявляем тут.
pub(crate) const MIN_SERVER_MAJOR: u32 = 12;

/// Пустая или неразобранная версия — не повод отказывать: значит, её просто
/// некому было сообщить, и пусть решает сервер.
pub(crate) fn check_server_version(server_version: &str) -> Result<(), AppError> {
    let major: u32 = server_version
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    if major == 0 || major >= MIN_SERVER_MAJOR {
        return Ok(());
    }
    Err(AppError::Msg(format!(
        "схема снимается только с PostgreSQL {MIN_SERVER_MAJOR}+ (сервер: {server_version}); \
         на более старом бери структуру по частям: tables, columns, indexes, ddl"
    )))
}

/// Форма ответа каталога: `build_dump` разбирает наборы строк по позиции, так
/// что расхождение длины — это не «пустой дамп», а перекос разбора.
pub(crate) fn check_parts(results: &[db::StatementResult]) -> Result<(), AppError> {
    if results.len() == db::SCHEMA_DUMP_PARTS {
        return Ok(());
    }
    Err(AppError::Msg(format!(
        "каталог вернул {} наборов строк вместо {} — дамп неполный",
        results.len(),
        db::SCHEMA_DUMP_PARTS
    )))
}

/// Семь наборов строк из `db::schema_dump_sql` -> дерево схем. Колонки,
/// констрейнты, индексы и триггеры приезжают плоскими списками и подшиваются
/// к своему отношению по паре (схема, имя).
pub(crate) fn build_dump(
    database: &str,
    server_version: &str,
    res: &[db::StatementResult],
) -> SchemaDump {
    let mut rels: Vec<RelationInfo> = Vec::new();
    let mut at: BTreeMap<(String, String), usize> = BTreeMap::new();
    for row in &res[0].rows {
        let schema = db::cell(row, 0);
        let name = db::cell(row, 1);
        let kind = match db::cell(row, 2).as_str() {
            "p" => "partitioned",
            "v" => "view",
            "m" => "matview",
            "f" => "foreign",
            _ => "table",
        };
        at.insert((schema.clone(), name.clone()), rels.len());
        rels.push(RelationInfo {
            schema,
            name,
            kind: kind.to_string(),
            partition_key: cell_opt(row, 3),
            partition_of: cell_opt(row, 4),
            partition_bound: cell_opt(row, 5),
            partitions: cell_opt(row, 6).and_then(|v| v.parse().ok()),
            row_security: db::cell_bool(row, 7),
            comment: cell_opt(row, 8),
            definition: cell_opt(row, 9),
            columns: Vec::new(),
            constraints: Vec::new(),
            indexes: Vec::new(),
            triggers: Vec::new(),
            policies: Vec::new(),
        });
    }
    let owner = |row: &Vec<Option<String>>| at.get(&(db::cell(row, 0), db::cell(row, 1))).copied();

    for row in &res[1].rows {
        let Some(i) = owner(row) else { continue };
        // generated-колонка хранит выражение там же, где default; показываем
        // его один раз — как generated, иначе это выглядит как два разных факта.
        // 's' — stored, 'v' — virtual (PG 18); без 'v' виртуальная колонка
        // отрендерилась бы как обычный `default <выражение>`, то есть неверно.
        let expr = cell_opt(row, 5);
        let generated_kind = match db::cell(row, 7).as_str() {
            "s" => Some("stored".to_string()),
            "v" => Some("virtual".to_string()),
            _ => None,
        };
        let generated = generated_kind
            .is_some()
            .then(|| expr.clone().unwrap_or_default());
        rels[i].columns.push(ColumnInfo {
            name: db::cell(row, 2),
            ty: db::cell(row, 3),
            nullable: !db::cell_bool(row, 4),
            default: if generated.is_some() { None } else { expr },
            identity: match db::cell(row, 6).as_str() {
                "a" => Some("always".to_string()),
                "d" => Some("by default".to_string()),
                _ => None,
            },
            generated,
            generated_kind,
            comment: cell_opt(row, 8),
        });
    }

    for row in &res[2].rows {
        let Some(i) = owner(row) else { continue };
        rels[i].constraints.push(ConstraintInfo {
            name: db::cell(row, 2),
            kind: match db::cell(row, 3).as_str() {
                "p" => "primary key",
                "u" => "unique",
                "f" => "foreign key",
                "x" => "exclude",
                _ => "check",
            }
            .to_string(),
            definition: db::cell(row, 4),
        });
    }

    for row in &res[3].rows {
        let Some(i) = owner(row) else { continue };
        rels[i].indexes.push(IndexInfo {
            name: db::cell(row, 2),
            unique: db::cell_bool(row, 3),
            definition: db::cell(row, 4),
        });
    }

    for row in &res[4].rows {
        let Some(i) = owner(row) else { continue };
        rels[i].triggers.push(TriggerInfo {
            name: db::cell(row, 2),
            timing: db::cell(row, 3),
            events: db::cell(row, 4),
            function: db::cell(row, 5),
            enabled: db::cell_bool(row, 6),
        });
    }

    // одна строка на метку, отсортированы по enumsortorder — склеиваем подряд
    // идущие. Метка приходит NULL ровно у enum'а без меток (LEFT JOIN), и
    // тогда тип есть, а список значений пуст — это не одна пустая метка.
    let mut enums: Vec<EnumInfo> = Vec::new();
    for row in &res[5].rows {
        let schema = db::cell(row, 0);
        let name = db::cell(row, 1);
        let label = cell_opt(row, 2);
        match enums.last_mut() {
            Some(e) if e.schema == schema && e.name == name => e.values.extend(label),
            _ => enums.push(EnumInfo {
                schema,
                name,
                values: label.into_iter().collect(),
                comment: cell_opt(row, 3),
            }),
        }
    }

    for row in &res[7].rows {
        let Some(i) = owner(row) else { continue };
        rels[i].policies.push(PolicyInfo {
            name: db::cell(row, 2),
            command: db::cell(row, 3),
            permissive: db::cell_bool(row, 4),
            roles: db::cell(row, 5),
            using: cell_opt(row, 6),
            with_check: cell_opt(row, 7),
        });
    }

    let sequences: Vec<SequenceInfo> = res[8]
        .rows
        .iter()
        .map(|row| SequenceInfo {
            schema: db::cell(row, 0),
            name: db::cell(row, 1),
            ty: db::cell(row, 2),
            start: db::cell(row, 3),
            increment: db::cell(row, 4),
            min: db::cell(row, 5),
            max: db::cell(row, 6),
            cycle: db::cell_bool(row, 7),
            comment: cell_opt(row, 8),
        })
        .collect();

    // Домены и composite-типы приезжают одним набором, различаются typtype.
    let mut domains: Vec<DomainInfo> = Vec::new();
    let mut composites: Vec<CompositeTypeInfo> = Vec::new();
    for row in &res[9].rows {
        if db::cell(row, 2) == "d" {
            domains.push(DomainInfo {
                schema: db::cell(row, 0),
                name: db::cell(row, 1),
                base_type: db::cell(row, 3),
                constraints: cell_opt(row, 4).filter(|s| !s.is_empty()),
                comment: cell_opt(row, 5),
            });
        } else {
            composites.push(CompositeTypeInfo {
                schema: db::cell(row, 0),
                name: db::cell(row, 1),
                attributes: db::cell(row, 4),
                comment: cell_opt(row, 5),
            });
        }
    }

    let routines: Vec<RoutineInfo> = res[6]
        .rows
        .iter()
        .map(|row| RoutineInfo {
            schema: db::cell(row, 0),
            name: db::cell(row, 1),
            kind: if db::cell(row, 2) == "p" { "procedure" } else { "function" }.to_string(),
            arguments: db::cell(row, 3),
            returns: cell_opt(row, 4),
            language: db::cell(row, 5),
            volatility: db::cell(row, 6),
            definition: cell_opt(row, 7),
            comment: cell_opt(row, 8),
        })
        .collect();

    let mut by_schema: BTreeMap<String, SchemaInfo> = BTreeMap::new();
    for r in rels {
        let key = r.schema.clone();
        let s = by_schema.entry(key).or_insert_with(|| SchemaInfo::new(&r.schema));
        match r.kind.as_str() {
            "view" => s.views.push(r),
            "matview" => s.materialized_views.push(r),
            "foreign" => s.foreign_tables.push(r),
            _ => s.tables.push(r),
        }
    }
    for s in sequences {
        let key = s.schema.clone();
        by_schema.entry(key).or_insert_with(|| SchemaInfo::new(&s.schema)).sequences.push(s);
    }
    for e in enums {
        let key = e.schema.clone();
        by_schema.entry(key).or_insert_with(|| SchemaInfo::new(&e.schema)).enums.push(e);
    }
    for d in domains {
        let key = d.schema.clone();
        by_schema.entry(key).or_insert_with(|| SchemaInfo::new(&d.schema)).domains.push(d);
    }
    for c in composites {
        let key = c.schema.clone();
        by_schema
            .entry(key)
            .or_insert_with(|| SchemaInfo::new(&c.schema))
            .composite_types
            .push(c);
    }
    for r in routines {
        let key = r.schema.clone();
        by_schema.entry(key).or_insert_with(|| SchemaInfo::new(&r.schema)).routines.push(r);
    }

    SchemaDump {
        database: database.to_string(),
        server_version: server_version.to_string(),
        // Обрезку считаем здесь, а не у вызывающего: так признак попадает и в
        // JSON CLI, и в structuredContent MCP-тула, который зовёт тот же
        // build_dump.
        truncated: res.iter().any(|r| r.truncated),
        schemas: by_schema.into_values().collect(),
    }
}

/// Однострочные комментарии: переносы ломают построчный формат.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Многострочное тело (viewdef / functiondef) с отступом под элемент.
fn push_block(out: &mut String, label: &str, body: &str) {
    out.push_str(&format!("  {label}:\n"));
    for line in body.trim_end().lines() {
        out.push_str(&format!("    {}\n", line.trim_end()));
    }
}

/// Границы, которые Postgres проставляет секвенции сам (`CREATE SEQUENCE` без
/// MINVALUE/MAXVALUE): у убывающей это [тип_min, -1], у возрастающей —
/// [1, тип_max]. Сравнением с ними отличаем заданное руками от умолчания.
fn default_bounds(ty: &str, increment: &str) -> (String, String) {
    let (lo, hi): (i64, i64) = match ty {
        "smallint" => (i16::MIN as i64, i16::MAX as i64),
        "integer" => (i32::MIN as i64, i32::MAX as i64),
        _ => (i64::MIN, i64::MAX),
    };
    if increment.starts_with('-') {
        (lo.to_string(), "-1".to_string())
    } else {
        ("1".to_string(), hi.to_string())
    }
}

/// `pg_get_indexdef` отдаёт полный CREATE INDEX, но имя индекса и таблица уже
/// напечатаны рядом — оставляем только «USING …».
fn index_tail(def: &str) -> &str {
    match def.find(" USING ") {
        Some(i) => def[i + 1..].trim(),
        None => def.trim(),
    }
}

/// Текстовый дамп. Опции берутся из `db::SchemaOptions`, а не из `SchemaArgs`,
/// чтобы тем же рендером отвечал MCP-tool `schema` (флаги там — поля JSON).
pub(crate) fn render_text(dump: &SchemaDump, o: &db::SchemaOptions) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "-- база: {}   сервер: {}\n",
        dump.database, dump.server_version
    ));
    if let Some(s) = &o.schema {
        out.push_str(&format!("-- только схема: {s}\n"));
    }
    let mut hidden: Vec<&str> = Vec::new();
    if !o.internal {
        hidden.push("системные схемы, объекты расширений, партиции (--internal)");
    }
    if !o.definitions {
        hidden.push("тела вьюх и функций (--definitions)");
    }
    if !o.comments {
        hidden.push("комментарии (--comments)");
    }
    // Права роли — не флаг, отключить их нельзя, но и умолчать нельзя: иначе
    // урезанный правами дамп читается как полный. Поэтому клауза безусловна, и
    // список пустым уже не бывает.
    hidden.push("объекты, на которые нет прав у роли подключения");
    out.push_str(&format!("-- скрыто: {}\n", hidden.join("; ")));

    if dump.schemas.iter().all(SchemaInfo::is_empty) {
        out.push_str("-- ничего не найдено (проверь --schema / --internal и права роли)\n");
        return out;
    }

    for s in &dump.schemas {
        if s.is_empty() {
            continue;
        }
        out.push_str(&format!("\n== схема {} ==\n", s.name));
        for r in s
            .tables
            .iter()
            .chain(&s.views)
            .chain(&s.materialized_views)
            .chain(&s.foreign_tables)
        {
            render_relation(&mut out, r);
        }
        for q in &s.sequences {
            let mut line = format!(
                "\nsequence {}.{} {}  start {} increment {}",
                q.schema, q.name, q.ty, q.start, q.increment
            );
            // min/max печатаем, только если они не те, что сервер ставит сам:
            // дефолтные границы — это шум на каждую секвенцию.
            let (min, max) = default_bounds(&q.ty, &q.increment);
            if q.min != min {
                line.push_str(&format!(" min {}", q.min));
            }
            if q.max != max {
                line.push_str(&format!(" max {}", q.max));
            }
            if q.cycle {
                line.push_str(" cycle");
            }
            out.push_str(&line);
            out.push('\n');
            if let Some(c) = &q.comment {
                out.push_str(&format!("  -- {}\n", one_line(c)));
            }
        }
        for e in &s.enums {
            let values = if e.values.is_empty() {
                "(без меток)".to_string()
            } else {
                e.values.join(" | ")
            };
            out.push_str(&format!("\nenum {}.{} = {}\n", e.schema, e.name, values));
            if let Some(c) = &e.comment {
                out.push_str(&format!("  -- {}\n", one_line(c)));
            }
        }
        for d in &s.domains {
            out.push_str(&format!("\ndomain {}.{} AS {}", d.schema, d.name, d.base_type));
            if let Some(c) = &d.constraints {
                out.push_str(&format!(" {c}"));
            }
            out.push('\n');
            if let Some(c) = &d.comment {
                out.push_str(&format!("  -- {}\n", one_line(c)));
            }
        }
        for t in &s.composite_types {
            out.push_str(&format!(
                "\ntype {}.{} = ({})\n",
                t.schema, t.name, t.attributes
            ));
            if let Some(c) = &t.comment {
                out.push_str(&format!("  -- {}\n", one_line(c)));
            }
        }
        for r in &s.routines {
            render_routine(&mut out, r);
        }
    }
    out.push_str(&format!("\n{}\n", summary(dump)));
    out
}

fn render_relation(out: &mut String, r: &RelationInfo) {
    let head = match r.kind.as_str() {
        "partitioned" => "partitioned table",
        "view" => "view",
        "matview" => "materialized view",
        "foreign" => "foreign table",
        _ => "table",
    };
    let mut line = format!("\n{head} {}.{}", r.schema, r.name);
    if let Some(k) = &r.partition_key {
        line.push_str(&format!(" PARTITION BY {k}"));
    }
    if let Some(p) = &r.partition_of {
        line.push_str(&format!(" PARTITION OF {p}"));
    }
    if let Some(b) = &r.partition_bound {
        line.push_str(&format!(" {b}"));
    }
    if let Some(n) = r.partitions.filter(|n| *n > 0) {
        line.push_str(&format!("  (партиций: {n})"));
    }
    out.push_str(&line);
    out.push('\n');
    if let Some(c) = &r.comment {
        out.push_str(&format!("  -- {}\n", one_line(c)));
    }

    let w_name = r.columns.iter().map(|c| c.name.chars().count()).max().unwrap_or(0);
    let w_type = r.columns.iter().map(|c| c.ty.chars().count()).max().unwrap_or(0);
    for c in &r.columns {
        let mut tail: Vec<String> = Vec::new();
        if !c.nullable {
            tail.push("not null".to_string());
        }
        if let Some(g) = &c.generated {
            let kind = c.generated_kind.as_deref().unwrap_or("stored");
            tail.push(format!("generated ({g}) {kind}"));
        } else if let Some(i) = &c.identity {
            tail.push(format!("identity {i}"));
        } else if let Some(d) = &c.default {
            tail.push(format!("default {d}"));
        }
        if let Some(cm) = &c.comment {
            tail.push(format!("-- {}", one_line(cm)));
        }
        let line = format!(
            "  {:<w_name$}  {:<w_type$}  {}",
            c.name,
            c.ty,
            tail.join("  ")
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }

    for c in &r.constraints {
        out.push_str(&format!("  constraint {} {}\n", c.name, c.definition));
    }
    for i in &r.indexes {
        out.push_str(&format!(
            "  index {}{} {}\n",
            i.name,
            if i.unique { " unique" } else { "" },
            index_tail(&i.definition)
        ));
    }
    for t in &r.triggers {
        out.push_str(&format!(
            "  trigger {} {} {} -> {}(){}\n",
            t.name,
            t.timing,
            t.events,
            t.function,
            if t.enabled { "" } else { " [disabled]" }
        ));
    }
    // Одного «RLS включён» мало: кто и что видит, написано только в политиках,
    // а без них агент возвращается к обходу таблиц по одной. Выключенный RLS
    // при живых политиках — тоже факт: они лежат, но ничего не фильтруют.
    if r.row_security {
        out.push_str("  row level security enabled\n");
    } else if !r.policies.is_empty() {
        out.push_str("  row level security disabled — политики ниже не действуют\n");
    }
    for p in &r.policies {
        let mut line = format!("policy {}", p.name);
        if !p.permissive {
            line.push_str(" restrictive");
        }
        line.push_str(&format!(" FOR {} TO {}", p.command, p.roles));
        if let Some(u) = &p.using {
            line.push_str(&format!(" USING {u}"));
        }
        if let Some(w) = &p.with_check {
            line.push_str(&format!(" WITH CHECK {w}"));
        }
        // Выражение политики бывает многострочным — но строка тут одна.
        out.push_str(&format!("  {}\n", one_line(&line)));
    }
    if let Some(d) = &r.definition {
        push_block(out, "definition", d);
    }
}

fn render_routine(out: &mut String, r: &RoutineInfo) {
    let mut line = format!("\n{} {}.{}({})", r.kind, r.schema, r.name, r.arguments);
    if let Some(ret) = &r.returns {
        line.push_str(&format!(" -> {ret}"));
    }
    line.push_str(&format!("  [{} {}]\n", r.language, r.volatility));
    out.push_str(&line);
    if let Some(c) = &r.comment {
        out.push_str(&format!("  -- {}\n", one_line(c)));
    }
    if let Some(d) = &r.definition {
        push_block(out, "definition", d);
    }
}

/// Итоговая строка: только непустые категории, чтобы не плодить нули.
fn summary(dump: &SchemaDump) -> String {
    let mut counts: Vec<(&str, usize)> = vec![
        ("таблиц", dump.schemas.iter().map(|s| s.tables.len()).sum()),
        ("вьюх", dump.schemas.iter().map(|s| s.views.len()).sum()),
        (
            "matview",
            dump.schemas.iter().map(|s| s.materialized_views.len()).sum(),
        ),
        (
            "foreign",
            dump.schemas.iter().map(|s| s.foreign_tables.len()).sum(),
        ),
        (
            "секвенций",
            dump.schemas.iter().map(|s| s.sequences.len()).sum(),
        ),
        ("enum", dump.schemas.iter().map(|s| s.enums.len()).sum()),
        ("доменов", dump.schemas.iter().map(|s| s.domains.len()).sum()),
        (
            "composite-типов",
            dump.schemas.iter().map(|s| s.composite_types.len()).sum(),
        ),
        ("функций", dump.schemas.iter().map(|s| s.routines.len()).sum()),
    ];
    counts.retain(|(_, n)| *n > 0);
    let body = counts
        .iter()
        .map(|(label, n)| format!("{label} {n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("-- итого: схем {}, {body}", dump.schemas.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_tail_keeps_only_using_part() {
        assert_eq!(
            index_tail("CREATE UNIQUE INDEX u ON public.t USING btree (a, b)"),
            "USING btree (a, b)"
        );
        // нестандартный def (например, из будущей версии PG) остаётся целиком
        assert_eq!(index_tail("something else"), "something else");
    }

    #[test]
    fn one_line_squashes_whitespace() {
        assert_eq!(one_line("две\n  строки\tтут"), "две строки тут");
    }

    /// Батч обязан оставаться ровно из SCHEMA_DUMP_PARTS стейтментов: build_dump
    /// разбирает наборы строк по позиции.
    #[test]
    fn dump_sql_keeps_statement_count() {
        for internal in [false, true] {
            let o = db::SchemaOptions {
                internal,
                definitions: internal,
                comments: internal,
                ..Default::default()
            };
            let sql = db::schema_dump_sql(&o);
            assert_eq!(db::split_statements(&sql).len(), db::SCHEMA_DUMP_PARTS);
        }
    }

    fn part(rows: &[&[Option<&str>]]) -> db::StatementResult {
        db::StatementResult {
            columns: Vec::new(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| c.map(str::to_string)).collect())
                .collect(),
            rows_affected: None,
            truncated: false,
        }
    }

    /// Синтетический ответ каталога -> текстовый рендер: проверяем и разбор по
    /// позициям колонок, и сам формат (живой базы в тестах нет).
    #[test]
    fn renders_relations_columns_and_routines() {
        let res = vec![
            part(&[
                &[
                    Some("public"), Some("users"), Some("r"), None, None, None, None,
                    Some("true"), Some("люди"), None,
                ],
                &[
                    Some("public"), Some("v_active"), Some("v"), None, None, None, None,
                    Some("false"), None, Some("SELECT id\n  FROM users"),
                ],
                &[
                    Some("public"), Some("ev_2024"), Some("r"), None, Some("public.ev"),
                    Some("FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"), None,
                    Some("false"), None, None,
                ],
                &[
                    Some("public"), Some("gen"), Some("r"), None, None, None, None,
                    Some("false"), None, None,
                ],
            ]),
            part(&[
                &[
                    Some("public"), Some("users"), Some("id"), Some("bigint"), Some("true"),
                    Some("nextval('users_id_seq'::regclass)"), Some(""), Some(""), None,
                ],
                &[
                    Some("public"), Some("users"), Some("email"), Some("text"), Some("true"),
                    None, Some(""), Some(""), Some("почта\nсотрудника"),
                ],
                &[
                    Some("public"), Some("users"), Some("org_id"), Some("bigint"), Some("false"),
                    None, Some(""), Some(""), None,
                ],
                // stored и virtual (PG 18) — разные вещи, рендер обязан их различать
                &[
                    Some("public"), Some("gen"), Some("lo"), Some("text"), Some("false"),
                    Some("lower(email)"), Some(""), Some("s"), None,
                ],
                &[
                    Some("public"), Some("gen"), Some("len"), Some("integer"), Some("false"),
                    Some("length(email)"), Some(""), Some("v"), None,
                ],
            ]),
            part(&[&[
                Some("public"), Some("users"), Some("users_pkey"), Some("p"),
                Some("PRIMARY KEY (id)"),
            ]]),
            part(&[&[
                Some("public"), Some("users"), Some("users_email_idx"), Some("true"),
                Some("CREATE UNIQUE INDEX users_email_idx ON public.users USING btree (email)"),
            ]]),
            part(&[&[
                Some("public"), Some("users"), Some("t_upd"), Some("BEFORE"), Some("UPDATE"),
                Some("public.set_updated_at"), Some("true"),
            ]]),
            part(&[
                &[Some("public"), Some("status"), Some("new"), None],
                &[Some("public"), Some("status"), Some("done"), None],
                // enum без меток: LEFT JOIN отдаёт одну строку с NULL-меткой
                &[Some("public"), Some("stage"), None, None],
            ]),
            part(&[&[
                Some("public"), Some("calc"), Some("f"), Some("a integer"), Some("integer"),
                Some("sql"), Some("immutable"), None, None,
            ]]),
            part(&[
                &[
                    Some("public"), Some("users"), Some("users_sel"), Some("SELECT"), Some("true"),
                    Some("public"), Some("(org_id = current_org())"), None,
                ],
                &[
                    Some("public"), Some("users"), Some("users_ins"), Some("INSERT"), Some("false"),
                    Some("app"), None, Some("(org_id IS NOT NULL)"),
                ],
            ]),
            part(&[
                &[
                    Some("public"), Some("invoice_seq"), Some("bigint"), Some("100"), Some("5"),
                    Some("10"), Some("900"), Some("true"), Some("счётчик"),
                ],
                &[
                    Some("public"), Some("plain_seq"), Some("bigint"), Some("1"), Some("1"),
                    Some("1"), Some("9223372036854775807"), Some("false"), None,
                ],
            ]),
            part(&[
                &[
                    Some("public"), Some("email"), Some("d"), Some("text"),
                    Some("NOT NULL CHECK (VALUE ~ '@'::text)"), Some("адрес почты"),
                ],
                &[
                    Some("public"), Some("addr"), Some("c"), None,
                    Some("street text, city text"), None,
                ],
            ]),
        ];
        let dump = build_dump("mydb", "16.2", &res);
        let o = db::SchemaOptions {
            definitions: true,
            comments: true,
            ..Default::default()
        };
        let text = render_text(&dump, &o);

        assert!(text.contains("== схема public =="));
        assert!(text.contains("table public.users"));
        assert!(text.contains("  -- люди"));
        assert!(text.contains("id      bigint  not null  default nextval('users_id_seq'::regclass)"));
        // многострочный COMMENT ON схлопывается в одну строку колонки
        assert!(text.contains("email   text    not null  -- почта сотрудника"));
        // nullable-колонка не тащит за собой хвост из пробелов
        assert!(text.contains("\n  org_id  bigint\n"));
        assert!(text.contains("  constraint users_pkey PRIMARY KEY (id)"));
        assert!(text.contains("  index users_email_idx unique USING btree (email)"));
        assert!(text.contains("  trigger t_upd BEFORE UPDATE -> public.set_updated_at()"));
        assert!(text.contains("view public.v_active"));
        assert!(text.contains("  definition:\n    SELECT id\n      FROM users"));
        assert!(text.contains("enum public.status = new | done"));
        assert!(text.contains("function public.calc(a integer) -> integer  [sql immutable]"));
        // границы партиции: без них не видно, какой кусок данных в ней лежит
        assert!(text.contains(
            "table public.ev_2024 PARTITION OF public.ev \
             FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"
        ));
        // virtual-колонку нельзя печатать как stored — это разные таблицы
        assert!(text.contains("lo   text     generated (lower(email)) stored"));
        assert!(text.contains("len  integer  generated (length(email)) virtual"));
        // RLS: сам факт бесполезен без политик
        assert!(text.contains("  row level security enabled\n"));
        assert!(text.contains("  policy users_sel FOR SELECT TO public USING (org_id = current_org())\n"));
        assert!(text.contains(
            "  policy users_ins restrictive FOR INSERT TO app WITH CHECK (org_id IS NOT NULL)\n"
        ));
        // enum без меток: тип есть, значений нет — и это видно
        assert!(text.contains("enum public.stage = (без меток)\n"));
        // границы секвенции печатаются, только если заданы руками
        assert!(text.contains("sequence public.invoice_seq bigint  start 100 increment 5 min 10 max 900 cycle\n"));
        assert!(text.contains("sequence public.plain_seq bigint  start 1 increment 1\n"));
        assert!(text.contains("domain public.email AS text NOT NULL CHECK (VALUE ~ '@'::text)\n"));
        assert!(text.contains("type public.addr = (street text, city text)\n"));
        assert!(text.contains(
            "-- итого: схем 1, таблиц 3, вьюх 1, секвенций 2, enum 2, доменов 1, \
             composite-типов 1, функций 1"
        ));
        assert!(!dump.truncated);
    }

    /// Обрезка каталога — часть ответа, а не только предупреждение в stderr:
    /// потребитель JSON иначе примет неполный дамп за полный.
    #[test]
    fn truncated_result_marks_the_dump() {
        let mut res: Vec<db::StatementResult> =
            (0..db::SCHEMA_DUMP_PARTS).map(|_| part(&[])).collect();
        res[1].truncated = true;
        assert!(build_dump("mydb", "16.2", &res).truncated);
    }

    /// Индекс за FK-констрейнтом (в т.ч. самоссылающимся) — не «индекс
    /// констрейнта»: свой индекс есть только у p/u/x.
    #[test]
    fn dump_sql_skips_only_constraint_owned_indexes() {
        let sql = db::schema_dump_sql(&db::SchemaOptions::default());
        assert!(sql.contains("co.contype IN ('p','u','x')"));
        assert!(!sql.contains("co.conrelid = i.indrelid"));
    }

    /// enum без меток должен доезжать до дампа — значит LEFT JOIN.
    #[test]
    fn dump_sql_keeps_enums_without_labels() {
        let sql = db::schema_dump_sql(&db::SchemaOptions::default());
        assert!(sql.contains("LEFT JOIN pg_enum e ON e.enumtypid = t.oid"));
    }

    /// Версия сервера: батч атомарен, поэтому на PG < 12 честнее отказать до
    /// запроса, чем отдать сырое «column a.attgenerated does not exist».
    #[test]
    fn old_server_is_refused_with_a_readable_message() {
        assert!(check_server_version("16.2").is_ok());
        assert!(check_server_version("12.0 (Debian)").is_ok());
        // версию сообщить некому (брокер её не отдаёт) — не мешаем
        assert!(check_server_version("").is_ok());
        let err = check_server_version("11.5").unwrap_err().to_string();
        assert!(err.contains("PostgreSQL 12+"), "{err}");
        assert!(check_server_version("9.6.24").is_err());
    }

    /// Имя схемы приходит от пользователя — только через quote_literal.
    #[test]
    fn dump_sql_escapes_schema_name() {
        let o = db::SchemaOptions {
            schema: Some("pub'lic".into()),
            ..Default::default()
        };
        let sql = db::schema_dump_sql(&o);
        assert!(sql.contains("n.nspname = 'pub''lic'"));
        assert!(!sql.contains("'pub'lic'"));
    }
}
