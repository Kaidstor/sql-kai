import { Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { accentColor } from "../lib/colors";
import { fuzzyScore, highlightRuns } from "../lib/fuzzy";
import { isKey } from "../lib/keys";
import { isMac } from "../lib/platform";
import { profileAddr, queryScopeOf, scopeLabelOf } from "../lib/profile";
import { sqlPreview } from "../lib/sql";
import { useApp } from "../lib/store";
import { cn, ColorDot, IconButton, Overlay } from "./ui";

/** Text with the fuzzy-matched characters highlighted. */
function Hl({ query, text }: { query: string; text: string }) {
  const runs = useMemo(() => highlightRuns(query, text), [query, text]);
  return (
    <>
      {runs.map((r, i) =>
        r.hit ? (
          <span key={i} className="text-sky-400">
            {r.text}
          </span>
        ) : (
          <span key={i}>{r.text}</span>
        ),
      )}
    </>
  );
}

interface PaletteItem {
  id: string;
  title: string;
  subtitle?: string;
  badge?: string;
  color?: string | null;
  /** Fuzzy haystack; group goes first so "m s dev" ranks ms-search/dev high. */
  keywords: string;
  hint?: string;
  action: () => void;
  /** Deletable item: hover shows a trash button, Ctrl+X removes the selected
   *  one. The callback owns confirmation — the palette stays open. */
  onDelete?: () => void;
}

/** Rows rendered at once — symbol lists reach thousands of items, and the
 *  list isn't virtualized; typing narrows it down. */
const RENDER_CAP = 100;

function PaletteModal({
  placeholder,
  items,
  onClose,
}: {
  placeholder: string;
  items: PaletteItem[];
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);

  const filtered = useMemo(() => {
    const all = query.trim()
      ? items
          .map((item) => ({ item, score: fuzzyScore(query, item.keywords) }))
          .filter(
            (x): x is { item: PaletteItem; score: number } => x.score !== null,
          )
          .sort((a, b) => b.score - a.score)
          .map((x) => x.item)
      : items;
    return all.slice(0, RENDER_CAP);
  }, [query, items]);

  useEffect(() => setSel(0), [query]);

  // deleting the last item leaves sel past the end — pull it back in range
  useEffect(() => {
    setSel((s) => Math.min(s, Math.max(filtered.length - 1, 0)));
  }, [filtered.length]);

  const run = (item?: PaletteItem) => {
    const target = item ?? filtered[sel];
    if (!target) return;
    onClose();
    target.action();
  };

  return (
    <Overlay onClose={onClose} className="items-start bg-black/50 pt-[14vh]">
      <div className="w-[34rem] max-w-[92vw] overflow-hidden rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <input
          autoFocus
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          value={query}
          placeholder={placeholder}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSel((s) => Math.min(s + 1, filtered.length - 1));
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              setSel((s) => Math.max(s - 1, 0));
            }
            if (e.key === "Enter") {
              e.preventDefault();
              run();
            }
            if (
              e.ctrlKey &&
              !e.metaKey &&
              !e.altKey &&
              !e.shiftKey &&
              isKey(e, "x")
            ) {
              // on Windows/Linux Ctrl+X is "cut" — keep it while text is selected
              const cut = e.currentTarget.selectionStart !== e.currentTarget.selectionEnd;
              const target = filtered[sel];
              if (!cut && target?.onDelete) {
                e.preventDefault();
                target.onDelete();
              }
            }
          }}
          className="w-full border-b border-zinc-800 bg-transparent px-4 py-3 text-[13px] text-zinc-100 outline-none placeholder:text-zinc-600"
        />
        <div className="max-h-80 overflow-y-auto p-1">
          {filtered.map((item, i) => {
            const color = accentColor(item.color);
            return (
              <div
                key={item.id}
                onMouseEnter={() => setSel(i)}
                onClick={() => run(item)}
                className={cn(
                  "group flex cursor-pointer items-center gap-2.5 rounded-md px-3 py-2",
                  i === sel && "bg-zinc-800",
                )}
              >
                {item.color !== undefined && <ColorDot color={color} />}
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 text-[13px] text-zinc-100">
                    <span className="truncate">
                      <Hl query={query} text={item.title} />
                    </span>
                    {item.badge && (
                      <span className="shrink-0 rounded border border-zinc-700 bg-zinc-800 px-1.5 py-px text-[10px] text-zinc-400">
                        <Hl query={query} text={item.badge} />
                      </span>
                    )}
                  </div>
                  {item.subtitle && (
                    <div className="truncate font-mono text-[11px] text-zinc-500">
                      <Hl query={query} text={item.subtitle} />
                    </div>
                  )}
                </div>
                {item.hint && (
                  <span className="shrink-0 text-[10px] text-zinc-600">
                    {item.hint}
                  </span>
                )}
                {item.onDelete && (
                  <IconButton
                    tabIndex={-1}
                    title={isMac ? "Delete ⌃X" : "Delete (Ctrl+X)"}
                    className="-my-1 opacity-0 group-hover:opacity-100"
                    onClick={(e) => {
                      e.stopPropagation();
                      item.onDelete?.();
                    }}
                  >
                    <Trash2 size={12} />
                  </IconButton>
                )}
              </div>
            );
          })}
          {filtered.length === 0 && (
            <div className="px-3 py-6 text-center text-[12px] text-zinc-600">
              nothing found
            </div>
          )}
        </div>
      </div>
    </Overlay>
  );
}

