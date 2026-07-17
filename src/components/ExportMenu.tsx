// "Export" dropdown for a result set: copy the loaded rows to the clipboard,
// or export the FULL result to a file — the backend re-runs the SQL with no
// row limit and streams it to disk, so the grid's fetch cap doesn't apply.
import { save } from "@tauri-apps/plugin-dialog";
import { FileDown, Loader2 } from "lucide-react";
import { useState } from "react";
import { api, errText } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { toCsv, toJson, toMarkdown, toTsv } from "../lib/export";
import { useApp } from "../lib/store";
import type { ExportFormat, StatementResult } from "../lib/types";
import { cn, MenuButton, Popover } from "./ui";

const COPY_FORMATS = [
  { label: "CSV", make: toCsv },
  { label: "TSV", make: toTsv },
  { label: "JSON", make: toJson },
  { label: "Markdown", make: toMarkdown },
] as const;

const FILE_FORMATS: { label: string; format: ExportFormat }[] = [
  { label: "CSV…", format: "csv" },
  { label: "JSON…", format: "json" },
  { label: "Markdown…", format: "md" },
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
  sessionId,
  sql,
  statementIndex = 0,
  fileBase,
  rerun = false,
  className,
}: {
  /** Loaded rows — feeds the clipboard section. */
  result: StatementResult | undefined;
  /** Connection the full export runs on; null = not connected (disabled). */
  sessionId: string | null;
  /** SQL of the FULL result (no view limit); null = nothing to export. */
  sql: string | null;
  /** Which statement of `sql` the result belongs to (multi-statement runs). */
  statementIndex?: number;
  /** Default file name without extension (table name, "result"). */
  fileBase: string;
  /** Full export re-runs the query (query tab) — reflected in the hint. */
  rerun?: boolean;
  className?: string;
}) {
  const showToast = useApp((s) => s.showToast);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<ExportFormat | null>(null);

  const loaded = result?.rows.length ?? 0;

  const copyLoaded = (label: string, make: (typeof COPY_FORMATS)[number]["make"]) => {
    if (!result) return;
    setOpen(false);
    void copyText(make(result.columns, result.rows)).then(
      (ok) => ok && showToast(`Copied ${loaded} row(s) as ${label}`, "info"),
    );
  };

  const exportAll = async (format: ExportFormat) => {
    if (!sessionId || !sql) return;
    setOpen(false);
    try {
      const path = await save({
        defaultPath: `${fileBase}.${format}`,
        filters: [
          { name: format.toUpperCase(), extensions: [format] },
          { name: "All files", extensions: ["*"] },
        ],
      });
      if (!path) return;
      setBusy(format);
      const out = await api.exportSql(sessionId, sql, statementIndex, format, path);
      showToast(
        out.truncated
          ? `Exported first ${out.rows.toLocaleString()} rows — XLSX sheet limit`
          : `Exported ${out.rows.toLocaleString()} row(s) → ${path.split("/").pop()}`,
        out.truncated ? "info" : "success",
      );
    } catch (e) {
      showToast(errText(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      panelClassName="w-52 p-1"
      trigger={
        <MenuButton
          className={className}
          title="Copy loaded rows or export the full result to a file"
          onClick={() => setOpen((v) => !v)}
        >
          {busy ? (
            <Loader2 size={11} className="animate-spin text-sky-400" />
          ) : (
            <FileDown size={11} className="text-sky-400/80" />
          )}
          Export
        </MenuButton>
      }
    >
      <SectionTitle>{`Copy loaded (${loaded})`}</SectionTitle>
      {COPY_FORMATS.map(({ label, make }) => (
        <Item
          key={label}
          label={label}
          disabled={!loaded}
          onClick={() => copyLoaded(label, make)}
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
          disabled={!sessionId || !sql || busy !== null}
          onClick={() => void exportAll(format)}
        />
      ))}
    </Popover>
  );
}
