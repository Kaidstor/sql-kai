# sql-kai

Десктопный Postgres-клиент (Tauri 2 + React 19) с нативной поддержкой SSH-туннелей
и CLI `kai` поверх того же ядра. Историческое имя папки/крейта — `sql-tauri`.

## Стек

- **Backend**: Rust, `tokio-postgres` (simple-query протокол — сервер сам форматирует значения в текст, любые типы отображаются без маппинга), собственный vault (`aes-gcm` + `argon2` — секреты шифруются мастер-паролем), SSH-туннели через супервизию системного `ssh -N -L`.
- **Frontend**: React 19, Vite 8, Tailwind CSS v4, zustand, TanStack Table, CodeMirror 6 (`@codemirror/lang-sql`).

## Возможности

- Профили подключений: сохраняются в `~/Library/Application Support/sql-tauri/profiles.json` (файл 0600, только несекретные поля). Пароли БД и SSH-passphrase шифруются в `vault.json`: один случайный ключ (DEK) шифрует все секреты одним AES-256-GCM блобом, сам DEK обёрнут ключом из мастер-пароля (Argon2id). При старте vault разблокируется один раз на всю сессию — никаких per-connection промптов ОС. Все конфиг-файлы пишутся атомарно (tmp+fsync+rename), DEK зачищается из памяти при блокировке.
- Touch ID (macOS): опциональный быстрый путь — приложение проверяет биометрию через `LocalAuthentication` (LAContext, системный fallback — пароль аккаунта) и затем читает копию DEK из login keychain. Никаких entitlements/провижининг-профилей/платного Apple-аккаунта не требуется, работает и в `tauri dev`. Мастер-пароль всегда остаётся app-fallback'ом. Включается чекбоксом на экране setup/unlock. Release-сборка подписывается сертификатом Apple Development (`bundle.macOS.signingIdentity`) — стабильная подпись избавляет от повторных keychain-промптов между сборками.
- SSH-туннель на профиль: указываешь SSH-хост (работают alias из `~/.ssh/config`, включая ProxyJump), приложение само подбирает свободный локальный порт, поднимает `ssh -N -L`, следит за процессом и убивает его при дисконнекте/выходе.
- SQL-редактор с подсветкой Postgres-диалекта, ⌘⏎ — выполнить, поддержка нескольких стейтментов через `;`, отмена длинного запроса (pg cancel protocol).
- Браузер схемы: схемы → таблицы/вьюхи, фильтр по имени.
- Табличный просмотр: пагинация, сортировка кликом по колонке, приблизительный счётчик строк (`reltuples`, без `count(*)` по большим таблицам).
- Лимит строк на выборку (100 … 50 000), индикатор обрезки.
- Выделение строк в гриде: клик — одна, ⌘/Ctrl+клик — точечно, Shift+клик — диапазон. ПКМ — контекстное меню (Base UI): копировать ячейку / строки через пробел (⌘C) / TSV / JSON / всё с заголовком.
- Кастомная шапка окна: нативный titlebar скрыт (`titleBarStyle: Overlay`), traffic lights поверх приложения, окно таскается за верхнюю полосу (сайдбар + таббар).

## SSH: как это работает

Туннель — это дочерний процесс `ssh -N -o BatchMode=yes -o ExitOnForwardFailure=yes -L 127.0.0.1:<free>:<db_host>:<db_port> <target>`.

- Аутентификация: ключи / ssh-agent / `~/.ssh/config`. Пароль по SSH не поддерживается (BatchMode) — GUI не может отвечать на интерактивные промпты.
- `Host`/`Port` в секции Database — адрес БД **с точки зрения SSH-сервера** (обычно `localhost:5432`).
- `ServerAliveInterval=15` — мёртвый туннель обнаруживается за ~45 секунд; при потере соединения клиент попросит переподключиться.

## CLI `kai`

Консольный клиент поверх того же Rust-ядра (отдельный бинарь, cargo-фича `cli`):
профили, ssh-туннели, vault и история запросов общие с GUI. Подробная
документация — [docs/kai.html](docs/kai.html).

```bash
kai <alias> -c "SELECT ..."     # SQL по профилю; вывод table/--json/--csv/-t
kai discover <ssh-alias>        # ssh → найти postgres в docker → создать профиль
kai exec <ssh-alias> -c "..."   # fallback без профиля: ssh + docker exec psql
kai tables|columns|ddl|indexes <alias> [schema.]table
kai tunnel list|close [--all]   # персистентные ssh-туннели (ControlMaster)
kai vault trust                 # тихий доступ CLI к паролям vault (keychain)
```

Ключевое:

- **Сессия по умолчанию read-only** (`SET default_transaction_read_only = on`);
  запись и DDL — только с явным `--write`.
- Мультистейтмент — одна неявная транзакция: ошибка в середине откатывает всё.
- `kai discover` сам находит postgres-контейнер на хосте (`docker ps` → env
  контейнера → published-порт или bridge-IP) и сохраняет профиль; пароль уходит
  в vault. Профиль сразу виден в GUI.
- **Переиспользование ssh:** для профилей с туннелем CLI держит персистентный
  ssh-мастер (`ControlMaster` + `ControlPersist`) — первый запрос платит за
  аутентификацию, последующие цепляются к готовому мастеру без повторной
  авторизации (запросы за туннелем заметно быстрее). Включено по умолчанию;
  `--no-mux` отключает, `KAI_SSH_MUX_TTL` задаёт TTL, `kai tunnel list|close`
  управляет мастерами. GUI не затронут.
- Vault в CLI разлочивается по цепочке: keychain-trust (`kai vault trust` —
  копия DEK в login keychain, чтение без промптов) → `KAI_VAULT_PASSWORD` →
  запрос в TTY; полный обход — `--password-env VAR`.
- `SQL_KAI_CONFIG_DIR` переопределяет конфиг-директорию (изолированные окружения/тесты).

Установка:

```bash
# из репозитория (нужен dist: pnpm install && pnpm build)
cargo install --path src-tauri --features cli --bin kai
codesign --force --sign "$(jq -r '.bundle.macOS.signingIdentity' src-tauri/tauri.conf.json)" ~/.cargo/bin/kai

# готовый бинарь из GitLab-релиза (macOS arm64; собирает и грузит release.sh)
curl -fL https://gitlab.com/kaidstor/sql-kai/-/releases/permalink/latest/downloads/kai-darwin-aarch64.tar.gz \
  | tar xz -C /usr/local/bin
```

## Запуск

```bash
pnpm install
pnpm tauri dev     # разработка
pnpm tauri build   # сборка .app/.dmg
./release.sh       # локальный релиз: бамп версии, сборка, подпись, GitLab-релиз
                   # (артефакты автообновления + kai-darwin-aarch64.tar.gz)
```

## Тесты

Интеграционный тест ядра (нужен Docker):

```bash
docker run -d --name sqltauri-test-pg -e POSTGRES_PASSWORD=testpw \
  -e POSTGRES_DB=demo -p 54329:5432 postgres:16-alpine
cd src-tauri && cargo test --test db_integration -- --ignored
```

## Roadmap / идеи

- TLS для прямых подключений (`rustls`), сейчас прямые коннекты — без TLS (через туннель это не критично).
- Общий ssh-туннель между GUI и CLI: сейчас CLI переиспользует свои коннекты через `ControlMaster`, но GUI держит отдельный туннель — их можно свести на один мастер.
- EXPLAIN-визуализация, экспорт CSV/JSON из грида.
