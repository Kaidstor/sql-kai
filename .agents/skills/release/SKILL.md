---
name: release
description: Выпустить релиз sql-kai — бамп версии, тег, GitHub-релиз с changelog, сборка .app/dmg/sql-kai и артефакты автообновления (latest.json). Использовать когда пользователь просит «сделай релиз», «выпусти версию», «зарелизь», «бампни версию» в этом репозитории.
compatibility: Требует авторизованный gh CLI и .env с ключами подписи Tauri, pnpm/jq/curl/rust в PATH; сборка и загрузка идут с этой машины (локальное «CI», внешний CI не участвует)
---

# Релиз sql-kai

Всё делает `./release.sh` из корня репо: бамп версий (package.json,
tauri.conf.json, Cargo.toml/lock) → коммит `release: vX.Y.Z` → сборка
(.app + dmg + CLI sql-kai sidecar'ом) → тег → push → GitHub-релиз с
changelog → `latest.json` → загрузка артефактов (`gh release upload`) →
зеркало `latest.json` в GitLab (пока `GITLAB_LATEST_MIRROR=1`).

## Предусловия

- `gh` CLI авторизован (`gh auth status`); релиз уходит в репозиторий из
  remote `origin`.
- `.env` в корне: `TAURI_SIGNING_PRIVATE_KEY`,
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`; для GitLab-зеркала —
  `GITLAB_TOKEN`, `PROJECT_ID`, `API`.
- **Чистое рабочее дерево.** Скрипт коммитит только файлы версий — всё
  остальное должно быть закоммичено заранее, иначе незакоммиченное попадёт
  в собранные артефакты, но не в git (артефакты перестанут соответствовать тегу).

## Порядок

1. Проверить, что всё собирается (не обязательно, если только что собирали):
   `pnpm build` и `(cd src-tauri && cargo build --release --features cli --bin sql-kai-cli)`.
2. Закоммитить рабочее дерево осмысленными коммитами (Conventional Commits,
   на русском) — **сообщения этих коммитов станут changelog'ом релиза**.
3. Выбрать версию: без аргумента — автобамп patch; новые фичи — минор явно:
   `./release.sh 1.2.0`.
4. Запустить `./release.sh [версия]`. Сборка небыстрая (cargo release +
   bundling) — запускать в фоне и следить за выводом.
5. Проверить: страница релиза на GitHub (описание = changelog, ассеты
   привязаны) и `latest.json` по
   `https://github.com/<owner>/<repo>/releases/latest/download/latest.json`.
   Пока включён `GITLAB_LATEST_MIRROR=1` — дополнительно, что GitLab-зеркало
   отдаёт свежую версию:
   `https://gitlab.com/<NAMESPACE>/<PROJECT>/-/releases/permalink/latest/downloads/latest.json`.

## Changelog

- По умолчанию собирается из `git log <прошлый-тег>..HEAD --pretty='- %s'`
  (merge- и release-коммиты отфильтровываются).
- Переопределить: `NOTES="- пункт 1
- пункт 2" ./release.sh …`
- Уходит в описание GitHub-релиза и в поле `notes` `latest.json`.

## Флаги

- `SKIP_BUILD=1` — артефакты уже собраны, только манифест и загрузка.
- `DEBUG=1` — `set -x` + HEAD-проверки доступности загруженных ссылок.

## Если что-то пошло не так

- **`git push` упал (сеть/ssh до github.com)** — скрипт пушит с ретраями и
  ssh-таймаутами, но если всё равно упало, сначала выяснить, что успело
  улететь: `git ls-remote origin refs/heads/main "refs/tags/vX.Y.Z"`.
  - *Ни ветка, ни тег не запушены* — release-коммит и тег только локальные,
    релиза на GitHub нет: `git tag -d vX.Y.Z`, `git reset --hard HEAD~1`
    и перезапустить скрипт начисто.
  - *Ветка запушилась, тег — нет* — **не делать `reset --hard`** (разведёт
    локальную main с remote). Перезапустить с той же версией:
    `SKIP_BUILD=1 ./release.sh X.Y.Z` — бамп-коммит пропустится (версии уже
    стоят), тег на HEAD переиспользуется, артефакты возьмутся из bundle.
- **Упал после создания релиза / на заливке ассетов** — флоу идемпотентен:
  существующий релиз переиспользуется, ассеты перезаливаются с `--clobber`.
  Перезапустить `SKIP_BUILD=1 ./release.sh X.Y.Z` (артефакты уже в
  `src-tauri/target/release/bundle`). Changelog при таком перезапуске задать
  явно через `NOTES=` — тег уже на HEAD и `git log <тег>..HEAD` пуст.
  Проверить итог: `releases/latest/download/latest.json` отдаёт новую версию,
  у релиза все ассеты (app.tar.gz+sig, dmg, CLI-архив, latest.json).

## После релиза

- Обновлённый CLI sql-kai приезжает внутри .app (externalBin);
  `~/.cargo/bin/sql-kai` — симлинк в бандл, обновится вместе с приложением.
- Запущенные GUI-клиенты подхватят обновление по permalink `latest.json`.
