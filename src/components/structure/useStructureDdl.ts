import { useApp } from "../../lib/store";

/** Confirm(optional)-then-run for the ad-hoc section DDL (create / drop /
 *  rename / toggle). The confirm message is the SQL itself; runDdl adds the
 *  production guard and refreshes the section. */
export function useStructureDdl(tabId: string) {
  const runDdl = useApp((s) => s.runDdl);
  const confirmDialog = useApp((s) => s.confirmDialog);
  return async (
    sql: string,
    confirm?: { title: string; danger?: boolean; label?: string },
  ): Promise<boolean> => {
    if (confirm) {
      const ok = await confirmDialog({
        title: confirm.title,
        message: sql,
        confirmLabel: confirm.label ?? (confirm.danger ? "Drop" : "Run"),
        danger: confirm.danger,
      });
      if (!ok) return false;
    }
    return runDdl(tabId, sql);
  };
}
