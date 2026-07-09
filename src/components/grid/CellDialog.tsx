import { X } from "lucide-react";
import { Button, IconButton, Overlay } from "../ui";

/** Full-value cell viewer/editor state (json/jsonb arrives prettified). */
export interface CellDialogState {
  row: number;
  col: number;
  text: string;
  isJson: boolean;
}

/** Overlay with the full cell value in a textarea: read-only viewer for plain
 *  results, staging editor when the grid is editable (⌘⏎ stages). */
export function CellDialog({
  dialog,
  columnName,
  columnType,
  canEdit,
  onText,
  onStage,
  onClose,
  onCopy,
}: {
  dialog: CellDialogState;
  columnName: string;
  columnType?: string;
  canEdit: boolean;
  onText: (text: string) => void;
  onStage: () => void;
  onClose: () => void;
  onCopy: (text: string) => void;
}) {
  return (
    <Overlay onClose={onClose} className="items-center bg-black/60">
      <div className="flex w-[44rem] max-w-[92vw] flex-col rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-2.5">
          <span className="font-mono text-[12px] text-zinc-100">
            {columnName}
          </span>
          {columnType && (
            <span className="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">
              {columnType}
            </span>
          )}
          <span className="text-[11px] text-zinc-600">
            row {dialog.row + 1}
          </span>
          <div className="ml-auto">
            <IconButton onClick={onClose}>
              <X size={14} />
            </IconButton>
          </div>
        </div>
        <textarea
          autoFocus
          spellCheck={false}
          value={dialog.text}
          readOnly={!canEdit}
          onChange={(e) => onText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            if ((e.metaKey || e.ctrlKey) && e.key === "Enter") onStage();
          }}
          className="selectable m-3 h-80 resize-y rounded-md border border-zinc-700 bg-zinc-950 p-2.5 font-mono text-[12px] leading-relaxed text-zinc-100 outline-none focus:border-sky-600"
        />
        <div className="flex items-center gap-2 border-t border-zinc-800 px-4 py-2.5">
          {dialog.isJson && (
            <span className="text-[11px] text-zinc-600">
              JSON · prettified{canEdit ? " · stored compact" : ""}
            </span>
          )}
          <div className="ml-auto flex items-center gap-2">
            <Button onClick={() => onCopy(dialog.text)}>Copy</Button>
            {canEdit && (
              <Button variant="primary" title="⌘⏎" onClick={onStage}>
                Stage change
              </Button>
            )}
            <Button onClick={onClose}>Close</Button>
          </div>
        </div>
      </div>
    </Overlay>
  );
}
