/**
 * Display preferences this device keeps across launches, such as the zoom and which sidebar
 * sections are open, in the webview's localStorage. They are optional: when storage is unavailable,
 * a read finds nothing and a save does nothing.
 */
export function readPreference(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function savePreference(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // The preference is optional when storage is unavailable.
  }
}
