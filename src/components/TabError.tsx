import { ReconnectButton } from "./ReconnectButton";
import { ErrorPre } from "./ui";

/** Error block for tab bodies; a dead-connection error (`lost`, from the
 *  backend's error code) also offers a Reconnect that re-dials the profile
 *  in place (tabs reload themselves). */
export function TabError({
  profileId,
  error,
  lost,
}: {
  profileId: string;
  error: string;
  lost?: boolean;
}) {
  if (!lost) return <ErrorPre>{error}</ErrorPre>;
  return (
    <div className="m-2 rounded-md border border-red-900/60 bg-red-950/40 p-3">
      <pre className="selectable font-mono text-[12px] whitespace-pre-wrap text-red-300">
        {error}
      </pre>
      <ReconnectButton
        profileId={profileId}
        className="mt-2 rounded-md border border-zinc-700 bg-zinc-900 px-2.5 py-1 text-[12px] font-medium text-zinc-300 transition-colors hover:bg-zinc-800 disabled:opacity-40 disabled:pointer-events-none"
      />
    </div>
  );
}
