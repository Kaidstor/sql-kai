---
name: verify
description: Верификация frontend-изменений sql-kai без сборки Tauri — Vite dev + Chrome (chrome-devtools MCP, либо playwright-core если MCP не подключён) с шимом Tauri IPC. Использовать для проверки React-компонентов/стора, когда не нужен Rust-бэкенд.
---

# Верификация UI sql-kai в браузере

Полная сборка Tauri долгая, а нативное окно не поддаётся автоматизации без
macOS-разрешений. Для frontend-изменений (React, zustand-стор, CSS) приложение
можно целиком прогнать в Chrome, подменив Tauri IPC шимом.

## Рецепт

0. **Проверь, что chrome-devtools MCP подключён к сессии** — есть ли
   инструменты `mcp__chrome-devtools__*` (`new_page`, `navigate_page`).
   Сервер прописан в `~/.claude.json`, но в конкретной сессии бывает не
   поднят. Если инструментов нет — не сдавайся и не проси пользователя
   чинить MCP: иди в «Фолбэк без MCP» ниже, шим и весь рецепт те же.
1. Vite в фоне на **своём порту**, не на дефолтном 1420:
   `pnpm dev --port <порт> --strictPort` (без `--` перед флагами — pnpm
   передаст его в vite буквально, и флаги молча проигнорируются: vite
   отрапортует `ready`, но на 1420). CLI-флаг перекрывает порт из
   vite.config.ts. Порт бери свободный из диапазона 14300–14399
   (проверь `lsof -nP -iTCP:<порт> -sTCP:LISTEN`) — 1420 может быть занят
   другой сессией Claude или настоящим `tauri dev`, мешать им нельзя.
   `--strictPort` обязателен: иначе при занятом порте vite молча съедет
   на соседний и проверка пойдёт против чужого инстанса.
2. Через chrome-devtools MCP: `new_page('about:blank')`, затем
   `navigate_page` с `initScript` — шим должен встать **до** загрузки модулей.
   Если `new_page` падает с «The browser is already running for
   …/chrome-devtools-mcp/chrome-profile» — лок профиля держит осиротевший
   Chrome прошлой сессии: убей его процессы
   (`pkill -f "chrome-devtools-mcp/chrome-profile"`) и повтори. Если
   `pkill`/`pgrep -f` молчат, а лок держится — держателя ищи по симлинку
   `~/.cache/chrome-devtools-mcp/chrome-profile/SingletonLock` (он вида
   `host-PID`): `kill -9 <PID>`, затем `rm -f …/chrome-profile/Singleton*`.
   После такого рестарта первый же вызов может ответить «browser was
   restarted or reconnected» и page id-шники слетают — это не ошибка
   рецепта, просто повтори `navigate_page` с тем же `initScript` на новой
   странице.
   **Профиль Chrome у MCP один на всю машину** — в отличие от Vite, его
   нельзя развести по сессиям: за него дерутся все инстансы
   chrome-devtools-mcp (другие сессии Claude, Zed и т.п.), и браузер может
   перезапускаться прямо между твоими вызовами, теряя страницу с шимом.
   Один-два рестарта — нормально, повторяй `navigate_page`; если война за
   лок не утихает — не выбивай чужой браузер до бесконечности, уходи в
   playwright-фолбэк: он поднимает свой инстанс и никому не мешает.
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

Дальше, для workspace с деревом схемы и вкладками таблиц — на
`connect_profile` → `{sessionId,profileId,serverVersion}` стор зовёт
`refreshTables`, который параллелит `list_tables` + `list_all_columns`:

- `list_tables` → `[{schema,name,kind}]`. `kind` бэкенд маппит из
  `pg_class.relkind`: `v`→`view`, `m`→`matview`, `f`→`foreign`, иначе
  `table` (`r`/`p`). Держи в фейке хотя бы один `view` рядом с `table` —
  view-ветки (иконка-глаз, read-only грид, `Copy CREATE VIEW`) иначе
  не отрисуются.
