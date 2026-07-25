---
name: sql-kai
description: Выполнить SQL и DDL в PostgreSQL через CLI `sql-kai` — профили с ssh-туннелями, vault с паролями, история запросов, дискавери postgres-контейнера по ssh, вся схема базы одним вызовом (`sql-kai schema`), журнал сервера (`sql-kai logs`), копия базы под миграции (`sql-kai fork`), ротация паролей ролей и барьер на запись в production. Использовать всегда, когда нужен запрос или изменение в Postgres — посмотреть данные или структуру базы, применить ALTER TABLE или .sql-файл, завести профиль к базе сервиса за ssh, проверить здоровье сохранённых подключений, посмотреть логи упавшего postgres, — в том числе когда это подзадача (проверить схему при отладке, применить миграцию перед деплоем), а не прямая просьба. Не запускать psql руками через `ssh <host> docker exec` — вместо этого sql-kai (профиль или exec-режим).
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

`sql-kai init` делает то же самое интерактивно (симлинк в PATH, `vault trust`,
`mcp install`, автодополнение) — предлагай её человеку, но сам не запускай:
без TTY она ничего не меняет, только печатает список ручных команд.

## Быстрый старт

```bash
sql-kai <alias> -c "SELECT ..."          # alias = имя профиля, id или группа
echo "SELECT now()" | sql-kai <alias>    # SQL со stdin; или -f query.sql
sql-kai <alias> -c "..." --json          # структурный вывод, значения типизированы
sql-kai <alias> -c "..." -t              # tuples-only (значения через |); есть и --csv
sql-kai schema <alias>                   # вся структура базы одним вызовом
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

Обход vault: `--password-env VAR` (пароль БД из env) или `SQL_KAI_VAULT_PASSWORD`
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

## Интроспекция: начинай со `schema`

```bash
sql-kai schema <alias>                   # ВСЯ структура базы одним вызовом
sql-kai schema <alias> --schema billing  # только одна схема
sql-kai schema <alias> --json            # то же деревом, для парсинга
```

`schema` отдаёт таблицы (в т.ч. партиционированные и foreign), вьюхи и матвьюхи
с колонками, типами, nullability, дефолтами, identity/generated, констрейнтами,
индексами, триггерами и RLS, плюс enum-типы, функции и процедуры — за один поход
в каталог и из одного снимка. **Не обходи базу циклом `tables` →
`columns`/`ddl`/`indexes` по каждой таблице**: это десятки round-trip, рваная
картина и заметно больше токенов на тот же результат.

```
-- база: domainator   сервер: 16.4
-- скрыто: системные схемы, объекты расширений, партиции (--internal); тела вьюх и функций (--definitions); комментарии (--comments)

== схема public ==

table public.domains
  id          bigint        not null  identity always
  name        text          not null
  status      domain_state  not null  default 'new'::domain_state
  created_at  timestamptz   not null  default now()
  constraint domains_pkey PRIMARY KEY (id)
  constraint domains_name_key UNIQUE (name)
  index domains_pkey unique USING btree (id)
  index domains_status_idx USING btree (status)
  trigger set_updated_at BEFORE UPDATE -> touch_updated_at()

enum public.domain_state = new | ok | failed

