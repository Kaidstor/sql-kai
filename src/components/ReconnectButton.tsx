import { Loader2, RefreshCw } from "lucide-react";
import { useApp } from "../lib/store";
import { cn } from "./ui";

/** Re-dials a dropped profile in place. Owns the "am I reconnecting?" read and
 *  the spinner/refresh-icon swap so the three call sites (tab error, table
 *  stale banner, sidebar) don't each reimplement them; each supplies its own
 *  button chrome via `className`. */
export function ReconnectButton({
  profileId,
  className,
  iconSize = 13,
  iconClassName,
  label = "Reconnect",
  busyLabel = "Reconnecting…",
}: {
  profileId: string;
  className?: string;
  iconSize?: number;
  iconClassName?: string;
  label?: string;
  busyLabel?: string;
}) {
  const reconnect = useApp((s) => s.reconnect);
  const busy = useApp((s) => Boolean(s.connecting[profileId]));
  return (
    <button
      type="button"
      className={cn("inline-flex items-center gap-1.5", className)}
      disabled={busy}
      onClick={() => void reconnect(profileId)}
    >
      {busy ? (
        <Loader2 size={iconSize} className={cn("animate-spin", iconClassName)} />
      ) : (
        <RefreshCw size={iconSize} className={iconClassName} />
      )}
      {busy ? busyLabel : label}
    </button>
  );
}
