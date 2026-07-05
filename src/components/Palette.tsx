import { useEffect, useMemo, useState } from "react";
import { accentColor } from "../lib/colors";
import { fuzzyScore, highlightRuns } from "../lib/fuzzy";
import { profileAddr, queryScopeOf, scopeLabelOf } from "../lib/profile";
import { sqlPreview } from "../lib/sql";
import { useApp } from "../lib/store";
import { cn, ColorDot, Overlay } from "./ui";

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
}

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
    if (!query.trim()) return items;
    return items
      .map((item) => ({ item, score: fuzzyScore(query, item.keywords) }))
      .filter((x): x is { item: PaletteItem; score: number } => x.score !== null)
      .sort((a, b) => b.score - a.score)
      .map((x) => x.item);
  }, [query, items]);

  useEffect(() => setSel(0), [query]);

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
                  "flex cursor-pointer items-center gap-2.5 rounded-md px-3 py-2",
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

/** ⌘O — fuzzy connection switcher; ⌘P — saved queries of the active collection. */
export function AppPalette() {
  const {
    palette,
    setPalette,
    profiles,
    sessions,
    queries,
    connect,
    selectProfile,
    activeProfileId,
    openSavedQuery,
  } = useApp();

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
      hint: sessions[p.id] ? "connected" : undefined,
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