- `list_all_columns` → `[{schema,table,columns:[…]}]`; падение не фатально
  (автокомплит просто останется keyword-only), но тогда в консоли тост.
- `list_columns` → `[{name,dataType,nullable,isPk,…}]` — грузится лениво на
  раскрытие узла в сайдбаре и нужен вкладке с данными.
- остальное → reject `{code:'shim', message:'unhandled <cmd>'}` — updater и
  прочие плагины глотают ошибки сами (в консоли будет шум «Uncaught (in
  promise)» — это артефакт шима, не баг).

## Инжект состояния стора (dev-хук `window.__useApp`)

Состояние, которое в жизни создаёт только живой бэкенд (чат агента,
tool-карточки, permission-запросы, ошибки), через шим не сэмулировать —
инжекть его напрямую в zustand-стор: dev-сборка выставляет
`window.__useApp` (см. `lib/store/index.ts`, только `import.meta.env.DEV`).

- `__useApp.getState()` / `__useApp.setState({...})` — читать/патчить стор
  из `evaluate_script`; экшены зовутся как методы:
  `getState().openTableTab(...)`.
- Для панели агента сначала подключи фейковый профиль —
  `await __useApp.getState().connect("<profileId>")` (шим ответит на
  `connect_profile`/`list_tables`), иначе workspace перекрыт лаунчером и
  `setState({launcherOpen:false})` покажет пустой экран. Потом
  `setState({activeProfileId, agentOpen:true, agentChats:{...}})` с
  руками собранными items (`id` — любые уникальные числа).
- HMR после правки кода перезагружает страницу (`import.meta.hot` →
  reload): шим из `initScript` переживает reload, а вот стор-инжект
  повторяй заново.

## Фолбэк без MCP (playwright-core)

MCP — не единственный способ довести браузер до страницы; шим, набор
команд и гочи ниже применимы один в один.

1. В скретчпаде сессии: `npm i playwright-core` (только драйвер, без
   загрузки браузеров).
2. Браузер бери из кэша playwright, не качай заново:
   `~/Library/Caches/ms-playwright/chromium-*/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing`
   → `chromium.launch({executablePath})`.
3. Шим ставится через `page.addInitScript(…)` — тот же
   `window.__TAURI_INTERNALS__`, что и в `initScript` у MCP.
4. Скриншоты — `page.screenshot({path})`, ограничения workspace roots на
   него не распространяются (можно писать прямо в скретчпад).

Селекторы: подсказки/пункты меню ищи через `getByRole("button", …)`,
а не `getByText` — тексты дублируются в ячейках грида и матчат лишнее.

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
- **Скриншоты**: `take_screenshot` с `filePath` в скретчпад падает с
  «Access denied: path is not within workspace roots» — MCP пишет только
  внутрь проекта. Бери скриншот без `filePath` (инлайн-аттачем) либо
  сохраняй в папку проекта и удаляй после.
- **Клавиатура**: `mcp__chrome-devtools__press_key` шлёт **trusted**
  CDP-ввод — им проверяется настоящая Tab-навигация (движение фокуса),
  Shift+Tab, шорткаты с модификаторами (`Meta+E`) и Enter/Space на
  сфокусированном элементе. Синтетический `new KeyboardEvent('keydown',…)`
  — только фолбэк для обработчиков в своём JS (они не проверяют
  `isTrusted`); встроенное поведение браузера (перемещение фокуса, ввод
  символов) на синтетику не реагирует.
- **Esc и диалоги**: ConnectionDialog и прочие на `Overlay` не закрываются
  по Esc даже от trusted-ввода — это дефолт `closeOnEsc=false` в
  `ui.tsx` (защита введённой формы), не артефакт среды. Закрывай кликом
  по Cancel/крестику/фону.
- После проверки: закрой вкладку (или уведи её на `about:blank`) и
  останови **свой** Vite (TaskStop / kill по своему порту). Чужие
  инстансы на 1420 и соседних портах не трогать — там может работать
  другая сессия.
