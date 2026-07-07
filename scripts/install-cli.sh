#!/usr/bin/env bash

set -euo pipefail

# Делает `kai` в PATH симлинком на бинарь внутри установленного sql-kai.app
# (модель VS Code: обновление приложения через updater обновляет и CLI).
# Использование:
#   ./scripts/install-cli.sh                  — ищет приложение в /Applications и ~/Applications
#   ./scripts/install-cli.sh /path/to/sql-kai.app

LINK="$HOME/.cargo/bin/kai"

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

BIN="$APP/Contents/MacOS/kai"
if [ ! -x "$BIN" ]; then
  echo "ERROR: в бандле нет CLI: $BIN" >&2
  echo "Нужна версия приложения, собранная с sidecar kai (release.sh с externalBin)." >&2
  exit 1
fi

# Проверяем, что бинарь запускается, до того как трогать текущий kai
"$BIN" --version >/dev/null

ln -sf "$BIN" "$LINK"
echo "✓ $LINK → $BIN"
echo "  ($("$LINK" --version))"
