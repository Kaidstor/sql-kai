// Shared types of the results grid (see ResultsGrid.tsx for the orchestrator).
import type { InsertRow } from "../../lib/store";

/** Staged-edit wiring; when present the grid becomes editable (dblclick a cell). */
export interface GridEditing {
  edits: Record<number, Record<number, string | null>>;
  deletes: readonly number[];
  /** Pending INSERT rows shown under the data (⌘D duplicate). */
  inserts: readonly InsertRow[];
  /** Set when editing is unavailable (e.g. no primary key) — shown on attempt. */
  disabledReason?: string;
  /** Last Apply failed and rolled back — staged cells render red, not amber. */
  applyFailed?: boolean;
  onEdit: (row: number, col: number, value: string | null) => void;
  onToggleDelete: (rows: number[], del: boolean) => void;
  onDuplicate: (rows: number[]) => void;
  onInsertEdit: (index: number, col: number, value: string | null | undefined) => void;
  onInsertRemove: (index: number) => void;
  /** ⌘S while the grid is focused and changes are pending. */
  onApply?: () => void;
  /** Esc while the grid is focused and changes are pending. */
  onDiscard?: () => void;
}

/** Grid coordinates of one cell. */
export interface CellRef {
  row: number;
  col: number;
}

/** Rectangular cell selection between two corners (shift+click extends). */
export interface CellRange {
  a: CellRef;
  b: CellRef;
}
