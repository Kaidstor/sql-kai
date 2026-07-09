---
name: kai
description: Выполнить SQL в PostgreSQL через CLI `kai` из sql-kai — профили с ssh-туннелями, vault с паролями, история запросов, авто-дискавери postgres-контейнера по ssh. Использовать когда пользователь просит выполнить SQL или заглянуть в базу на удалённом сервере/проде, упоминает kai или sql-kai, хочет завести профиль подключения к Postgres за ssh, посмотреть таблицы/колонки/DDL/индексы удалённой базы или проверить здоровье сохранённых подключений.
compatibility: Требует установленный CLI kai (симлинк из бандла sql-kai.app или cargo install из этого репозитория) и ssh-доступ к хостам для туннелей/discover
---

# kai — SQL в Postgres по профилям sql-kai

`kai` — CLI десктопного клиента sql-kai: профили подключений, vault с паролями
и история запросов общие с GUI. Если GUI запущен, запрос выполняется через его
сессию (брокер), иначе CLI сам поднимает ssh-туннель и подключается.

Установка бинаря:

```bash
# из бандла приложения (обновляется вместе с ним):
ln -sf /Applications/sql-kai.app/Contents/MacOS/kai ~/.local/bin/kai
# или из исходников:
cargo install --path src-tauri --features cli --bin kai
```

## Быстрый старт

```bash
kai <alias> -c "SELECT ..."          # alias = имя профиля, id или группа
echo "SELECT now()" | kai <alias>    # SQL со stdin; или -f query.sql
kai <alias> -c "..." --json          # структурный вывод, значения типизированы
kai <alias> -c "..." -t              # tuples-only (значения через |); есть и --csv
kai profiles list                    # какие профили уже есть
```

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
kai discover <ssh-alias>
```

Дискавери найдёт postgres-контейнер на хосте, достанет user/db/пароль из env
контейнера, сохранит профиль (ssh-туннель + пароль в vault) и проверит
подключение. Профиль сразу виден и в GUI.

## Vault

Чтобы CLI читал пароли без вопросов, один раз (интерактивно):

```bash
kai vault trust      # копия ключа в login keychain; отзыв: kai vault revoke
kai vault status
```

Обход vault: `--password-env VAR` (пароль БД из env) или `KAI_VAULT_PASSWORD`
(мастер-пароль из env). Ротация и хранение паролей во внешнем сторе — команды
`kai rotate` / `--from-sec` — требуют отдельного CLI `sec` в PATH.

## Fallback: exec-режим

Если до базы не достучаться туннелем (порт не публикуется, IP контейнера
недоступен) или профиль не нужен — прямой режим без пароля:

```bash
kai exec <ssh-alias> -c "SELECT 1"   # ssh -> docker exec psql; тоже read-only, --write для записи
kai exec <ssh-alias> --dry-run       # показать команду, не заходя на хост
```

Вывод — сырой текст psql (`-t`, `--csv` поддерживаются), маскирования колонок
здесь нет: перечисляй колонки явно вместо `SELECT *`.

## Интроспекция и здоровье

```bash
kai tables <alias> [--counts]        # таблицы/вьюхи (+ примерное число строк)
kai columns <alias> [schema.]table
kai ddl <alias> [schema.]table       # CREATE TABLE / VIEW
kai indexes <alias> [schema.]table
kai history [-n 20] [<alias>]        # история запросов (общая с GUI)
kai saved list <alias> | kai saved run <alias> <имя>
kai doctor [<alias>]                 # сохранённые пароли ещё аутентифицируются?
kai sessions                         # живые сессии GUI и брокера
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

- Полная документация CLI: `docs/kai.html` в этом репозитории.
- Внешний managed-Postgres (не в docker на хосте) discover не найдёт — профиль
  для него заводится вручную в GUI.
