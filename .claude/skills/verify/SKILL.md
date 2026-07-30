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
   **Браузер должен быть headless** — всплывающее окно Chrome мешает
   пользователю работать и ест ресурсы: сервер в `~/.claude.json` запускается
   с флагом `--headless=true` в `args`. Скриншоты, снапшоты и trusted-ввод в
   headless работают как обычно. Если по ходу проверки на экране всё же
   всплыло окно — флаг из конфига пропал; верни его
   (`"args": ["-y", "chrome-devtools-mcp@latest", "--headless=true"]`) и
   предупреди пользователя, что подействует он после переподключения MCP
   (новой сессии) — текущую проверку можно закончить в видимом окне,
   не дёргая браузер.
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

## Результат запроса в гриде (проверка ResultsGrid)

Чтобы довести query-таб до отрисованного грида, не угадывай имена полей —
они такие:

- SQL в таб кладётся экшеном **`setTabSql(tabId, sql)`** (не
  `updateQuerySql`), выполняется **`runQuery(tabId)`**.
- Результат лежит в **`tab.state.result`** (единственное число, тип
  `ExecResult`), не `results`. Рядом: `running`, `error`, `resultSql`,
  `explain`.
- Грид рендерится только у **активной** вкладки — если запускал запрос не в
  ней, переключись: `setState({ activeTabId: <id> })` (или экшен
  `setActiveTab`, если есть), иначе `tbody` в DOM будет пустой при живом
  `state.result`.

`execute_sql` удобнее не зашивать в initScript, а домешивать поверх уже
стоящего шима (данные под конкретную проверку):

```js
const orig = window.__TAURI_INTERNALS__.invoke;
window.__TAURI_INTERNALS__.invoke = (cmd, args) => {
  if (cmd === "execute_sql")
    return Promise.resolve({
      results: [{ columns: ["id", "email"],
                  rows: [["1", "a@b.c"], ["2", null]],
                  rowsAffected: null, truncated: false }],
      durationMs: 4,
    });
  if (cmd === "record_history") return Promise.resolve([]);
  if (cmd === "session_tx_status") return Promise.resolve("idle");
  return orig(cmd, args);
};
const s = window.__useApp.getState();
const tab = s.tabs.find((t) => t.state.kind === "query");
s.setTabSql(tab.id, "SELECT id, email FROM users");
await s.runQuery(tab.id);
```

Гоча: первый `runQuery` сразу после override иногда отрабатывает вхолостую
(результата нет, ошибки нет) — просто повтори вызов в следующем
`evaluate_script`, второй проход стабилен.

## Вкладка таблицы (TableTab)

Грид вкладки таблицы ходит другим путём, чем query-таб:

- Открытие: **`openTableTab(profileId, schema, table)`** — первый аргумент
  именно profileId (легко перепутать со schema). Перепутаешь — стор молча
  создаст вкладку с profileId="public": `refreshTablePage` выйдет на
  `sessionFor(...) == null` без ошибки (`loading:false, error:null`,
  данных нет) — тупик выглядит как «ничего не произошло».
- Данные грузятся командой **`get_table_page`** (не `execute_sql`) →
  `{result: StatementResult, durationMs, approxRows}`; в сторе лежат в
  **`tab.state.data.result`** (не `state.result`, как у query-таба).
- Дефолтный `pageSize` — 100; для проверок на большом объёме:
  `refreshTablePage(tabId, { pageSize: 1000 })` — patch мержится в state
  перед перезагрузкой страницы.
- Pending-вставки (зелёные строки) для проверки грида:
  `duplicateRows(tabId, [ri, …])` — все дубликаты получают `after`
  последней строки-источника и рендерятся под ней.
- Редактирование (dblclick по ячейке) доступно только при PK — шимовый
  `list_columns` должен отдавать `isPk: true` хотя бы одной колонке.
- Staged-правка ячейки из стора: **`stageCellEdit(tabId, row, col, value)`**,
  где `row` и `col` — **числовые индексы** (строка в `rows`, колонка в
  `columns`), не имя колонки. Со строкой вместо индекса правка молча ляжет
  под несуществующий ключ: счётчик Apply вырастет, а подсветки в гриде не
  будет — выглядит как «правки не применились».
- **Сколько подтверждений спросит действие** (прод-барьер и т.п.) считай не
  по DOM — селектор диалога легко промахивается по контейнеру — а по самому
  промису: `const p = page.evaluate(id => __useApp.getState().applyTableEdits(id), id)`,
  один клик по кнопке диалога, `await p`. Промис резолвится только если
  диалог был последним; будь их два — `await` повиснет до таймаута. Прод-путь
  дополнительно виден в вызовах шима: `execute_sql` с `prodWrite:false`
  (отказ `{code:"read_only"}`) и повтор с `true`.

## Фолбэк без MCP (playwright-core)

MCP — не единственный способ довести браузер до страницы; шим, набор
команд и гочи ниже применимы один в один.

1. В скретчпаде сессии: `npm i playwright-core` (только драйвер, без
   загрузки браузеров). `npm` на VPN с IPv6-блэкхолом виснет —
   `NODE_OPTIONS=--dns-result-order=ipv4first`.
2. Браузер — **системный Chrome**, ничего не качать:
   `executablePath: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"`.
   Кэш playwright (`~/Library/Caches/ms-playwright/`) проверить можно, но он
   бывает пустым (каталог `b/browser@<hash>` без содержимого) — тогда это
   тупик, а не повод запускать `playwright install`.
3. Запуск — **`chromium.launchPersistentContext(userDataDir, {executablePath,
   headless: true, viewport})`**, каталог профиля свой, в скретчпаде.
   `headless: true` ставь явно (и не «чини» отсутствием headless упавший
   запуск): окно браузера не должно всплывать у пользователя. `chromium.launch()` с
   `args: ["--user-data-dir=…"]` падает с требованием
   launchPersistentContext, а без своего профиля инстанс полезет в чужой.
   `launchPersistentContext` возвращает контекст: `newPage()` у него, в конце
   `close()` — отдельного `browser` нет.
4. Шим ставится через `page.addInitScript(…)` — тот же
   `window.__TAURI_INTERNALS__`, что и в `initScript` у MCP.
5. Скриншоты — `page.screenshot({path})`, ограничения workspace roots на
   него не распространяются (можно писать прямо в скретчпад).

Селекторы: подсказки/пункты меню ищи через `getByRole("button", …)`,
а не `getByText` — тексты дублируются в ячейках грида и матчат лишнее.

## Гочи

- **Тосты в статус-баре попадают в скриншоты**: действия стора
  (`discardTableEdits` → «Pending changes discarded» и т.п.) показывают тост
  в правом нижнем углу на несколько секунд. Перед кадром подожди ~4с и
  проверь `document.body.innerText.includes("<текст тоста>")`, иначе
  пересъёмка.
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
- **Ленивые чанки (React.lazy)**: `AgentPanel` и прочие lazy-компоненты
  монтируются только после подгрузки своего чанка — проверка
  `document.body.innerText`/скриншот сразу после `setState({agentOpen:true})`
  ловит пустоту и выглядит как «фича не работает». Перед ассертом дождись
  элемента панели (или ~1с) и только потом делай выводы.
- **Батчинг React в одном `evaluate_script`**: несколько шагов стора в одном
  вызове (`setState` + экшен) схлопываются в один рендер — эффекты с deps на
  промежуточное состояние не сработают, как будто шага не было (например,
  `setState(чат с items)` + `resetAgentChat()` одним тиком не обновит
  список сохранённых). Шаги, между которыми UI должен отрендериться,
  разноси по отдельным `evaluate_script`.
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
