#!/usr/bin/env bash
# Тонкая обёртка над общим релизным скриптом личных проектов —
# вся логика в ../_release/tauri-release.sh (версии, тег, GitLab-релиз,
# сборка app+dmg, updater-манифест, CLI-sidecar, аплоад артефактов).
set -euo pipefail
cd "$(dirname "$0")"

APP_NAME=sql-kai
PM=pnpm
# архив старого имени CLI — не должен уезжать в релиз
LEGACY_ARTIFACTS="kai-darwin-aarch64.tar.gz"

source ../_release/tauri-release.sh
