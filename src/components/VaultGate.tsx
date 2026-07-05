import { Fingerprint, Loader2, Lock, ShieldAlert, ShieldPlus } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { errText } from "../lib/api";
import { useApp } from "../lib/store";
import { Button, Field, Input } from "./ui";

// Auto-prompt Touch ID only once per app launch: on start it's welcome, but
// right after an explicit Lock it would feel like the app fighting the user.
let autoPrompted = false;

/** Gates the whole app behind the secret vault: first run sets a master
 *  password, later runs unlock with it (or Touch ID once enrolled). Nothing
 *  (profiles, sessions) loads until the vault is open. */
export function VaultGate({ children }: { children: ReactNode }) {
  const { vault, vaultError } = useApp();

  if (vaultError) return <ErrorScreen message={vaultError} />;
  // init() hasn't reported the vault state yet — avoid flashing a wrong screen.
  if (!vault) return null;
  if (vault.unlocked) return <>{children}</>;
  return vault.exists ? <UnlockScreen /> : <SetupScreen />;
}

function Shell({
  icon,
  title,
  subtitle,
  children,
}: {
  icon: ReactNode;
  title: string;
  subtitle: string;
  children: ReactNode;
}) {
  return (
    <div className="h-screen flex items-center justify-center bg-zinc-950 text-zinc-200 antialiased">
      <div className="w-90 rounded-lg border border-zinc-800 bg-zinc-900 p-6 shadow-2xl">
        <div className="flex flex-col items-center gap-2 pb-5 text-center">
          <div className="flex size-11 items-center justify-center rounded-full border border-zinc-700 bg-zinc-925 text-sky-400">
            {icon}
          </div>
          <div className="text-[14px] font-semibold text-zinc-100">{title}</div>
          <div className="text-[12px] leading-relaxed text-zinc-500">
            {subtitle}
          </div>
        </div>
        {children}
      </div>
    </div>
  );
}

function BiometricCheckbox({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label className="flex items-center gap-2 text-[12px] text-zinc-300 cursor-pointer">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="accent-sky-600"
      />
      Unlock with Touch ID next time
    </label>
  );
}

function ErrorScreen({ message }: { message: string }) {
  const init = useApp((s) => s.init);
  const [busy, setBusy] = useState(false);

  return (
    <Shell
      icon={<ShieldAlert size={20} />}
      title="Vault unavailable"
      subtitle="The secret vault state could not be read."
    >
      <div className="space-y-3">
        <div className="selectable rounded-md border border-red-900 bg-red-950/40 px-3 py-2 font-mono text-[12px] whitespace-pre-wrap text-red-300">
          {message}
        </div>
        <Button
          variant="primary"
          className="w-full justify-center py-1.5"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            void init().finally(() => setBusy(false));
          }}
        >
          {busy && <Loader2 size={13} className="animate-spin" />}
          Retry
        </Button>
      </div>
    </Shell>
  );
}

