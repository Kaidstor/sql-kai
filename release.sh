#!/usr/bin/env bash
# Тонкая обёртка над общим релизным скриптом личных проектов —
# вся логика в ../_release/tauri-release.sh (версии, тег, GitHub-релиз,
# сборка app+dmg, updater-манифест, CLI-sidecar, аплоад артефактов).
set -euo pipefail
cd "$(dirname "$0")"

APP_NAME=sql-kai
PM=pnpm
FORGE=github
# переходное зеркало latest.json в GitLab — для клиентов ≤1.20.0, чей
# updater-endpoint ещё смотрит на gitlab.com; убрать через несколько релизов
GITLAB_LATEST_MIRROR=1
# автобамп Homebrew-каска (version/sha256 + push tap) после публикации
BREW_CASK=../homebrew-tap/Casks/sql-kai.rb
# архив старого имени CLI — не должен уезжать в релиз
LEGACY_ARTIFACTS="kai-darwin-aarch64.tar.gz"

source ../_release/tauri-release.sh
