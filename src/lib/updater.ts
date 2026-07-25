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

export async function installAndRelaunch(
  update: Update,
  onProgress?: (p: UpdateProgress) => void
) {
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
}
