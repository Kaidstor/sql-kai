#!/usr/bin/env bash

set -euo pipefail

# Делает `sql-kai` в PATH симлинком на CLI-бинарь (sql-kai-cli) внутри
# установленного sql-kai.app (модель VS Code: обновление приложения через
# updater обновляет и CLI).
# Использование:
#   ./scripts/install-cli.sh                  — ищет приложение в /Applications и ~/Applications
#   ./scripts/install-cli.sh /path/to/sql-kai.app

LINK="$HOME/.cargo/bin/sql-kai"

if [ $# -ge 1 ]; then
  APP="$1"
else
  for candidate in "/Applications/sql-kai.app" "$HOME/Applications/sql-kai.app"; do
    if [ -d "$candidate" ]; then
      APP="$candidate"
      break
    fi
  done
  if [ -z "${APP:-}" ]; then
    echo "ERROR: sql-kai.app не найден в /Applications и ~/Applications." >&2
    echo "Установите приложение из релиза (или укажите путь аргументом) и повторите." >&2
    exit 1
  fi
fi

BIN="$APP/Contents/MacOS/sql-kai-cli"
if [ ! -x "$BIN" ]; then
  echo "ERROR: в бандле нет CLI: $BIN" >&2
  echo "Нужна версия приложения, собранная с sidecar sql-kai-cli (release.sh с externalBin)." >&2
  exit 1
fi

# Проверяем, что бинарь запускается, до того как трогать текущий симлинк
"$BIN" --version >/dev/null

ln -sf "$BIN" "$LINK"
echo "✓ $LINK → $BIN"
echo "  ($("$LINK" --version))"

# Легаси-симлинк времён команды `kai`: убираем, только если он вёл в бандл
# sql-kai (чужой kai не трогаем; кому нужно короткое имя — сделает alias).
LEGACY="$HOME/.cargo/bin/kai"
if [ -L "$LEGACY" ] && readlink "$LEGACY" | grep -q "sql-kai.app"; then
  rm -f "$LEGACY"
  echo "✓ легаси-симлинк $LEGACY удалён (команда теперь sql-kai)"
fi
