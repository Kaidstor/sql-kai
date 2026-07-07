import { Loader2, RefreshCw } from "lucide-react";
import { isConnectionLost } from "../lib/api";
import { useApp } from "../lib/store";
import { Button, ErrorPre } from "./ui";

/** Error block for tab bodies; a dead-connection error also offers a
 *  Reconnect that re-dials the profile in place (tabs reload themselves). */
export function TabError({
  profileId,
  error,
}: {
  profileId: string;
  error: string;
}) {
  const reconnect = useApp((s) => s.reconnect);
  const busy = useApp((s) => Boolean(s.connecting[profileId]));
  if (!isConnectionLost(error)) return <ErrorPre>{error}</ErrorPre>;
  return (
    <div className="m-2 rounded-md border border-red-900/60 bg-red-950/40 p-3">
      <pre className="selectable font-mono text-[12px] whitespace-pre-wrap text-red-300">
        {error}
      </pre>
      <Button
        className="mt-2 border border-zinc-700 bg-zinc-900"
        disabled={busy}
        onClick={() => void reconnect(profileId)}
      >
        {busy ? (
          <Loader2 size={13} className="animate-spin" />
        ) : (
          <RefreshCw size={13} />
        )}
        Reconnect
      </Button>
    </div>
  );
}
