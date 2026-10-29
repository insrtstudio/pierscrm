import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

export type UpdateProgress =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "available"; update: Update }
  | { phase: "downloading"; downloaded: number; total: number | null }
  | { phase: "installing" }
  | { phase: "uptodate" }
  | { phase: "error"; message: string };

/** Check for an update. Returns the Update handle if one is available, else null.
 *  Never throws — network / dev-mode errors resolve to null (or throw only when
 *  `loud` is set, so the Settings button can surface a real message). */
export async function checkForUpdate(loud = false): Promise<Update | null> {
  try {
    const update = await check();
    return update && update.available ? update : null;
  } catch (e) {
    if (loud) throw e;
    return null;
  }
}

// Single-flight install shared by the launch banner and Settings, so the two
// can never download the same update concurrently.
let inflight: Promise<void> | null = null;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((l) => l());

export function isInstalling() {
  return inflight != null;
}

/** Subscribe to install start/stop (for useSyncExternalStore). */
export function subscribeInstalling(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function installAndRelaunch(
  update: Update,
  onProgress?: (p: UpdateProgress) => void
): Promise<void> {
  if (inflight) return inflight;
  inflight = (async () => {
    let downloaded = 0;
    let total: number | null = null;
    await update.downloadAndInstall((event) => {
      switch (event.event) {
        case "Started":
          total = event.data.contentLength ?? null;
          onProgress?.({ phase: "downloading", downloaded: 0, total });
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          onProgress?.({ phase: "downloading", downloaded, total });
          break;
        case "Finished":
          onProgress?.({ phase: "installing" });
          break;
      }
    });
    await relaunch();
  })();
  notify();
  // Only reached on failure in practice (success relaunches the app).
  inflight.catch(() => {}).finally(() => {
    inflight = null;
    notify();
  });
  return inflight;
}
