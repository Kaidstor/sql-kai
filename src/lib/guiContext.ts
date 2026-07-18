// Ответ GUI на MCP-тул `selection`: что пользователь сейчас видит (активная
// вкладка с фильтром/сортировкой) и что выделил (строки/колонки/ячейки —
// с данными). Собирается из стора по событию agent://gui-request и уходит
// обратно командой agent_gui_reply. Ответ читает модель — формат: понятный
// JSON без внутренних индексов стора.
import { readSelectionSnapshot } from "../components/grid/useGridSelection";
import type { Tab } from "./store/types";
import type { StatementResult } from "./types";

/** Кап строк данных в ответе: модель читает выделение, а не весь датасет. */
const MAX_ROWS = 100;

type ReadSnapshot = typeof readSelectionSnapshot;

export function buildGuiContext(
  state: { tabs: Tab[]; activeTabId: string | null },
  profileId: string,
  read: ReadSnapshot = readSelectionSnapshot,
): unknown {
  const tab = state.tabs.find((t) => t.id === state.activeTabId);
  if (!tab) return { note: "no active tab in the GUI" };
  if (tab.profileId !== profileId) {
    return {
      note: "the active GUI tab belongs to a different connection profile",
    };
  }
  switch (tab.state.kind) {
    case "table": {
      const st = tab.state;
      const result = st.data?.result;
      return {
        tab: {
          kind: "table",
          schema: st.schema,
          table: st.table,
          filter: st.filter || null,
          sorts: st.sorts,
          page: st.page,
          pageSize: st.pageSize,
          approxRows: st.data?.approxRows ?? null,
          loadedRows: result?.rows.length ?? 0,
        },
        selection: (result && selectionOf(result, read)) ?? { kind: "none" },
      };
    }
    case "query": {
      const st = tab.state;
      // выделение ищем по всем statement-гридам результата
      let selection: unknown = null;
      let statement: number | undefined;
      for (const [i, r] of (st.result?.results ?? []).entries()) {
        const sel = selectionOf(r, read);
        if (sel) {
          selection = sel;
          statement = i;
          break;
        }
      }
      return {
        tab: {
          kind: "query",
          title: tab.title,
          sql: st.resultSql ?? st.sql,
          statements: st.result?.results.length ?? 0,
        },
        ...(statement !== undefined && statement > 0 ? { statement } : {}),
        selection: selection ?? { kind: "none" },
      };
    }
    case "structure":
      return {
        tab: {
          kind: "structure",
          schema: tab.state.schema,
          table: tab.state.table,
        },
        selection: { kind: "none" },
      };
    case "activity":
      return { tab: { kind: "activity" }, selection: { kind: "none" } };
  }
}

/** Выделение одного result-грида с данными; null — ничего не выделено.
 *  rowNumbers — как в гаттере грида: 1-based, в пределах загруженной страницы. */
function selectionOf(result: StatementResult, read: ReadSnapshot): unknown {
  const snap = read(result);
  if (!snap) return null;
  if (snap.rows.length > 0) {
    const rows = snap.rows.slice(0, MAX_ROWS);
    return {
      kind: "rows",
      rowNumbers: rows.map((i) => i + 1),
      columns: result.columns,
      rows: rows.map((i) => result.rows[i]),
      ...(snap.rows.length > rows.length
        ? { capped: MAX_ROWS, totalSelected: snap.rows.length }
        : {}),
    };
  }
  if (snap.cols.length > 0) {
    return {
      kind: "columns",
      columns: snap.cols.map((c) => result.columns[c]),
      rows: result.rows
        .slice(0, MAX_ROWS)
        .map((r) => snap.cols.map((c) => r[c] ?? null)),
      ...(result.rows.length > MAX_ROWS
        ? { capped: MAX_ROWS, totalRows: result.rows.length }
        : {}),
    };
  }
  if (snap.cellRect) {
    const { r1, r2, c1, c2 } = snap.cellRect;
    const rows: (string | null)[][] = [];
    for (let r = r1; r <= Math.min(r2, r1 + MAX_ROWS - 1); r++) {
      rows.push((result.rows[r] ?? []).slice(c1, c2 + 1));
    }
    return {
      kind: "cells",
      rowNumbers: rows.map((_, i) => r1 + i + 1),
      columns: result.columns.slice(c1, c2 + 1),
      rows,
    };
  }
  return null;
}
