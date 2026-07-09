import { Bookmark, BookmarkPlus, Globe, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { queryScopeOf, scopeLabelOf } from "../lib/profile";
import { sqlPreview } from "../lib/sql";
import { useApp, type QueryTabState, type Tab } from "../lib/store";
import type { SavedQuery } from "../lib/types";
import { Button, IconBtn, Input, MenuBtn, Popover } from "./ui";

/** "Save query" button: name + target collection (group/database or global).
 *  Open state lives in the store so ⌘S on an unsaved query can pop it too. */
export function SaveQueryButton({ tab }: { tab: Tab }) {
  const profiles = useApp((s) => s.profiles);
  const queries = useApp((s) => s.queries);
  const saveQuery = useApp((s) => s.saveQuery);
  const linkQueryTab = useApp((s) => s.linkQueryTab);
  const saveDialogFor = useApp((s) => s.saveDialogFor);
  const setSaveDialogFor = useApp((s) => s.setSaveDialogFor);
  const state = tab.state as QueryTabState;
  const open = saveDialogFor === tab.id;
  const [name, setName] = useState("");
  const [global, setGlobal] = useState(false);

  const linked = state.savedQueryId
    ? queries.find((q) => q.id === state.savedQueryId)
    : undefined;

  // propose a name: the linked query's, or the tab title unless it's "Query N"
  useEffect(() => {
    if (!open) return;
    setName(linked?.name ?? (/^Query \d+$/.test(tab.title) ? "" : tab.title));
    setGlobal(linked ? !linked.scope : false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const profile = profiles.find((p) => p.id === tab.profileId);
  if (!profile) return null;
  const scopeKey = queryScopeOf(profile);
  const scopeLabel = scopeLabelOf(profile);

  const overwrites = queries.some(
    (q) => q.name === name.trim() && (q.scope ?? null) === (global ? null : scopeKey),
  );

  const close = () => setSaveDialogFor(null);
  const submit = async () => {
    const saved = await saveQuery(
      name.trim(),
      state.sql,
      global ? null : scopeKey,
    );
    if (saved) {
      linkQueryTab(tab.id, saved);
      close();
      setName("");
    }
  };

  return (
    <Popover
      open={open}
      onClose={close}
      panelClassName="w-72 p-3 space-y-2.5"
      trigger={
        <IconBtn
          title="Save query ⌘S"
          disabled={!state.sql.trim()}
          onClick={() => setSaveDialogFor(open ? null : tab.id)}
        >
          <BookmarkPlus size={14} />
        </IconBtn>
      }
    >
      <div className="text-[11px] font-semibold tracking-wider text-zinc-500">
        SAVE QUERY
      </div>
      <Input
        autoFocus
        placeholder="Query name…"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && name.trim()) void submit();
        }}
      />
      <div className="space-y-1 text-[12px] text-zinc-300">
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="radio"
            checked={!global}
            onChange={() => setGlobal(false)}
            className="accent-sky-600"
          />
          <Bookmark size={12} className="text-sky-500" />
          <span className="truncate">
            {profile.group?.trim() ? `Collection “${scopeLabel}”` : scopeLabel}
          </span>
        </label>
        <label className="flex cursor-pointer items-center gap-2">
          <input
            type="radio"
            checked={global}
            onChange={() => setGlobal(true)}
            className="accent-sky-600"
          />
          <Globe size={12} className="text-emerald-500" />
          Global — all databases
        </label>
      </div>
      {overwrites && (
        <div className="text-[11px] text-amber-400">
          Overwrites the existing query with this name.
        </div>
      )}
      <div className="flex justify-end gap-2 pt-0.5">
        <Button onClick={close}>Cancel</Button>
        <Button variant="primary" disabled={!name.trim()} onClick={() => void submit()}>
          Save
        </Button>
      </div>
    </Popover>
  );
}

function QueryRow({
  query,
  onOpen,
  onDelete,
}: {
  query: SavedQuery;
  onOpen: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className="group flex cursor-pointer items-center gap-2 rounded px-2 py-1 hover:bg-zinc-800"
      onClick={onOpen}
      title={query.sql}
    >
      <div className="min-w-0 flex-1">
        <div className="truncate text-[12px] text-zinc-200">{query.name}</div>
        <div className="truncate font-mono text-[10px] text-zinc-600">
          {sqlPreview(query.sql, 80)}
        </div>
      </div>
      <IconBtn
        title="Delete saved query"
        className="opacity-0 group-hover:opacity-100"
        onClick={(e) => {
          e.stopPropagation();
          if (confirm(`Delete saved query "${query.name}"?`)) onDelete();
        }}
      >
        <X size={12} />
      </IconBtn>
    </div>
  );
}

/** Dropdown listing the profile's collection + the global one. */
export function SavedQueriesMenu({ tab }: { tab: Tab }) {
  const profiles = useApp((s) => s.profiles);
  const queries = useApp((s) => s.queries);
  const deleteQuery = useApp((s) => s.deleteQuery);
  const openSavedQuery = useApp((s) => s.openSavedQuery);
  const [open, setOpen] = useState(false);

  const profile = profiles.find((p) => p.id === tab.profileId);
  if (!profile) return null;
  const scopeKey = queryScopeOf(profile);
  const scopeLabel = scopeLabelOf(profile);

  const scoped = queries.filter((q) => q.scope === scopeKey);
  const global = queries.filter((q) => !q.scope);

  const openSaved = (q: SavedQuery) => {
    setOpen(false);
    openSavedQuery(tab.profileId, q);
  };

  const section = (title: ReactNode, items: SavedQuery[]) => (
    <div className="py-1">
      <div className="flex items-center gap-1.5 px-2 pt-1 pb-0.5 text-[10px] font-semibold tracking-wider text-zinc-500">
        {title}
      </div>
      {items.length === 0 ? (
        <div className="px-2 py-1 text-[11px] italic text-zinc-600">empty</div>
      ) : (
        items.map((q) => (
          <QueryRow
            key={q.id}
            query={q}
            onOpen={() => openSaved(q)}
            onDelete={() => void deleteQuery(q.id)}
          />
        ))
      )}
    </div>
  );

  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      panelClassName="w-80 max-h-96 overflow-y-auto p-1"
      trigger={
        <MenuBtn title="Saved queries" onClick={() => setOpen((v) => !v)}>
          <Bookmark size={13} />
          Saved
        </MenuBtn>
      }
    >
      {section(
        <>
          <Bookmark size={10} className="text-sky-500" />
          {(profile.group?.trim() ? `COLLECTION ${scopeLabel}` : scopeLabel).toUpperCase()}
        </>,
        scoped,
      )}
      <div className="mx-1 border-t border-zinc-800" />
      {section(
        <>
          <Globe size={10} className="text-emerald-500" />
          GLOBAL
        </>,
        global,
      )}
    </Popover>
  );
}
