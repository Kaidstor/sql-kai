#!/usr/bin/env bash

set -euo pipefail

# Загрузка переменных из .env файла
if [ -f .env ]; then
  set -a  # автоматически экспортировать все переменные
  source .env
  set +a
fi

# Включить отладочный вывод: DEBUG=1 ./release.sh …
DEBUG=${DEBUG:-0}
if [[ "$DEBUG" == "1" ]]; then
  set -x
fi

# Локальное «CI»: релиз без GitLab CI/CD, всё собирается на этой машине.
# Требования:
#   - pnpm, jq, curl в PATH
#   - .env с GITLAB_TOKEN (project access token с scope api),
#     TAURI_SIGNING_PRIVATE_KEY(_PASSWORD), NAMESPACE, PROJECT, PROJECT_ID, API
# Использование:
#   ./release.sh            — бамп patch-версии из package.json
#   ./release.sh 0.2.0      — явная версия
#   SKIP_BUILD=1 ./release.sh …  — пропустить сборку (артефакты уже собраны)
#   NOTES="…" ./release.sh …     — свой changelog; без NOTES он собирается из
#                                  коммитов с прошлого тега (git log --pretty='- %s')

# Determine version: use argument if provided, else bump patch version from package.json
if [ $# -eq 0 ] || [ -z "$1" ]; then
  CUR_VER=$(jq -r .version package.json)
  NEW_VER=$(echo "$CUR_VER" | awk -F. -v OFS=. '{$NF++; print}')
else
  NEW_VER=$1
fi

TAG="v${NEW_VER}"

# Changelog: NOTES из env или коммиты с прошлого тега (release-коммиты
# отфильтровываются). Уходит в описание GitLab-релиза и в notes latest.json —
# его показывает апдейтер. Считаем до release-коммита, чтобы он не попал в список.
PREV_TAG=$(git describe --tags --abbrev=0 2>/dev/null || true)
if [[ -n "${NOTES:-}" ]]; then
  RELEASE_NOTES="$NOTES"
elif [[ -n "$PREV_TAG" ]]; then
  RELEASE_NOTES=$(git log "${PREV_TAG}..HEAD" --no-merges --pretty='- %s' | grep -vE '^- release(\(|:)' || true)
else
  RELEASE_NOTES=""
fi
[[ -n "$RELEASE_NOTES" ]] || RELEASE_NOTES="Release $TAG"

echo "🎯  Релиз версии ${NEW_VER}"
echo "📝  Changelog:"
printf '%s\n' "$RELEASE_NOTES" | sed 's/^/      /'

echo "> Обновляем версии…"
npm pkg set version="$NEW_VER"

# macOS-совместимое обновление tauri.conf.json
tmp=$(mktemp)
jq --arg v "$NEW_VER" '.version = $v' src-tauri/tauri.conf.json > "$tmp" \
  && mv "$tmp" src-tauri/tauri.conf.json

# CLI kai показывает версию из Cargo.toml (clap `version`) — бампаем и его
# (+ Cargo.lock, чтобы дерево осталось чистым после сборки)
perl -0pi -e "s/(\[package\][^\[]*?version = \")[^\"]+/\${1}$NEW_VER/s" src-tauri/Cargo.toml
perl -0pi -e "s/(name = \"sql-kai\"\nversion = \")[^\"]+/\${1}$NEW_VER/" src-tauri/Cargo.lock

git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git commit -m "release: ${TAG}"

echo "> Создаём Git-тег $TAG"
git tag "$TAG"
git push origin HEAD
git push origin "$TAG"

echo "> Создаём релиз $TAG"
# jq -n экранирует переводы строк/кавычки changelog'а в валидный JSON
RELEASE_PAYLOAD=$(jq -n --arg name "Release $TAG" --arg tag "$TAG" --arg desc "$RELEASE_NOTES" \
  '{name: $name, tag_name: $tag, description: $desc}')
RELEASE_RESP=$(curl -s -w "\n%{http_code}" --request POST \
  --header "PRIVATE-TOKEN: $GITLAB_TOKEN" \
  --header "Content-Type: application/json" \
  --data "$RELEASE_PAYLOAD" \
  "$API/projects/$PROJECT_ID/releases")

HTTP_CODE=$(echo "$RELEASE_RESP" | tail -n1)
BODY=$(echo "$RELEASE_RESP" | sed '$d')

if [[ "$HTTP_CODE" != "201" ]]; then
  echo "ERROR: Failed to create release (HTTP $HTTP_CODE)"
  echo "Response: $BODY"
  exit 1
fi
echo "  ✓ Релиз создан"


if [[ -z "${SKIP_BUILD:-}" ]]; then
  echo "> Устанавливаем deps и собираем…"
  pnpm install
  echo "> Собираем CLI kai…"
  (cd src-tauri && cargo build --release --features cli --bin kai)
  # cargo даёт ad-hoc подпись (меняется при каждой сборке → keychain-промпты
  # у vault trust); переподписываем тем же сертификатом, что и приложение
  SIGN_ID=$(jq -r '.bundle.macOS.signingIdentity // empty' src-tauri/tauri.conf.json)
  if [[ -n "$SIGN_ID" ]]; then
    codesign --force --sign "$SIGN_ID" src-tauri/target/release/kai
  fi
  # kai едет внутрь .app как sidecar (externalBin) → обновляется вместе с
  # приложением через updater; ~/.cargo/bin/kai — симлинк в бандл
  # (scripts/install-cli.sh). externalBin добавляем только здесь через
  # --config-override, чтобы tauri dev / cargo check не требовали binaries/.
  TRIPLE=$(rustc -Vv | awk '/^host:/{print $2}')
  mkdir -p src-tauri/binaries
  cp src-tauri/target/release/kai "src-tauri/binaries/kai-$TRIPLE"
  # app — для updater-архива, dmg — для ручной установки со страницы релиза
  pnpm tauri build --ci --bundles app,dmg \
    --config '{"bundle":{"externalBin":["binaries/kai"]}}'
  echo "✔️  Сборка готова"
else
  echo "> SKIP_BUILD задан — пропускаем стадию сборки."
fi

# директория с bundle-артефактами
BUNDLE_DIR=src-tauri/target/release/bundle

echo "> Генерируем latest.json..."
# найдём первый tar.gz архив для подписи
TAR_ARCHIVE=$(find "$BUNDLE_DIR" -type f -name "*.tar.gz" | head -n1)
SIG_FILE="$TAR_ARCHIVE.sig"
# читаем подпись, удаляя переводы строк
SIG=$(tr -d '\n' < "$SIG_FILE")
# формируем URL загрузки артефакта

URL="https://gitlab.com/$NAMESPACE/$PROJECT/-/releases/$TAG/downloads/$(basename "$TAR_ARCHIVE")"
# создаём манифест (jq — ради корректного экранирования changelog в notes)
jq -n --arg v "$NEW_VER" --arg notes "$RELEASE_NOTES" \
  --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg url "$URL" --arg sig "$SIG" \
  '{version: $v, notes: $notes, pub_date: $date,
    platforms: {"darwin-aarch64": {url: $url, signature: $sig}}}' \
  > "$BUNDLE_DIR/latest.json"

