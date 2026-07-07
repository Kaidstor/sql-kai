import { Check, Download, RotateCw, TriangleAlert, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useUpdater } from "../lib/updater";
import { cn } from "./ui";

/**
 * Floating pill (bottom-left) announcing a new version. Two steps:
 * "Update to vX" → download (%) → "Restart to Update". The × dismisses it,
 * but the restart step resurfaces even if the earlier prompt was dismissed.
 * A manual "Check for Updates…" (app menu) also gets a transient pill here
 * when there is nothing to install ("You're up to date" / check failed).
 */
export function UpdateToast() {
  const {
    update,
    downloading,
    progress,
    ready,
    error,
    manualCheck,
    install,
    restart,
  } = useUpdater();
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    if (ready) setDismissed(false);
  }, [ready]);

  // A manual check that found an update resurfaces a dismissed prompt;
  // "up to date" / failure notices auto-hide after a few seconds.
  useEffect(() => {
    if (!manualCheck) return;
    if (manualCheck.status === "found") {
      setDismissed(false);
      return;
    }
    const t = setTimeout(() => useUpdater.setState({ manualCheck: null }), 4000);
    return () => clearTimeout(t);
  }, [manualCheck]);

  if (manualCheck && manualCheck.status !== "found") {
    const failed = manualCheck.status === "error";
    return (
      <div
        className={cn(
          "fixed bottom-9 left-3 z-50 flex items-center gap-1.5 rounded-lg px-3 py-1.5",
          "text-[12px] font-medium shadow-lg ring-1 ring-inset",
          failed
            ? "bg-red-950 text-red-200 shadow-red-950/40 ring-red-500/30"
            : "bg-zinc-800 text-zinc-200 shadow-black/40 ring-white/10",
        )}
        title={failed ? manualCheck.message : undefined}
      >
        {failed ? <TriangleAlert size={13} /> : <Check size={13} />}
        {failed ? "Update check failed" : "You're up to date"}
      </div>
    );
  }

  if (!update || dismissed) return null;

  const onAction = () => {
    if (ready) void restart();
    else if (!downloading) void install();
  };

  const label = ready ? (
    "Restart to Update"
  ) : downloading ? (
    <>
      Downloading…
      <span className="w-[4ch] text-left tabular-nums">{progress ?? 0}%</span>
    </>
  ) : (
    `Update to v${update.version}`
  );
  const Icon = ready ? RotateCw : Download;

  return (
    <div
      className={cn(
        "fixed bottom-9 left-3 z-50 flex items-stretch overflow-hidden rounded-lg",
        "bg-sky-600 text-[12px] font-medium text-white",
        "shadow-lg shadow-sky-950/40 ring-1 ring-inset ring-white/15",
      )}
    >
      <button
        onClick={onAction}
        disabled={downloading}
        title={
          error
            ? `Update failed: ${error}`
            : ready
              ? "Restart to apply the update"
              : `Download v${update.version} and update`
        }
        className={cn(
          "flex items-center gap-1.5 py-1.5 pr-3 pl-3 transition-colors",
          "hover:bg-sky-500 disabled:cursor-default disabled:opacity-90",
        )}
      >
        <Icon size={13} className={downloading ? "animate-pulse" : undefined} />
        {label}
      </button>
      <div className="w-px bg-white/20" />
      <button
        onClick={() => setDismissed(true)}
        title="Dismiss"
        className="flex items-center px-2 text-white/80 transition-colors hover:bg-sky-500 hover:text-white"
      >
        <X size={13} />
      </button>
    </div>
  );
}
