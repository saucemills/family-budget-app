import { useCallback, useEffect, useRef, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

/** How often to look for a new version while the app stays open. */
const RECHECK_MS = 4 * 60 * 60 * 1000;

type CheckState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "up_to_date"; at: Date }
  | { kind: "failed"; message: string };

/**
 * Looks for a newer version on GitHub Releases when the app opens, every few hours,
 * and whenever "Check for updates" is clicked, then offers a one-click install.
 * Updates are signed; the app refuses any that aren't. Your budget file lives outside
 * the app folder, so updating never touches it.
 */
export default function UpdateBanner() {
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [checkState, setCheckState] = useState<CheckState>({ kind: "idle" });
  const [install, setInstall] = useState<"idle" | "downloading" | "error">("idle");
  const [progress, setProgress] = useState<number | null>(null);
  const busy = useRef(false);

  const runCheck = useCallback(async (manual: boolean) => {
    if (busy.current) return;
    busy.current = true;
    if (manual) setCheckState({ kind: "checking" });
    try {
      const u = await check();
      if (u) {
        setUpdate(u);
        setCheckState({ kind: "idle" });
      } else {
        setCheckState({ kind: "up_to_date", at: new Date() });
      }
    } catch (e) {
      // Automatic checks fail quietly (no internet, or running from source);
      // a click always gets an answer.
      console.info("Update check failed:", e);
      if (manual) setCheckState({ kind: "failed", message: String(e) });
    } finally {
      busy.current = false;
    }
  }, []);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    runCheck(false);
    const timer = window.setInterval(() => runCheck(false), RECHECK_MS);
    return () => window.clearInterval(timer);
  }, [runCheck]);

  const installUpdate = async () => {
    if (!update) return;
    setInstall("downloading");
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
      setInstall("error");
    }
  };

  return (
    <div className="update-box">
      {update ? (
        <div className="update-available">
          <div className="update-title">Version {update.version} is available</div>
          {install === "idle" && (
            <button className="primary" onClick={installUpdate}>
              Update and restart
            </button>
          )}
          {install === "downloading" && <div className="muted">Downloading{progress != null ? ` ${progress}%` : "…"}</div>}
          {install === "error" && (
            <div className="muted">
              The update didn't install.{" "}
              <button className="link" onClick={installUpdate}>
                Try again
              </button>
            </div>
          )}
        </div>
      ) : (
        <div className="update-check">
          <button className="link" onClick={() => runCheck(true)} disabled={checkState.kind === "checking"}>
            {checkState.kind === "checking" ? "Checking for updates…" : "Check for updates"}
          </button>
          {checkState.kind === "up_to_date" && (
            <div className="muted">
              You're up to date{version ? ` (version ${version})` : ""}. Checked {checkState.at.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}.
            </div>
          )}
          {checkState.kind === "failed" && (
            <div className="muted" title={checkState.message}>
              Couldn't check for updates. Are you online? <span className="detail">{checkState.message}</span>
            </div>
          )}
        </div>
      )}
      {version && <div className="version">Version {version}</div>}
    </div>
  );
}
