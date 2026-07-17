---
name: sql-kai
description: Выполнить SQL в PostgreSQL через CLI `sql-kai` из sql-kai — профили с ssh-туннелями, vault с паролями, история запросов, авто-дискавери postgres-контейнера по ssh. Использовать когда пользователь просит выполнить SQL или заглянуть в базу на удалённом сервере/проде, упоминает sql-kai или sql-kai, хочет завести профиль подключения к Postgres за ssh, посмотреть таблицы/колонки/DDL/индексы удалённой базы или проверить здоровье сохранённых подключений.
compatibility: Требует установленный CLI sql-kai (симлинк из бандла sql-kai.app или cargo install из этого репозитория) и ssh-доступ к хостам для туннелей/discover
---

# sql-kai — SQL в Postgres по профилям sql-kai

`sql-kai` — CLI десктопного клиента sql-kai: профили подключений, vault с паролями
и история запросов общие с GUI. Если GUI запущен, запрос выполняется через его
сессию (брокер); без GUI первый запрос сам поднимает фоновый holder, и сессия
с ssh-туннелем живут между вызовами (серия запросов не платит за connect).
Одиночный вызов без фоновых процессов: `--local` (одноразовая сессия мимо
GUI/holder) или `--no-mux` (то же + свежий ssh без ControlMaster);
`sql-kai holder stop` гасит держатель вместе с его сессиями.

Установка бинаря:

```bash
# из бандла приложения (обновляется вместе с ним):
ln -sf /Applications/sql-kai.app/Contents/MacOS/sql-kai-cli ~/.local/bin/sql-kai
# или из исходников (cargo-таргет называется sql-kai-cli, не sql-kai):
cargo install --path src-tauri --features cli --bin sql-kai-cli
```

Нюанс: `cargo install` кладёт бинарь `~/.cargo/bin/sql-kai-cli` и симлинк `sql-kai`,
указывающий в бандл приложения, НЕ обновляет — команда `sql-kai` обновляется только
вместе с приложением (релиз + перезапуск, дальше апдейтер сам).

## Быстрый старт

```bash
sql-kai <alias> -c "SELECT ..."          # alias = имя профиля, id или группа
echo "SELECT now()" | sql-kai <alias>    # SQL со stdin; или -f query.sql
sql-kai <alias> -c "..." --json          # структурный вывод, значения типизированы
sql-kai <alias> -c "..." -t              # tuples-only (значения через |); есть и --csv
sql-kai profiles list [фильтр]           # профили; фильтруй по имени сервиса: profiles list vuln
```

**Профиль ищи с фильтром, а не полным списком**: `sql-kai profiles list <сервис>` —
подстрока без учёта регистра по name/group/host/db/ssh. Профиль сервиса обычно совпадает
с ним по имени, полный список на десятки строк тянуть в контекст не нужно. Фильтр не
нашёл ничего — тогда уже смотри полный `profiles list`.

- **Сессия по умолчанию read-only** (`SET default_transaction_read_only = on`).
  Запись/DDL требуют `--write`.
- **Чувствительные колонки маскируются автоматически**: `password*`, `*secret*`,
  `*_token`, `*_key`, `passphrase`, `credential*`, `salt`, `jwt` → `[redacted]`
  (+ предупреждение в stderr). Реально нужно значение — `--no-redact`, но не
  выводи креды в чат без необходимости.
- Для парсинга ответа предпочитай `--json`: значения типизированы по реальным
  типам колонок (`id bigint` — число, `'3'::text` — строка). Нужна точная
  строковая форма числа — кастуй в `::text`.
- Несколько стейтментов (`-c` повторно или через `;`) — одна неявная
  транзакция: ошибка в середине откатывает всё.
- Результат капится `--max-rows` (по умолчанию 1000, будет пометка truncated).

## Новый хост: discover

Если профиля ещё нет — завести одной командой (`alias` тут ssh-хост из
`~/.ssh/config`):

```bash
sql-kai discover <ssh-alias>
```

Дискавери найдёт postgres-контейнер на хосте, достанет user/db/пароль из env
контейнера, сохранит профиль (ssh-туннель + пароль в vault) и проверит
подключение. Профиль сразу виден и в GUI.

Если postgres-контейнеров на хосте несколько, discover берёт первый и печатает
предупреждение со списком; нужный выбирается явно — и тогда обязательно давай
профилю своё имя, иначе upsert по имени перезапишет профиль `<ssh-alias>`:

```bash
sql-kai discover <ssh-alias> --container <имя-контейнера> --name <alias>-app
```

Тот же флаг есть у `sql-kai exec` (`--container`).

## Vault

Чтобы CLI читал пароли без вопросов, один раз (интерактивно):

```bash
sql-kai vault trust      # копия ключа в login keychain; отзыв: sql-kai vault revoke
sql-kai vault status
```

Обход vault: `--password-env VAR` (пароль БД из env) или `KAI_VAULT_PASSWORD`
(мастер-пароль из env). Ротация и хранение паролей во внешнем сторе — команды
`sql-kai rotate` / `--from-sec` — требуют отдельного CLI `sec` в PATH.

## Fallback: exec-режим

Если до базы не достучаться туннелем (порт не публикуется, IP контейнера
недоступен) или профиль не нужен — прямой режим без пароля:

```bash
sql-kai exec <ssh-alias> -c "SELECT 1"   # ssh -> docker exec psql; тоже read-only, --write для записи
sql-kai exec <ssh-alias> --dry-run       # показать команду, не заходя на хост
```

Вывод — сырой текст psql (`-t`, `--csv` поддерживаются), маскирования колонок
здесь нет: перечисляй колонки явно вместо `SELECT *`.

## Интроспекция и здоровье

```bash
sql-kai tables <alias> [--counts]        # таблицы/вьюхи (+ примерное число строк)
sql-kai columns <alias> [schema.]table
sql-kai ddl <alias> [schema.]table       # CREATE TABLE / VIEW
sql-kai indexes <alias> [schema.]table
sql-kai history [-n 20] [<alias>]        # история запросов (общая с GUI)
sql-kai saved list <alias> | sql-kai saved run <alias> <имя>
sql-kai doctor [<alias>]                 # сохранённые пароли ещё аутентифицируются?
sql-kai sessions                         # живые сессии GUI-брокера и holder'а
```

## Правила безопасности

- **Read-only — сразу.** `SELECT`/`EXPLAIN`/интроспекцию выполнять без вопросов;
  CLI и так не даст писать без `--write`.
- **`--write` — только с явного подтверждения пользователя** в текущей сессии.
  Перед изменением показать предварительный `SELECT` затрагиваемых строк;
  `UPDATE`/`DELETE` всегда с `WHERE`; DDL оформлять миграцией в репозитории
  сервиса, а не «вживую».
- Не выгребать секреты/ПДн без причины; на больших таблицах — `LIMIT`.

## Ссылки

- Полная документация CLI: `docs/sql-kai.html` в этом репозитории.
- Внешний managed-Postgres (не в docker на хосте) discover не найдёт — профиль
  для него заводится вручную в GUI.