/** ⌘⌥O — fuzzy connection switcher; ⌘P — saved queries; ⌘T — tables/columns/
 *  functions of the active database. */
export function Palette() {
  const palette = useApp((s) => s.palette);
  const setPalette = useApp((s) => s.setPalette);
  const profiles = useApp((s) => s.profiles);
  const sessions = useApp((s) => s.sessions);
  const lost = useApp((s) => s.lost);
  const queries = useApp((s) => s.queries);
  const connect = useApp((s) => s.connect);
  const selectProfile = useApp((s) => s.selectProfile);
  const activeProfileId = useApp((s) => s.activeProfileId);
  const openSavedQuery = useApp((s) => s.openSavedQuery);
  const deleteQuery = useApp((s) => s.deleteQuery);
  const confirmDialog = useApp((s) => s.confirmDialog);
  const tables = useApp((s) => s.tables);
  const schemaColumns = useApp((s) => s.schemaColumns);
  const schemaFunctions = useApp((s) => s.schemaFunctions);
  const loadSchemaFunctions = useApp((s) => s.loadSchemaFunctions);
  const openTableTab = useApp((s) => s.openTableTab);
  const openQueryTab = useApp((s) => s.openQueryTab);

  // Functions are the only symbol source not already in memory — fetch once
  // per connection when the symbols palette first opens.
  useEffect(() => {
    if (palette === "symbols" && activeProfileId) {
      void loadSchemaFunctions(activeProfileId);
    }
  }, [palette, activeProfileId, loadSchemaFunctions]);

  if (!palette) return null;
  const close = () => setPalette(null);

  if (palette === "connections") {
    const connectedFirst = [...profiles].sort(
      (a, b) => Number(Boolean(sessions[b.id])) - Number(Boolean(sessions[a.id])),
    );
    const items: PaletteItem[] = connectedFirst.map((p) => ({
      id: p.id,
      title: p.name,
      badge: p.group?.trim() || undefined,
      subtitle: profileAddr(p),
      color: p.color ?? null,
      keywords: `${p.group ?? ""} ${p.name} ${p.database} ${p.host}`,
      hint: sessions[p.id]
        ? "connected"
        : lost[p.id]
          ? "connection lost — ⏎ reconnects"
          : undefined,
      action: () =>
        sessions[p.id] ? selectProfile(p.id) : void connect(p.id),
    }));
    return (
      <PaletteModal
        placeholder="Open connection…  (try: m s dev)"
        items={items}
        onClose={close}
      />
    );
  }

  if (palette === "symbols") {
    if (!activeProfileId || !sessions[activeProfileId]) {
      return (
        <PaletteModal placeholder="No active connection" items={[]} onClose={close} />
      );
    }
    const pid = activeProfileId;
    const items: PaletteItem[] = [
      ...(tables[pid] ?? []).map((t): PaletteItem => ({
        id: `t:${t.schema}.${t.name}`,
        title: t.schema === "public" ? t.name : `${t.schema}.${t.name}`,
        badge: t.kind === "table" ? undefined : t.kind,
        keywords: `${t.schema} ${t.name}`,
        hint: "⏎ open data",
        action: () => openTableTab(pid, t.schema, t.name),
      })),
      ...(schemaColumns[pid] ?? []).flatMap((t) =>
        t.columns.map(
          (col): PaletteItem => ({
            id: `c:${t.schema}.${t.table}.${col}`,
            title: `${t.table}.${col}`,
            badge: t.schema === "public" ? undefined : t.schema,
            subtitle: "column",
            keywords: `${t.schema} ${t.table} ${col}`,
            hint: "⏎ open table",
            action: () => openTableTab(pid, t.schema, t.table),
          }),
        ),
      ),
      ...(schemaFunctions[pid] ?? []).map((f): PaletteItem => {
        const qual = f.schema === "public" ? f.name : `${f.schema}.${f.name}`;
        return {
          id: `f:${f.schema}.${f.name}(${f.args})`,
          title: `${f.name}(${f.args})`,
          badge: f.schema === "public" ? undefined : f.schema,
          subtitle: "function",
          keywords: `${f.schema} ${f.name}`,
          hint: "⏎ query",
          action: () =>
            openQueryTab(pid, `SELECT ${qual}(${f.args ? `/* ${f.args} */` : ""});`, f.name),
        };
      }),
    ];
    return (
      <PaletteModal
        placeholder="Table, column or function…"
        items={items}
        onClose={close}
      />
    );
  }

  // queries palette for the active profile's collection + global
  const profile = profiles.find((p) => p.id === activeProfileId);
  const scopeKey = profile ? queryScopeOf(profile) : null;
  const scopeLabel = profile ? scopeLabelOf(profile) : "";
  const items: PaletteItem[] = (
    profile ? queries.filter((q) => !q.scope || q.scope === scopeKey) : []
  ).map((q) => ({
    id: q.id,
    title: q.name,
    badge: q.scope ? scopeLabel : "global",
    subtitle: sqlPreview(q.sql),
    keywords: `${q.name} ${q.sql}`,
    action: () => profile && openSavedQuery(profile.id, q),
    onDelete: async () => {
      const ok = await confirmDialog({
        title: `Delete saved query "${q.name}"?`,
        confirmLabel: "Delete",
        danger: true,
      });
      if (ok) void deleteQuery(q.id);
    },
  }));
  return (
    <PaletteModal
      placeholder={
        profile
          ? `Saved queries — ${scopeLabel} + global…`
          : "No active connection"
      }
      items={items}
      onClose={close}
    />
  );
}
