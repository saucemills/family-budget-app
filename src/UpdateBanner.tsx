import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/**
 * Checks GitHub Releases for a newer version when the app opens, and offers a
 * one-click install. Updates are signed; the app refuses any that aren't.
 * Your budget file lives outside the app folder, so updating never touches it.
 */
export default function UpdateBanner() {
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [status, setStatus] = useState<"idle" | "downloading" | "error">("idle");
  const [progress, setProgress] = useState<number | null>(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    // In development (npm run tauri dev) there's nothing to update to; ignore errors quietly.
    check()
      .then((u) => u && setUpdate(u))
      .catch((e) => console.info("Update check skipped:", e));
  }, []);

  const install = async () => {
    if (!update) return;
    setStatus("downloading");
    let total = 0;
    let done = 0;
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") total = event.data.contentLength ?? 0;
        if (event.event === "Progress") {
          done += event.data.chunkLength;
          if (total) setProgress(Math.round((done / total) * 100));
        }
      });
      await relaunch();
    } catch (e) {
      console.error(e);
      setStatus("error");
    }
  };

  return (
    <div className="update-box">
      {update && (
        <div className="update-available">
          <div className="update-title">Version {update.version} is available</div>
          {status === "idle" && (
            <button className="primary" onClick={install}>
              Update and restart
            </button>
          )}
          {status === "downloading" && <div className="muted">Downloading{progress != null ? ` ${progress}%` : "…"}</div>}
          {status === "error" && (
            <div className="muted">
              The update didn't install. <button className="link" onClick={install}>Try again</button>
            </div>
          )}
        </div>
      )}
      {version && <div className="version">Version {version}</div>}
    </div>
  );
}