-- итого: схем 1, таблиц 1, enum 1
```

Флаги: `--schema <name>` (сузить до одной схемы), `--definitions` (тела вьюх и
исходники функций), `--comments` (тексты `COMMENT ON`), `--internal` (системные
схемы, объекты расширений и листовые партиции — по умолчанию скрыты как шум).
Точечные `columns`/`ddl`/`indexes` остаются для случая «нужна одна таблица, а
дамп большой».

## Точечная интроспекция, журналы, здоровье

```bash
sql-kai tables <alias> [--counts]        # только имена (+ примерное число строк)
sql-kai columns <alias> [schema.]table
sql-kai ddl <alias> [schema.]table       # CREATE TABLE / VIEW
sql-kai indexes <alias> [schema.]table
sql-kai logs <alias> [-n 200] [-f]       # журнал postgres-сервера (ssh -> docker logs)
sql-kai history [-n 20] [<alias>]        # история запросов (общая с GUI)
sql-kai saved list <alias> | sql-kai saved run <alias> <имя>
sql-kai doctor [<alias>]                 # сохранённые пароли ещё аутентифицируются?
sql-kai sessions                         # живые сессии GUI-брокера и holder'а
```

`sql-kai logs` работает и тогда, когда сам postgres уже не отвечает (упал,
перезапускается, забил диск) — это первое, что стоит посмотреть при «база
недоступна», прежде чем гадать. Фильтры: `--since 10m`, `--until`,
`--timestamps`, `--container <имя>`; строки идут в stdout, так что `| grep ERROR`
работает. `-f` стримит до Ctrl+C — не запускай его в блокирующем режиме.
Для managed-базы без ssh команда честно откажет: журнал там только в панели
провайдера.

## Правила безопасности

- **Read-only — сразу.** `SELECT`/`EXPLAIN`/интроспекцию выполнять без вопросов;
  CLI и так не даст писать без `--write`.
- **`--write` — только с явного подтверждения пользователя** в текущей сессии.
  Перед изменением показать предварительный `SELECT` затрагиваемых строк;
  `UPDATE`/`DELETE` всегда с `WHERE`; DDL оформлять миграцией в репозитории
  сервиса, а не «вживую».
- **В production-профиль одного `--write` не хватает.** Профили с меткой
  `production` требуют отдельного разрешения, и даёт его человек: ввод имени
  профиля в ответ на вопрос в терминале, флаг `--prod-write` рядом с `--write`
  или `SQL_KAI_ALLOW_PROD_WRITE=<профиль>` в окружении. Барьер общий для `q`,
  `saved run`, `rotate`, MCP-tool `query` и `exec` на ssh-хост prod-профиля;
  чтение он не трогает. Получил отказ — **не подбирай обход и не выставляй эту
  переменную сам**: покажи отказ пользователю и спроси. В выводе CLI метки не
  видно (она стоит в карточке профиля в GUI, у MCP её отдаёт tool `profiles`) —
  так что про прод узнаёшь из самого отказа, и это нормально.
- **Читающий вызов выполняется внутри `BEGIN READ ONLY`.** Поэтому на чтении
  отвергается всё, что вывело бы батч из этой транзакции или сняло бы с неё
  read-only: `COMMIT`, `ROLLBACK`, `END`, `ABORT`, `PREPARE TRANSACTION`,
  `DISCARD`, `SET TRANSACTION`, `SET …transaction_read_only`. Это не баг парсера
  и не повод переписывать запрос хитрее: батчу, которому они нужны, нужен
  `--write`. Обычные `SET search_path` / `SET statement_timeout` работают.
- Не выгребать секреты/ПДн без причины; на больших таблицах — `LIMIT`.

## Безопасный DDL на проде

Postgres выполняет DDL транзакционно, но почти любая форма `ALTER TABLE` берёт
**AccessExclusiveLock** — он несовместим со всем, включая `SELECT`. Опасна не
столько сама блокировка, сколько её длительность и очередь за ней: пока ALTER
ждёт чужой долгий запрос, все новые запросы к таблице встают за ALTER'ом, и
таблица встаёт целиком. Отсюда правила.

**1. Таймауты — первым делом.** В том же батче, что и сам DDL (батч = одна
неявная транзакция, поэтому `SET LOCAL` — и настройка не протечёт в следующие
запросы: сессия sql-kai живёт между вызовами):

```sql
SET LOCAL lock_timeout = '3s';
SET LOCAL statement_timeout = '60s';
ALTER TABLE ...;
```

Не дождались лока — упала команда, а не прод; повторить в тихое окно. Заодно
посмотри, нет ли долгих транзакций, за которыми придётся стоять:

```sql
SELECT pid, state, now() - xact_start AS age, left(query, 80)
FROM pg_stat_activity
WHERE state <> 'idle' AND xact_start < now() - interval '1 minute'
ORDER BY age DESC;
```

**2. Что дёшево, а что переписывает таблицу.**

| Операция | Цена |
|---|---|
| `ADD COLUMN` без дефолта | только метаданные, лок короткий |
| `ADD COLUMN` с неволатильным дефолтом (константа, `now()`) | тоже только метаданные — начиная с PG 11 |
| `ADD COLUMN` с волатильным дефолтом (`random()`, `clock_timestamp()`) | полный рерайт таблицы |
| `DROP COLUMN`, `RENAME`, `SET/DROP DEFAULT` | только метаданные (место после DROP не освобождается) |
| `SET NOT NULL` | скан всей таблицы под AccessExclusiveLock (см. п. 4) |
| `ALTER COLUMN ... TYPE` | рерайт таблицы + перестройка её индексов (исключение — бинарно совместимые: `varchar(n)` → более длинный `varchar`/`text`) |
| `ADD CONSTRAINT CHECK` / `FOREIGN KEY` без `NOT VALID` | скан всей таблицы под локом |
| `VACUUM FULL`, `CLUSTER`, `REINDEX` без `CONCURRENTLY` | рерайт под AccessExclusiveLock — на живой базе не делать (снаружи есть pg_repack) |

Дешёвые формы всё равно берут AccessExclusiveLock — просто на миллисекунды.
Именно поэтому им и нужен `lock_timeout`: без него они ждут в очереди сколько
угодно, блокируя всех за собой.

**3. Индексы — только `CONCURRENTLY`.** Обычный `CREATE INDEX` держит ShareLock:
чтение идёт, запись стоит всё время построения. `CREATE INDEX CONCURRENTLY` не
блокирует ни чтение, ни запись, но: не работает внутри транзакции, делает два
прохода по таблице (дольше) и при падении оставляет невалидный индекс — его надо
снести и повторить.

```bash
# один стейтмент на вызов: несколько -c или ; уходят одним батчем = одна
# неявная транзакция, а в транзакции CONCURRENTLY упадёт
sql-kai <alias> -c "CREATE INDEX CONCURRENTLY idx_domains_status ON domains (status)" --write
sql-kai <alias> -c "SELECT indexrelid::regclass FROM pg_index WHERE NOT indisvalid"
```

Невалидный индекс убирают `DROP INDEX CONCURRENTLY` — тоже отдельным вызовом.

**4. `NOT NULL` и внешние ключи — в три шага.** Прямой `SET NOT NULL` сканирует
таблицу под AccessExclusiveLock. Разбиваем так, чтобы под тяжёлым локом ничего
не сканировалось:

```sql
-- 1. мгновенно: констрейнт объявлен, но существующие строки не проверяются
ALTER TABLE t ADD CONSTRAINT t_col_nn CHECK (col IS NOT NULL) NOT VALID;
-- 2. скан под SHARE UPDATE EXCLUSIVE — чтение и запись не блокируются
ALTER TABLE t VALIDATE CONSTRAINT t_col_nn;
-- 3. PG 12+ видит валидный CHECK и повторно таблицу не сканирует
ALTER TABLE t ALTER COLUMN col SET NOT NULL;
ALTER TABLE t DROP CONSTRAINT t_col_nn;   -- по желанию
```

**Каждый шаг — отдельным вызовом `sql-kai`.** Одним батчем они уедут в одну
транзакцию, и короткий лок шага 1 будет удерживаться всё время скана шага 2 —
ровно то, чего мы избегали.

Перед шагом 1 колонку надо забэкфилить — **не одним `UPDATE` на всю таблицу**
(долгая транзакция, раздувание, лаг реплик), а пачками по ключу, каждая — своим
вызовом (то есть своей транзакцией):

```sql
UPDATE t SET col = 'default'
WHERE id IN (SELECT id FROM t WHERE col IS NULL ORDER BY id LIMIT 10000);
```

Тот же приём для внешнего ключа: `ADD CONSTRAINT ... FOREIGN KEY ... NOT VALID`,
затем отдельным вызовом `VALIDATE CONSTRAINT`.

**5. Смена типа колонки — через новую колонку.** `ALTER COLUMN ... TYPE`
переписывает таблицу и её индексы под AccessExclusiveLock: на большой таблице
это простой на минуты. Безопасный путь (expand/contract):

1. `ADD COLUMN col_new <тип>` — дёшево;
2. триггер `BEFORE INSERT OR UPDATE`, который пишет значение в обе колонки;
3. бэкфилл пачками (см. выше), сверка расхождений;
4. деплой приложения на новую колонку;
5. `DROP COLUMN col_old` и снос триггера — **следующим** релизом.

Так же и с переименованиями: `RENAME` мгновенен, но ломает работающий код —
сначала новое имя (колонка или вьюха-совместимость), потом деплой, потом снос
старого.

**6. Прогони на копии — для этого есть `fork`.**

```bash
sql-kai fork <alias> --data              # копия базы профиля в локальном docker
                                         # (без --data — только схема, быстро)
