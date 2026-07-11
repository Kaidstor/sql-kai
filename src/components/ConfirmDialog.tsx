import { useApp } from "../lib/store";
import { Button, Overlay } from "./ui";

/** App-styled modal confirm (see confirmDialog in the ui slice) —
 *  window.confirm() doesn't block in the Tauri webview, so destructive
 *  actions route through this instead. Enter confirms, Esc cancels; the
 *  store restores focus to the triggering element on close. */
export function ConfirmDialog() {
  const confirm = useApp((s) => s.confirm);
  const resolveConfirm = useApp((s) => s.resolveConfirm);
  if (!confirm) return null;

  return (
    <Overlay
      onClose={() => resolveConfirm(false)}
      closeOnEsc
      className="items-center bg-black/60"
    >
      <div className="w-100 rounded-lg border border-zinc-700 bg-zinc-900 p-4 shadow-2xl">
        <div className="text-[13px] font-semibold text-zinc-100">
          {confirm.title}
        </div>
        {confirm.message && (
          <div className="selectable mt-2 max-h-60 overflow-y-auto whitespace-pre-wrap text-[12px] leading-relaxed text-zinc-400">
            {confirm.message}
          </div>
        )}
        <div className="mt-4 flex justify-end gap-2">
          <Button onClick={() => resolveConfirm(false)}>Cancel</Button>
          <Button
            autoFocus
            variant={confirm.danger ? "danger" : "primary"}
            onClick={() => resolveConfirm(true)}
          >
            {confirm.confirmLabel ?? "Confirm"}
          </Button>
        </div>
      </div>
    </Overlay>
  );
}
