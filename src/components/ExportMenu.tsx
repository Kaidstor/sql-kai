// "Export" dropdown for a result set: copy the displayed rows to the
// clipboard, or export the FULL result to a file — the backend re-runs the
// SQL with no row limit and streams it to disk, so the grid's fetch cap
// doesn't apply.
import { FileDown, Loader2 } from "lucide-react";
import { useState } from "react";
import { copyText } from "../lib/clipboard";
import { toCsv, toJson, toTsv } from "../lib/export";
import { promptExportPath } from "../lib/exportFile";
import { dangerousStatements } from "../lib/sql";
import { useApp } from "../lib/store";
import type { ExportFormat, StatementResult } from "../lib/types";
import { cn, MenuButton, Popover } from "./ui";

const COPY_FORMATS = [
  { label: "CSV", make: toCsv },
  { label: "TSV", make: toTsv },
  { label: "JSON", make: toJson },
] as const;

const FILE_FORMATS: { label: string; format: ExportFormat }[] = [
  { label: "CSV…", format: "csv" },
  { label: "JSON…", format: "json" },
  { label: "Excel (XLSX)…", format: "xlsx" },
];

function Item({
  label,
  disabled,
  onClick,
}: {
  label: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "block w-full rounded px-2 py-1 text-left text-[12px] text-zinc-200",
        "hover:bg-zinc-800/60 disabled:pointer-events-none disabled:opacity-40",
      )}
    >
      {label}
    </button>
  );
}

function SectionTitle({ children, title }: { children: string; title?: string }) {
  return (
    <div
      title={title}
      className="px-2 pb-0.5 pt-1.5 text-[10px] uppercase tracking-wide text-zinc-600"
    >
      {children}
    </div>
  );
}

export function ExportMenu({
  result,
  profileId,
  sessionId,
  sql,
  statementIndex = 0,
  autoBegin = false,
  isolatedTabId,
  fileBase,
  rerun = false,
  className,
}: {
  /** Displayed rows (client filter/edits applied) — feeds the copy section. */
  result: StatementResult | undefined;
  /** Profile the connection belongs to — session-lost routing on export. */
  profileId: string;
  /** Connection the full export runs on; null = not connected (disabled). */
  sessionId: string | null;
  /** SQL of the FULL result (no view limit); null = nothing to export. */
  sql: string | null;
  /** Which statement of `sql` the result belongs to (multi-statement runs). */
  statementIndex?: number;
  /** Manual-commit tab: the export re-run wraps writes in BEGIN like Run. */
  autoBegin?: boolean;
  /** Query tab owning `sessionId` when it is the tab's isolated session. */
  isolatedTabId?: string;
  /** Default file name without extension (table name, "result"). */
  fileBase: string;
  /** Full export re-runs the query (query tab) — reflected in the hint. */
  rerun?: boolean;
  className?: string;
}) {
  const showToast = useApp((s) => s.showToast);
  const confirmDialog = useApp((s) => s.confirmDialog);
  const exportSqlToFile = useApp((s) => s.exportSqlToFile);
  // Another tab may be exporting on this very connection — reflect it here
  // too, so two exports never queue behind each other unknowingly.
  const exportingHere = useApp((s) =>
    Boolean(sessionId && s.exporting[sessionId]),
  );
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<ExportFormat | null>(null);

  const shown = result?.rows.length ?? 0;

  const copyShown = (label: string, make: (typeof COPY_FORMATS)[number]["make"]) => {
    if (!result) return;
    setOpen(false);
    void copyText(make(result.columns, result.rows)).then(
      (ok) => ok && showToast(`Copied ${shown} row(s) as ${label}`, "info"),
    );
  };

  const exportAll = async (format: ExportFormat) => {
    if (!sessionId || !sql) return;
    setOpen(false);
    // The file export re-runs the script; when it contains writes, exporting
    // means applying them AGAIN — never do that silently.
    if (rerun) {
      const danger = dangerousStatements(sql);
      if (danger.length > 0) {
        const ok = await confirmDialog({
          title: "Export re-runs the query",
          message:
            "Exporting runs the whole script again, including:\n" +
            danger.map((d) => `• ${d.label}: ${d.preview}`).join("\n"),
          confirmLabel: "Run again & export",
          danger: true,
        });
        if (!ok) return;
      }
    }
    const path = await promptExportPath(fileBase, format);
    if (!path) return;
    setBusy(format);
    try {
      // toasts, session-lost routing and the tx-badge refresh live in the store
      await exportSqlToFile({
        profileId,
        sessionId,
        sql,
        statementIndex,
        autoBegin,
        format,
        path,
        isolatedTabId,
      });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      align="right"
      panelClassName="w-52 p-1"
      trigger={
        <MenuButton
          className={className}
          title="Copy the shown rows or export the full result to a file"
          onClick={() => setOpen((v) => !v)}
        >
          {busy || exportingHere ? (
            <Loader2 size={11} className="animate-spin text-sky-400" />
          ) : (
            <FileDown size={11} className="text-sky-400/80" />
          )}
          Export
        </MenuButton>
      }
    >
      <SectionTitle title="The rows as displayed — client filter and staged edits included">
        {`Copy rows (${shown})`}
      </SectionTitle>
      {COPY_FORMATS.map(({ label, make }) => (
        <Item
          key={label}
          label={label}
          disabled={!shown}
          onClick={() => copyShown(label, make)}
        />
      ))}
      <div className="my-1 border-t border-zinc-800" />
      <SectionTitle
        title={
          rerun
            ? "Re-runs the query with no row limit and writes every row to the file"
            : "Fetches every row (current filter & sort) and writes them to the file"
        }
      >
        Export all to file
      </SectionTitle>
      {FILE_FORMATS.map(({ label, format }) => (
        <Item
          key={format}
          label={label}
          disabled={!sessionId || !sql || busy !== null || exportingHere}
          onClick={() => void exportAll(format)}
        />
      ))}
    </Popover>
  );
}