function SetupScreen() {
  const setupVault = useApp((s) => s.setupVault);
  const supported = useApp((s) => s.vault?.biometricsSupported ?? false);
  const [pw, setPw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [enableBio, setEnableBio] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const mismatch = confirm.length > 0 && pw !== confirm;
  const canSubmit = pw.length > 0 && pw === confirm && !busy;

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      await setupVault(pw, supported && enableBio);
    } catch (e) {
      setError(errText(e));
      setBusy(false);
    }
  };

  return (
    <Shell
      icon={<ShieldPlus size={20} />}
      title="Set a master password"
      subtitle="It encrypts every saved connection password and SSH passphrase. There is no recovery if you forget it."
    >
      <div className="space-y-3">
        <Field label="Master password">
          <Input
            type="password"
            value={pw}
            onChange={(e) => setPw(e.target.value)}
            autoFocus
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
        </Field>
        <Field label="Confirm password">
          <Input
            type="password"
            value={confirm}
            onChange={(e) => setConfirm(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
        </Field>
        {supported && (
          <BiometricCheckbox checked={enableBio} onChange={setEnableBio} />
        )}
        {mismatch && (
          <div className="text-[11px] text-amber-400">
            Passwords don't match.
          </div>
        )}
        {error && <div className="text-[11px] text-red-400">{error}</div>}
        <Button
          variant="primary"
          className="w-full justify-center py-1.5"
          disabled={!canSubmit}
          onClick={() => void submit()}
        >
          {busy && <Loader2 size={13} className="animate-spin" />}
          Create vault
        </Button>
      </div>
    </Shell>
  );
}

function UnlockScreen() {
  const unlockVault = useApp((s) => s.unlockVault);
  const unlockVaultBiometric = useApp((s) => s.unlockVaultBiometric);
  const supported = useApp((s) => s.vault?.biometricsSupported ?? false);
  const enrolled = useApp((s) => s.vault?.biometricsEnrolled ?? false);
  const [pw, setPw] = useState("");
  const [enableBio, setEnableBio] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fails, setFails] = useState(0);
  const bioBusy = useRef(false);

  const bioUnlock = async () => {
    if (bioBusy.current) return;
    bioBusy.current = true;
    setBusy(true);
    setError(null);
    try {
      await unlockVaultBiometric();
    } catch (e) {
      const message = errText(e);
      // A dismissed system prompt is a choice, not a failure to report.
      if (message !== "cancelled") setError(message);
      setBusy(false);
      bioBusy.current = false;
    }
  };

  useEffect(() => {
    if (enrolled && !autoPrompted) {
      autoPrompted = true;
      void bioUnlock();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const submit = async () => {
    if (!pw || busy) return;
    setBusy(true);
    setError(null);
    try {
      await unlockVault(pw, supported && !enrolled && enableBio);
    } catch (e) {
      setError(errText(e));
      setFails((f) => f + 1);
      setPw("");
      setBusy(false);
    }
  };

  return (
    <Shell
      icon={<Lock size={20} />}
      title="Unlock the vault"
      subtitle="Your saved connection secrets are decrypted once for this session."
    >
      <div className="space-y-3">
        {enrolled && (
          <>
            <Button
              variant="primary"
              className="w-full justify-center py-1.5"
              disabled={busy}
              onClick={() => void bioUnlock()}
            >
              {busy && <Loader2 size={13} className="animate-spin" />}
              {!busy && <Fingerprint size={14} />}
              Unlock with Touch ID
            </Button>
            <div className="flex items-center gap-2 text-[10px] text-zinc-600">
              <div className="h-px flex-1 bg-zinc-800" />
              or use the master password
              <div className="h-px flex-1 bg-zinc-800" />
            </div>
          </>
        )}
        <Field label="Master password">
          <Input
            type="password"
            value={pw}
            onChange={(e) => setPw(e.target.value)}
            autoFocus={!enrolled}
            onKeyDown={(e) => e.key === "Enter" && void submit()}
          />
        </Field>
        {supported && !enrolled && (
          <BiometricCheckbox checked={enableBio} onChange={setEnableBio} />
        )}
        {error && (
          <div className="selectable text-[11px] whitespace-pre-wrap text-red-400">
            {error}
          </div>
        )}
        {fails >= 3 && (
          <p className="text-[11px] leading-relaxed text-zinc-500">
            Forgot the master password? There is no recovery — delete{" "}
            <code className="text-zinc-400">vault.json</code> from the app's
            Application Support folder to start over (saved passwords are
            lost, connections stay).
          </p>
        )}
        <Button
          variant={enrolled ? "ghost" : "primary"}
          className={
            enrolled
              ? "w-full justify-center py-1.5 border border-zinc-700"
              : "w-full justify-center py-1.5"
          }
          disabled={!pw || busy}
          onClick={() => void submit()}
        >
          {busy && !bioBusy.current && (
            <Loader2 size={13} className="animate-spin" />
          )}
          Unlock
        </Button>
      </div>
    </Shell>
  );
}
