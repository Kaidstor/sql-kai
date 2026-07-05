import { Loader2, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { ACCENTS, accentColor } from "../lib/colors";
import { useApp } from "../lib/store";
import type { Profile } from "../lib/types";
import { Button, Field, IconBtn, Input, Overlay, cn } from "./ui";

interface FormState {
  name: string;
  group: string;
  color: string;
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
  sshPassphrase: string;
  clearSshPassphrase: boolean;
}

const emptyForm: FormState = {
  name: "",
  group: "",
  color: "",
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
  sshPassphrase: "",
  clearSshPassphrase: false,
};

function fromProfile(p: Profile): FormState {
  return {
    name: p.name,
    group: p.group ?? "",
    color: p.color ?? "",
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
    sshPassphrase: "",
    clearSshPassphrase: false,
  };
}

function toProfile(form: FormState, existing?: Profile): Profile {
  return {
    id: existing?.id ?? "",
    name: form.name.trim() || `${form.user}@${form.host}/${form.database}`,
    group: form.group.trim() || null,
    color: form.color || null,
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
        }
      : null,
  };
}

export function ConnectionDialog() {
  const { dialog, closeDialog, saveProfile, showToast, profiles } = useApp();
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

  // null = keep saved, "" = forget saved, non-empty = replace.
  const passwordArg = form.password
    ? form.password
    : form.clearPassword
      ? ""
      : null;
  const sshPassphraseArg = form.sshPassphrase
    ? form.sshPassphrase
    : form.clearSshPassphrase
      ? ""
      : null;

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
              <div className="relative">
                <Input
                  type="password"
                  value={form.password}
                  onChange={(e) => set({ password: e.target.value })}
                  placeholder={
                    form.clearPassword
                      ? "(removed on save)"
                      : editing?.hasPassword
                        ? "•••••• (keep saved)"
                        : ""
                  }
                  className={
                    editing?.hasPassword && !form.clearPassword && !form.password
                      ? "pr-6"
                      : undefined
                  }
                />
                {editing?.hasPassword &&
                  !form.clearPassword &&
                  !form.password && (
                    <button
                      type="button"
                      title="Forget saved password"
                      onClick={() => set({ clearPassword: true })}
                      className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-zinc-500 hover:text-red-400 transition-colors"
                    >
                      <X size={12} />
                    </button>
                  )}
              </div>
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
                  <div className="relative">
                    <Input
                      type="password"
                      value={form.sshPassphrase}
                      onChange={(e) => set({ sshPassphrase: e.target.value })}
                      placeholder={
                        form.clearSshPassphrase
                          ? "(removed on save)"
                          : editing?.hasSshPassphrase
                            ? "•••••• (keep saved)"
                            : "if the key is encrypted"
                      }
                      className={
                        editing?.hasSshPassphrase &&
                        !form.clearSshPassphrase &&
                        !form.sshPassphrase
                          ? "pr-6"
                          : undefined
                      }
                    />
                    {editing?.hasSshPassphrase &&
                      !form.clearSshPassphrase &&
                      !form.sshPassphrase && (
                        <button
                          type="button"
                          title="Forget saved passphrase"
                          onClick={() => set({ clearSshPassphrase: true })}
                          className="absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5 text-zinc-500 hover:text-red-400 transition-colors"
                        >
                          <X size={12} />
                        </button>
                      )}
                  </div>
                </Field>
              </div>
              <Field label="Identity file (optional)">
                <Input
                  value={form.sshKeyPath}
                  onChange={(e) => set({ sshKeyPath: e.target.value })}
                  placeholder="~/.ssh/id_ed25519"
                />
              </Field>
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