sql-kai <alias>-fork -f migration.sql --write   # прогон миграции на копии
docker rm -f sql-kai-fork-<alias>-fork          # снести форк
```

Форк снимается `pg_dump` из read-only-сессии, поднимается на `postgres:<мажор
версии источника>` на 127.0.0.1 и заводится отдельным профилем **без** метки
production — на нём можно всё. Копия без данных проверяет только синтаксис и
совместимость схемы; чтобы оценить время рерайта и увидеть реальные планы,
нужен `--data`. Проверь на форке ровно тот файл, который потом пойдёт на прод.

**7. План отката — до, а не после.**

- К каждому шагу заранее пропиши обратный (`DROP COLUMN` к `ADD COLUMN`,
  `DROP INDEX CONCURRENTLY` к созданию индекса).
- Несколько коротких (метаданных) `ALTER` шли одним батчем: это одна неявная
  транзакция, упал третий — откатились все три. Долгие шаги — `VALIDATE
  CONSTRAINT`, бэкфилл, `CONCURRENTLY` — наоборот, по одному на вызов.
- Необратимое (`DROP COLUMN`/`DROP TABLE`, сужение типа) не выполняй в одном
  релизе с выкладкой кода: сначала перестать использовать, снести через релиз.
  Перед удалением сними страховку — `CREATE TABLE t_backup_YYYYMMDD AS SELECT …`
  или выгрузку в файл: `sql-kai <alias> -c "SELECT …" --json > before.json`.
- Миграция живёт файлом в репозитории сервиса. sql-kai — способ её применить и
  проверить, а не место, где хранится схема.

## Если sql-kai подключён как MCP-сервер

У агента могут быть тулы `sql-kai` (`schema`, `query`, `tables`, `columns`,
`ddl`, `indexes`, `profiles`, `open_table`, `open_query`, `selection`) — тогда
пользуйся ими вместо запуска CLI из шелла: та же сессия, тот же vault, ответ
приходит и текстом, и структурой. Соответствие такое же: `schema` — первый
инструмент интроспекции, `query` с `write: true` — только с подтверждения
пользователя, и в production он не пройдёт вовсе (у сервера нет TTY, а
`SQL_KAI_ALLOW_PROD_WRITE` задаёт человек в конфиге своего MCP-клиента).
Отказ на проде повторять бессмысленно — сообщи о нём пользователю.
Подключается сервер командой `sql-kai mcp install <клиент>`.

## Ссылки

- Полная документация CLI: `docs/sql-kai.html` в этом репозитории
  (разделы «Схема базы одним дампом», «Форк базы», «MCP», «Запись в production»).
- Внешний managed-Postgres (не в docker на хосте) discover не найдёт — профиль
  для него заводится вручную в GUI.
