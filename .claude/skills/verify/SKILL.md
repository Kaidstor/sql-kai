---
name: verify
description: Верификация frontend-изменений sql-kai без сборки Tauri — Vite dev + Chrome (chrome-devtools MCP) с шимом Tauri IPC. Использовать для проверки React-компонентов/стора, когда не нужен Rust-бэкенд.
---

# Верификация UI sql-kai в браузере

Полная сборка Tauri долгая, а нативное окно не поддаётся автоматизации без
macOS-разрешений. Для frontend-изменений (React, zustand-стор, CSS) приложение
можно целиком прогнать в Chrome, подменив Tauri IPC шимом.

## Рецепт

1. `pnpm dev` в фоне — Vite на `http://localhost:1420` (порт фиксированный).
2. Через chrome-devtools MCP: `new_page('about:blank')`, затем
   `navigate_page` с `initScript` — шим должен встать **до** загрузки модулей.
3. Шим — это `window.__TAURI_INTERNALS__` с `invoke`, `transformCallback`
   и `metadata` (см. ниже). Все вызовы `@tauri-apps/api` и плагинов идут
   через него.
4. Логируй вызовы в `window.__SHIM_CALLS__` — по нему проверяется, что и
   когда дернул фронт (например, отложенный `delete_profile`).

## Минимальный набор команд шима

Для доходa до лаунчера (vault-гейт → workspace):

- `vault_status` → `{exists:true, unlocked:true, biometricSupported:false, biometricEnrolled:false}`
- `get_settings` → `{}`
- `list_profiles` → фейковые профили (`{id,name,host,port,database,user,ssh?,production?}`)
- `list_queries` / `list_sessions` / `list_cli_sessions` / `list_history` → `[]`
- `plugin:event|*` → resolve с числом (id подписки)
- остальное → reject `{code:'shim', message:'unhandled <cmd>'}` — updater и
  прочие плагины глотают ошибки сами (в консоли будет шум «Uncaught (in
  promise)» — это артефакт шима, не баг).

## Гочи

- **Hover-состояния**: `hover` из MCP не удерживается до скриншота. Для
  скриншота hover-верстки инжекть `<style>` с копией `group-hover:*` правил
  под маркер-классом (`.force-hover .hidden{display:flex!important}` и т.д.)
  и вешай класс на карточку.
- **Короткие таймеры (undo-грейс 5с)**: раунд-трип между MCP-вызовами >5с,
  промежуточное состояние не поймать. Перехвати `setTimeout` на нужный `ms`
  (капчурь колбэк в Map, `clearTimeout` — удаление из Map), состояние
  зависнет; анимации замораживай `document.getAnimations()` →
  `a.currentTime = …; a.pause()`. Восстанови оригиналы для проб на реальное
  срабатывание.
- **Ввод в контролируемые инпуты React**: сеттер из
  `Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set`
  + `dispatchEvent(new Event('input',{bubbles:true}))`.
- **localStorage переживает сессии проверки** (вкладки+SQL персистятся
  per-profile): при повторном заходе с тем же фейковым profile id
  восстановится старый SQL/вкладки — не удивляйся задвоенному тексту;
  либо чисти localStorage, либо бери свежие id профилей.
- **Dev = React.StrictMode**: mount→unmount→remount двоит эффекты —
  cleanup с `focus()`/сайд-эффектами срабатывает сразу после первого
  mount'а и перебивает autoFocus. Если такое видишь — это баг паттерна,
  а не среды: фокус-менеджмент лучше держать вне эффектов (в сторе).
- После проверки: закрой вкладку, останови `pnpm dev` (TaskStop).
