# Claude Data Viz

A desktop viewer for files that Claude Code writes. Watches folders you point it at, links every render back to the prompt that produced it, and shows them in a live-updating gallery.

Built with Tauri 2, React 19, and Rust. macOS-first; Linux/Windows should work but aren't tested.

## What it solves

You ask Claude to plot something. It writes `plot.png` somewhere. You go find it. You forget which prompt made which version. You repeat this fifty times in an afternoon.

This app sits in the corner of your screen and:

- Tails the folder Claude is writing into.
- Tails Claude Code's session transcript (`~/.claude/projects/*/*.jsonl`) in parallel.
- Joins the two streams: when a file appears, the app finds the prompt that triggered it.
- Renders the file (image, HTML, PDF) with the prompt as a header.

No setup beyond picking the folder. No Claude config changes. No metadata embedded in the files — works with any tool Claude calls (Write, Bash, matplotlib, Plotly, whatever).

## How the prompt matching works

The non-obvious part. Files don't carry their prompt — the app reconstructs the link from timestamps.

Each file's modification time is matched against tool calls in the JSONL transcript using a four-tier fallback (`src-tauri/src/transcript.rs:73-129`):

1. **Deterministic** — tool result completed within ±5s of the file's mtime *and* the tool's input or output mentions the path. Strongest signal.
2. **Timing-only** — tool result within ±5s, no path mention required. Catches tools that write quietly (matplotlib `savefig`, build scripts that don't echo filenames).
3. **Probable** — path is mentioned, timing is within ±30s of tool invocation. Tolerates flush delays.
4. **Fallback** — no tool match, but a user prompt landed within ±30s. Best-effort guess, no `tool_use_id` attached.

Each match returns the user prompt that preceded the tool call. The transcript watcher backfills the last 64KB of any newly-discovered session file so prompts written milliseconds before the watcher attached aren't lost.

## Supported files

| Kind | Renderer | Notes |
|---|---|---|
| PNG, JPG, WEBP, GIF, SVG | `<img>` in a pan/zoom wrapper | Pinch, wheel, drag, double-click, reset |
| HTML | sandboxed iframe (`allow-scripts allow-same-origin`) | Plotly, Bokeh, Altair, D3, Folium, etc. all run interactively |
| PDF | native `<embed>` | Browser PDF viewer's zoom/scroll |

Anything else (`.ipynb`, raw `.json` figures, etc.) is not currently supported.

## Quick start

Prereqs: Node 20+, Rust toolchain (for Tauri builds), and a working Claude Code install (the app reads `~/.claude/projects/`).

```bash
git clone git@github.com:palpen/claude-data-viz.git
cd claude-data-viz
npm install
npm run tauri dev
```

On first run, pick a folder Claude writes into — `/tmp/renders`, `~/Documents/claude-output`, whatever. Add more folders later from the top bar.

To build a release bundle:

```bash
npm run tauri build
```

The `.app` lands in `src-tauri/target/release/bundle/macos/`.

## Keybindings

| Key | Action |
|---|---|
| `1`–`9` | Jump to nth item in sidebar |
| `f` | Toggle "follow latest" |
| `Space` | Toggle fullscreen viewer |
| `⌘O` | Reveal selected file in Finder |
| `0` | Reset image zoom |
| `=` / `+` | Zoom in |
| `-` | Zoom out |

(Image-zoom keys are local to the viewer; they don't fire while typing in inputs.)

## Architecture, briefly

- **`src-tauri/src/fs_watcher.rs`** — `notify_debouncer_full`-based file watcher with cold scan (50 newest files in last 24h) + hot watch. Stable-size recheck filters out half-written files.
- **`src-tauri/src/transcript.rs`** — JSONL tailer + `TranscriptIndex`. Ring buffer of the last 64 tool calls, capped tool output at 16KB (tail-kept), 1-hour staleness filter on session files.
- **`src/store/vizStore.ts`** — Zustand store. Items keyed `${watch_id}::${abs_path}`, capped at 200 in memory (oldest evicted by mtime).
- **`src/components/Viewer.tsx`** — image viewer wrapping `react-zoom-pan-pinch`; HTML/PDF branches separate.
- **State persistence** — `~/Library/Application Support/com.pspenano.claude-data-viz/{prefs,viz-history}.json`. Atomic writes (write to `.tmp`, rename).

## Limitations / known gaps

- No marquee box-zoom (double-click-to-zoom covers most cases).
- No Jupyter notebook rendering.
- No raw Plotly/Vega JSON rendering — needs the wrapping HTML.
- The four-tier matcher is heuristic at the edges. If two files appear within ±30s of one prompt and only one tool ran, both get attributed to that tool. Rare in practice.
- macOS-tested only. On other OSes, the transcript path discovery and the prefs/history paths may need tweaking.

## Status

Personal project. Works for me. PRs and issues welcome but not promised attention.

## License

Unlicensed. Treat as all-rights-reserved until that changes.
