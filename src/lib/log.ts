// Frontend logging helpers. Some Tauri-bound promises are intentionally swallowed
// (best-effort UI hooks). This wrapper keeps them swallowed but visible in DevTools.
export function swallowWithLog(context: string) {
  return (err: unknown) => {
    // eslint-disable-next-line no-console
    console.warn(`[swallowed] ${context}`, err);
  };
}
