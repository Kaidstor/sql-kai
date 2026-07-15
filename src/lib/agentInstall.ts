// Managed install of npm-based ACP adapters. `npx -y pkg` on every start
// depends on the npm registry being reachable *right now* — on a flaky
// network (or broken IPv6, where npm's HTTP stack hangs without
// happy-eyeballs) the panel spins on "starting…" forever. Instead adapters
// are installed once into <appData>/acp/<providerId> with visible progress
// and an activity timeout, and then spawned from disk with no network at all.
import { appDataDir, join } from "@tauri-apps/api/path";
import { invoke } from "@tauri-apps/api/core";
import { RawProc } from "./acp";

export interface NpmAdapter {
  pkg: string;
  /** Pinned known-good version — bumped with app releases. */
  version: string;
  /** Executable name in node_modules/.bin. */
  bin: string;
}

/** Node child processes get IPv4-first DNS: npm's fetch stack (and some
 * agents) don't do happy-eyeballs and hang forever on networks where IPv6
 * black-holes (VPNs). Plain order change — IPv6-only setups still work. */
export const NODE_NET_ENV = { NODE_OPTIONS: "--dns-result-order=ipv4first" };

/** Kill `npm install` when it prints nothing for this long. */
const INSTALL_IDLE_MS = 60_000;

/** In-flight installs per provider — two profiles must not race one dir. */
const installs = new Map<string, Promise<string>>();

async function installDir(providerId: string): Promise<string> {
  return join(await appDataDir(), "acp", providerId);
}

/** Path of the adapter binary, or null when (this version) isn't installed. */
async function installedBin(dir: string, a: NpmAdapter): Promise<string | null> {
  const version = await invoke<string | null>("acp_installed_version", {
    dir,
    pkg: a.pkg,
  });
  if (version !== a.version) return null;
  return join(dir, "node_modules", ".bin", a.bin);
}

/**
 * Returns the adapter's executable path, installing it first when missing.
 * `onProgress` receives raw npm output lines (shown under the spinner).
 */
export function ensureAdapter(
  providerId: string,
  a: NpmAdapter,
  onProgress: (line: string) => void,
): Promise<string> {
  const inflight = installs.get(providerId);
  if (inflight) return inflight;

  const run = (async () => {
    const dir = await installDir(providerId);
    const existing = await installedBin(dir, a);
    if (existing) return existing;

    onProgress(`installing ${a.pkg}@${a.version}…`);
    const tail: string[] = [];
    let lastActivity = Date.now();
    const proc = await RawProc.spawn(
      "npm",
      [
        "install",
        "--prefix",
        dir,
        "--no-fund",
        "--no-audit",
        "--no-progress",
        // короткие сетевые таймауты + ретраи: на флапающей сети лучше три
        // быстрых попытки, чем одна вечная
        "--fetch-timeout=30000",
        "--fetch-retries=3",
        "--fetch-retry-mintimeout=3000",
        "--loglevel=http",
        `${a.pkg}@${a.version}`,
      ],
      NODE_NET_ENV,
      dir,
      (line) => {
        lastActivity = Date.now();
        tail.push(line);
        if (tail.length > 20) tail.shift();
        onProgress(line);
      },
    );

    // activity-based watchdog: молчание дольше INSTALL_IDLE_MS = завис
    const watchdog = setInterval(() => {
      if (Date.now() - lastActivity > INSTALL_IDLE_MS) proc.kill();
    }, 5_000);
    const code = await proc.done.finally(() => clearInterval(watchdog));

    if (code !== 0) {
      const silence = Date.now() - lastActivity > INSTALL_IDLE_MS;
      throw new Error(
        (silence
          ? `npm install stalled (no output for ${INSTALL_IDLE_MS / 1000}s) — check your network.`
          : `npm install failed (code ${code ?? "?"}).`) +
          (tail.length ? `\n${tail.slice(-6).join("\n")}` : ""),
      );
    }
    const bin = await installedBin(dir, a);
    if (!bin) {
      throw new Error(`npm install finished but ${a.pkg}@${a.version} is not in ${dir}`);
    }
    return bin;
  })();

  installs.set(providerId, run);
  return run.finally(() => installs.delete(providerId));
}