echo "> Пакуем CLI kai…"
# имя без версии — стабильная ссылка через permalink/latest/downloads/
tar -czf "$BUNDLE_DIR/kai-darwin-aarch64.tar.gz" -C src-tauri/target/release kai

echo "> Собираем список артефактов текущей версии…"

BUNDLE_FILES=()
# Ищем все пригодные файлы, но берём только те,
# что содержат номер текущей версии $NEW_VER или latest.json
while IFS= read -r -d '' file; do
  name="$(basename "$file")"
  if [[ "$name" == *"$NEW_VER"* ]] || [[ "$name" == "latest.json" ]] || [[ "$name" == *.app.tar.gz ]] || [[ "$name" == *.app.tar.gz.sig ]] || [[ "$name" == kai-*.tar.gz ]]; then
    BUNDLE_FILES+=("$file")
  fi
done < <(find "$BUNDLE_DIR" -type f \( \
        -name "*.dmg"        -o -name "*.tar.gz"       -o -name "*.tar.gz.sig" \
        -o -name "*.AppImage" -o -name "*.AppImage.sig" \
        -o -name "*.deb"     -o -name "*.deb.sig" \
        -o -name "*.msi"     -o -name "*.msi.sig" \
        -o -name "*.zip"     -o -name "*.zip.sig" \
        -o -name "latest.json" \) -print0)

echo "Найдено ${#BUNDLE_FILES[@]} файлов:"
printf '  %s\n' "${BUNDLE_FILES[@]}"

# Заливаем каждый файл через Uploads API и привязываем к релизу
for file in "${BUNDLE_FILES[@]}"; do
  echo "→ Upload: $file"
  RESP=$(curl -s --header "PRIVATE-TOKEN: $GITLAB_TOKEN" \
    --form "file=@$file" \
    "$API/projects/$PROJECT_ID/uploads")

  # GitLab ≥17.1 возвращает .full_path, а более старые версии – .url
  URL=$(echo "$RESP" | jq -r '.full_path // .url')
  # Формируем абсолютный URL, который корректен для всех версий GitLab
  FULL_URL="https://gitlab.com${URL}"
  NAME=$(basename "$file")

  echo "  → Link to release ${TAG}: $NAME"
  curl -s --request POST \
    --header "PRIVATE-TOKEN: $GITLAB_TOKEN" \
    --header "Content-Type: application/json" \
    --data "{\"name\":\"$NAME\",\"url\":\"$FULL_URL\",\"direct_asset_path\":\"/$NAME\"}" \
    "$API/projects/$PROJECT_ID/releases/$TAG/assets/links" > /dev/null

  echo "    ✓ $NAME"
  if [[ "$DEBUG" == "1" ]]; then
    echo "    ↪ Проверяем доступность:"
    echo "      HEAD $FULL_URL"
    curl -I -s "$FULL_URL" | head -n 1
    LINK_DL="https://gitlab.com/$NAMESPACE/$PROJECT/-/releases/${TAG}/downloads/$NAME"
    echo "      HEAD $LINK_DL"
    curl -I -s "$LINK_DL" | head -n 1
  fi
done

echo "✅ Релиз $TAG загружен и assets привязаны."
echo "Клиенты с endpoint:"
echo "  https://gitlab.com/$NAMESPACE/$PROJECT/-/releases/permalink/latest/downloads/latest.json"
echo "при запуске получат обновление до $NEW_VER."
