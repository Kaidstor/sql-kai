import { Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { ACCENTS, accentColor } from "../lib/colors";
import { useApp } from "../lib/store";
import type { Profile } from "../lib/types";
import { Button, Field, IconBtn, Input, Overlay, ProdBadge, cn } from "./ui";

interface FormState {
  name: string;
  group: string;
  color: string;
  production: boolean;
  host: string;
  port: string;
  database: string;
  user: string;
  password: string;
  /** Stages removal of the saved password (typing a new one overrides). */
  clearPassword: boolean;
  useSsh: boolean;
  sshHost: string;
  sshUser: string;
  sshPort: string;
  sshKeyPath: string;
  sshKeepalive: string;
  sshPassphrase: string;
  clearSshPassphrase: boolean;
}

const emptyForm: FormState = {
  name: "",
  group: "",
  color: "",
  production: false,
  host: "localhost",
  port: "5432",
  database: "",
  user: "postgres",
  password: "",
  clearPassword: false,
  useSsh: false,
  sshHost: "",
  sshUser: "",
  sshPort: "",
  sshKeyPath: "",
  sshKeepalive: "",
  sshPassphrase: "",
  clearSshPassphrase: false,
};

function fromProfile(p: Profile): FormState {
  return {
    name: p.name,
    group: p.group ?? "",
    color: p.color ?? "",
    production: Boolean(p.production),
    host: p.host,
    port: String(p.port),
    database: p.database,
    user: p.user,
    password: "",
    clearPassword: false,
    useSsh: Boolean(p.ssh?.host),
    sshHost: p.ssh?.host ?? "",
    sshUser: p.ssh?.user ?? "",
    sshPort: p.ssh?.port ? String(p.ssh.port) : "",
    sshKeyPath: p.ssh?.keyPath ?? "",
    sshKeepalive: p.ssh?.keepaliveInterval != null ? String(p.ssh.keepaliveInterval) : "",
    sshPassphrase: "",
    clearSshPassphrase: false,
  };
}

/** Secret argument for save/test: null = keep saved, "" = forget, else replace. */
const secretArg = (value: string, clear: boolean) => value || (clear ? "" : null);

/** Password input that can also stage forgetting the stored secret: while
 *  empty and a secret is saved, an ✕ appears that stages the removal. */
function SecretInput({
  value,
  hasSaved,
  cleared,
  emptyPlaceholder = "",
  clearTitle,
  onChange,
  onClear,
}: {
  value: string;
  /** A secret is stored in the vault for the profile being edited. */
  hasSaved: boolean;
  /** Removal is staged (applied on save). */
  cleared: boolean;
  emptyPlaceholder?: string;
  clearTitle: string;
  onChange: (value: string) => void;
  onClear: () => void;
}) {
  const canClear = hasSaved && !cleared && !value;
  return (
    <div className="relative">
      <Input
        type="password"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={
          cleared
            ? "(removed on save)"
            : hasSaved
              ? "•••••• (keep saved)"
              : emptyPlaceholder
        }
        className={canClear ? "pr-6" : undefined}
      />
      {canClear && (
        <button
          type="button"
          title={clearTitle}
          onClick={onClear}
          className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-zinc-500 hover:text-red-400 transition-colors"
        >
          <X size={12} />
        </button>
      )}
    </div>
  );
}

function toProfile(form: FormState, existing?: Profile): Profile {
  return {
    id: existing?.id ?? "",
    name: form.name.trim() || `${form.user}@${form.host}/${form.database}`,
    group: form.group.trim() || null,
    color: form.color || null,
    production: form.production,
    host: form.host.trim(),
    port: Number(form.port) || 5432,
    database: form.database.trim(),
    user: form.user.trim(),
    ssh: form.useSsh
      ? {
          host: form.sshHost.trim(),
          user: form.sshUser.trim() || null,
          port: form.sshPort ? Number(form.sshPort) : null,
          keyPath: form.sshKeyPath.trim() || null,
          keepaliveInterval: form.sshKeepalive.trim()
            ? Math.max(0, Math.trunc(Number(form.sshKeepalive)) || 0)
            : null,
        }
      : null,
  };
}

export function ConnectionDialog() {
  const dialog = useApp((s) => s.dialog);
  const closeDialog = useApp((s) => s.closeDialog);
  const saveProfile = useApp((s) => s.saveProfile);
  const showToast = useApp((s) => s.showToast);
  const profiles = useApp((s) => s.profiles);
  const knownGroups = [
    ...new Set(profiles.map((p) => p.group?.trim()).filter(Boolean)),
  ] as string[];
  const editing = dialog.profile;
  const [form, setForm] = useState<FormState>(emptyForm);
  const [testing, setTesting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);

  useEffect(() => {
    if (dialog.open) {
      setForm(editing ? fromProfile(editing) : emptyForm);
      setTestResult(null);
    }
  }, [dialog.open, editing]);

  if (!dialog.open) return null;

  const set = (patch: Partial<FormState>) => {
    setForm((f) => ({ ...f, ...patch }));
    setTestResult(null);
  };

  const passwordArg = secretArg(form.password, form.clearPassword);
  const sshPassphraseArg = secretArg(form.sshPassphrase, form.clearSshPassphrase);

  const onTest = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      const message = await api.testProfile(
        toProfile(form, editing),
        passwordArg,
        sshPassphraseArg,
      );
      setTestResult({ ok: true, message });
    } catch (e) {
      setTestResult({ ok: false, message: errText(e) });
    } finally {
      setTesting(false);
    }
  };

  const onSave = async () => {
    setSaving(true);
    try {
      await saveProfile(toProfile(form, editing), passwordArg, sshPassphraseArg);
    } catch (e) {
      showToast(errText(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Overlay onClose={closeDialog} className="items-center bg-black/60">
      <div className="w-130 max-h-[90vh] overflow-y-auto rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-800">
          <div className="text-[13px] font-semibold text-zinc-100">
            {editing ? "Edit connection" : "New connection"}
          </div>
          <IconBtn onClick={closeDialog}>
            <X size={15} />
          </IconBtn>
        </div>

        <div className="p-4 space-y-3">
          <div className="grid grid-cols-3 gap-3">
            <Field label="Name" className="col-span-2">
              <Input
                value={form.name}
                onChange={(e) => set({ name: e.target.value })}
                placeholder="prod / staging / local…"
                autoFocus
              />
            </Field>
            <Field label="Group (shared queries)">
              <Input
                list="profile-groups"
                value={form.group}
                onChange={(e) => set({ group: e.target.value })}
                placeholder="ms-search"
              />
              <datalist id="profile-groups">
                {knownGroups.map((g) => (
                  <option key={g} value={g} />
                ))}
              </datalist>
            </Field>
          </div>

          <div className="grid grid-cols-3 gap-3">
            <Field label="Host" className="col-span-2">
              <Input
                value={form.host}
                onChange={(e) => set({ host: e.target.value })}
                placeholder="localhost"
              />
            </Field>
            <Field label="Port">
              <Input
                value={form.port}
                onChange={(e) => set({ port: e.target.value })}
                placeholder="5432"
              />
            </Field>
          </div>

          <div className="grid grid-cols-3 gap-3">
            <Field label="Database">
              <Input
                value={form.database}
                onChange={(e) => set({ database: e.target.value })}
              />
            </Field>
            <Field label="User">
              <Input
                value={form.user}
                onChange={(e) => set({ user: e.target.value })}
              />
            </Field>
            <Field label="Password">
              <SecretInput
                value={form.password}
                hasSaved={Boolean(editing?.hasPassword)}
                cleared={form.clearPassword}
                clearTitle="Forget saved password"
                onChange={(password) => set({ password })}
                onClear={() => set({ clearPassword: true })}
              />
            </Field>
          </div>

          <Field label="Color (highlights the connection everywhere)">
            <div className="flex items-center gap-1.5 pt-0.5">
              <button
                type="button"
                title="Default theme"
                onClick={() => set({ color: "" })}
                className={cn(
                  "size-5 rounded-full border border-zinc-600 bg-zinc-925",
                  !form.color && "ring-2 ring-zinc-300",
                )}
              />
              {ACCENTS.map((c) => (
                <button
                  key={c}
                  type="button"
                  title={c}
                  onClick={() => set({ color: c })}
                  className={cn(
                    "size-5 rounded-full",
                    form.color === c && "ring-2 ring-zinc-300",
                  )}
                  style={{ background: accentColor(c)! }}
                />
              ))}
            </div>
          </Field>

          <label
            className="flex items-center gap-2 pt-1 text-[12px] text-zinc-300 cursor-pointer"
            title="INSERT/UPDATE/DELETE/DDL ask for confirmation; the UI is marked with a PROD badge"
          >
            <input
              type="checkbox"
              checked={form.production}
              onChange={(e) => set({ production: e.target.checked })}
              className="accent-red-600"
            />
            Production — confirm data-modifying queries
            {form.production && <ProdBadge />}
          </label>

          <label className="flex items-center gap-2 pt-1 text-[12px] text-zinc-300 cursor-pointer">
            <input
              type="checkbox"
              checked={form.useSsh}
              onChange={(e) => set({ useSsh: e.target.checked })}
              className="accent-sky-600"
            />
            Connect through SSH tunnel
          </label>

          {form.useSsh && (
            <div className="rounded-md border border-zinc-800 bg-zinc-950/50 p-3 space-y-3">
              <div className="grid grid-cols-3 gap-3">
                <Field label="SSH host / alias" className="col-span-2">
                  <Input
                    value={form.sshHost}
                    onChange={(e) => set({ sshHost: e.target.value })}
                    placeholder="my-server (from ~/.ssh/config) or 1.2.3.4"
                  />
                </Field>
                <Field label="SSH port">
                  <Input
                    value={form.sshPort}
                    onChange={(e) => set({ sshPort: e.target.value })}
                    placeholder="22"
                  />
                </Field>
              </div>
              <div className="grid grid-cols-2 gap-3">
                <Field label="SSH user (optional)">
                  <Input
                    value={form.sshUser}
                    onChange={(e) => set({ sshUser: e.target.value })}
                    placeholder="from ssh config"
                  />
                </Field>
                <Field label="Key passphrase (optional)">
                  <SecretInput
                    value={form.sshPassphrase}
                    hasSaved={Boolean(editing?.hasSshPassphrase)}
                    cleared={form.clearSshPassphrase}
                    emptyPlaceholder="if the key is encrypted"
                    clearTitle="Forget saved passphrase"
                    onChange={(sshPassphrase) => set({ sshPassphrase })}
                    onClear={() => set({ clearSshPassphrase: true })}
                  />
                </Field>
              </div>
              <div className="grid grid-cols-3 gap-3">
                <Field label="Identity file (optional)" className="col-span-2">
                  <Input
                    value={form.sshKeyPath}
                    onChange={(e) => set({ sshKeyPath: e.target.value })}
                    placeholder="~/.ssh/id_ed25519"
                  />
                </Field>
                <Field label="Keepalive, sec">
                  <Input
                    type="number"
                    min={0}
                    value={form.sshKeepalive}
                    onChange={(e) => set({ sshKeepalive: e.target.value })}
                    placeholder="15"
                    title="Ping the server after this many seconds when idle to prevent getting disconnected due to inactivity (ServerAliveInterval). Empty = 15, 0 = off."
                  />
                </Field>
              </div>
              <p className="text-[11px] leading-relaxed text-zinc-500">
                Auth uses your keys / ssh-agent / ~/.ssh/config (incl.
                ProxyJump); the passphrase is stored in the keychain. DB host
                above is resolved{" "}
                <span className="text-zinc-400">from the SSH server</span> — e.g.{" "}
                <code className="text-zinc-400">localhost:5432</code> for a DB
                on the same box.
              </p>
            </div>
          )}

          {testResult && (
            <div
              className={cn(
                "selectable rounded-md border px-3 py-2 text-[12px] whitespace-pre-wrap font-mono",
                testResult.ok
                  ? "border-emerald-900 bg-emerald-950/40 text-emerald-300"
                  : "border-red-900 bg-red-950/40 text-red-300",
              )}
            >
              {testResult.message}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between px-4 py-3 border-t border-zinc-800">
          <Button onClick={() => void onTest()} disabled={testing || saving}>
            {testing && <Loader2 size={13} className="animate-spin" />}
            Test connection
          </Button>
          <div className="flex items-center gap-2">
            <Button onClick={closeDialog} disabled={saving}>
              Cancel
            </Button>
            <Button
              variant="primary"
              onClick={() => void onSave()}
              disabled={saving || !form.host || !form.database || !form.user}
            >
              {saving && <Loader2 size={13} className="animate-spin" />}
              Save
            </Button>
          </div>
        </div>
      </div>
    </Overlay>
  );
}
