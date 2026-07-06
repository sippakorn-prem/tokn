import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { LogicalSize } from "@tauri-apps/api/dpi";
import tauriConf from "../src-tauri/tauri.conf.json";

/** Un-zoomed popover size — the config is the single source of truth. */
const { width: BASE_W, height: BASE_H } = tauriConf.app.windows[0];

export const ZOOM_MIN = 0.8;
export const ZOOM_MAX = 1.6;
const ZOOM_STEP = 0.1;
const STORAGE_KEY = "tk-zoom";

const clamp = (z: number) =>
  Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(z * 100) / 100));

function load(): number {
  const raw = Number(localStorage.getItem(STORAGE_KEY));
  return Number.isFinite(raw) && raw > 0 ? clamp(raw) : 1;
}

/** Scale both the webview content and the window so nothing clips. */
async function apply(z: number): Promise<void> {
  const win = getCurrentWebviewWindow();
  // Size first, then zoom: the window grows to fit before content enlarges,
  // avoiding a frame where the zoomed content is clipped.
  await win.setSize(new LogicalSize(Math.round(BASE_W * z), Math.round(BASE_H * z)));
  await win.setZoom(z);
}

export interface ZoomControls {
  zoom: number;
  zoomIn: () => void;
  zoomOut: () => void;
  reset: () => void;
  canIn: boolean;
  canOut: boolean;
}

/**
 * Popover zoom, driven by ⌘+ / ⌘- / ⌘0 and the footer control. The level is
 * persisted so the popover reopens at the user's chosen size.
 */
export function useZoom(): ZoomControls {
  const [zoom, setZoom] = useState(load);
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;

  const set = useCallback((next: number) => {
    const z = clamp(next);
    setZoom(z);
    localStorage.setItem(STORAGE_KEY, String(z));
    void apply(z);
  }, []);

  const zoomIn = useCallback(() => set(zoomRef.current + ZOOM_STEP), [set]);
  const zoomOut = useCallback(() => set(zoomRef.current - ZOOM_STEP), [set]);
  const reset = useCallback(() => set(1), [set]);

  // Re-apply the persisted zoom on mount so a fresh window launch honors it.
  // The window already launches at base size/zoom 1.0, so skip the IPC when
  // there's nothing to change (the common case).
  useEffect(() => {
    if (zoomRef.current !== 1) void apply(zoomRef.current);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      switch (e.key) {
        case "=":
        case "+":
          e.preventDefault();
          zoomIn();
          break;
        case "-":
        case "_":
          e.preventDefault();
          zoomOut();
          break;
        case "0":
          e.preventDefault();
          reset();
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [zoomIn, zoomOut, reset]);

  return {
    zoom,
    zoomIn,
    zoomOut,
    reset,
    canIn: zoom < ZOOM_MAX,
    canOut: zoom > ZOOM_MIN,
  };
}
