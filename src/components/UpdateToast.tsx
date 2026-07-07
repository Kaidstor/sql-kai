import { Download, RotateCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useUpdater } from "../lib/updater";
import { cn } from "./ui";

/**
 * Floating pill (bottom-left) announcing a new version. Two steps:
 * "Update to vX" → download (%) → "Restart to Update". The × dismisses it,
 * but the restart step resurfaces even if the earlier prompt was dismissed.
 */
export function UpdateToast() {
  const { update, downloading, progress, ready, error, install, restart } =
    useUpdater();
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    if (ready) setDismissed(false);
  }, [ready]);

  if (!update || dismissed) return null;

  const onAction = () => {
    if (ready) void restart();
    else if (!downloading) void install();
  };

  const label = ready
    ? "Restart to Update"
    : downloading
      ? `Downloading… ${progress ?? 0}%`
      : `Update to v${update.version}`;
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
