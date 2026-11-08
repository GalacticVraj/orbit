# Orbit

**The fastest way to act on files.** Select files in Explorer, press **Alt+Q**, and a glass command ring blooms
around the cursor. Pick an action and it runs locally, right away. There's no cloud, no account and no telemetry.

## Install & use

1. Run `Orbit-Setup.exe` and choose **Yes**. It installs per-user to `%LOCALAPPDATA%\Programs\Orbit` (no admin
   rights needed), starts with Windows, adds a Start-menu entry and lives in the tray.
2. Select one or more files in Explorer or on the desktop, then press **Alt+Q**. You can also right-click and use
   *Show more options → Orbit*.
3. To pick an action, click it, use the arrow keys or Tab and press Enter, or press **1–9**. **Esc** or Backspace goes back or closes the ring.
4. To uninstall, use *Settings → Apps → Orbit*. Your plugins and the rename undo history in `%APPDATA%\Orbit` are kept.

Command-line flags: `--portable` (run without installing), `--install`, `--uninstall`, `--install-plugin <folder|zip>`, `--quit`, `--ring <paths…>`.

## Actions

| Selection | Actions |
|---|---|
| PDF | Compress (images downsampled and re-encoded, streams compressed, unused objects pruned) · Split Pages · Extract Images · Convert to Word · Merge PDFs (2 or more) |
| Word | Convert to PDF · Extract Images |
| PowerPoint | Convert to PDF · Export Slides → PNG / JPG |
| Images | Convert → JPG / PNG / WEBP · Resize → 50% / 1920 / 1280 / 800 px · Compress |
| Videos | Extract Audio (AAC) · Compress Video (H.264) · Convert to MP4 |
| Folders | Zip · Calculate Size |
| Multiple files | Every per-file action runs as a batch · Zip · Calculate Size · **Batch Rename** |
| Anything | Smart Rename · Undo Rename (after a rename) · installed plugins |

Outputs are written next to the source and never overwrite anything (`name (2).ext`). Once an action finishes,
Orbit selects the result in the Explorer window you were using.

**Smart Rename** is rule-based, with a live preview. Pattern `Vacation_###` gives `Vacation_001.jpg`, … and the
tokens `{name}`, `{date}` and `{ext}` are available. It also offers find → replace, case, sorting by natural name
or date, and a smart default (`IMG_1024.jpg → Image_001.jpg`). Swaps such as A↔B are safe, a failure rolls back
the whole batch, and **Undo** is one ring click away.

## Phase 1: tech stack decision

Score from 1 to 5 (higher is better):

| Stack | RAM | Startup | Native look | Animation | Win integration | Verdict |
|---|---|---|---|---|---|---|
| **Rust + Win32 + Direct2D/DirectWrite** | 5 (≈2 MB private idle) | 5 | 5 (we draw Fluent ourselves) | 5 (springs, vsync via DwmFlush) | 5 (direct COM) | **Chosen** |
| C++ Win32 | 5 | 5 | 5 | 5 | 5 | Same result, with less memory safety and slower iteration |
| WinUI 3 | 2 (60–100 MB) | 2 | 5 | 4 | 3 | Too heavy for a resident utility |
| WPF | 2 (50–80 MB) | 2 | 3 | 4 | 3 | Over the RAM budget; .NET JIT startup |
| Avalonia | 2 | 2 | 2 | 4 | 2 | Not native; heavy |
| Qt | 3 | 3 | 2 | 4 | 3 | Large runtime; not native-looking |
| Tauri | 2 (WebView2 ≈ 80 MB+) | 3 | 2 | 4 | 2 | A browser engine for a 400 px ring |
| Electron | 1 | 1 | 1 | 4 | 2 | Exactly what Orbit must not be |

## Architecture

```
orbit/
├─ src/main.rs            Host: single instance, Alt+Q hotkey, tray, CLI, dispatch to worker threads
├─ crates/
│  ├─ core/               Shared types: Action, Outcome, Bitmap, Kind, safe output paths
│  ├─ shell/              Explorer: selection (incl. Win11 tabs and the desktop), thumbnails, reveal, context menu
│  ├─ ring/               The ring: layered window, D2D renderer, glass backdrop, springs, input, task states
│  ├─ engine/             Conversions: pdf.rs (lopdf) · img.rs (image) · office.rs (Word/PowerPoint COM)
│  │                      · video.rs (Media Foundation) · files.rs (zip, size, docx media)
│  ├─ rename/             Smart Rename: engine.rs (rules, two-phase apply, undo journal) · ui.rs (Mica window)
│  ├─ plugins/            Out-of-process plugins: manifest discovery, sandboxed launch, install from folder/zip
│  └─ setup/              Per-user install/uninstall, autostart, .ico + procedural logo
├─ examples-plugins/checksum/   Example plugin ("Copy SHA-256")
├─ docs/PLUGINS.md        Plugin format, protocol, trust model
├─ build.ps1              Build / package script
└─ .cargo/config.toml     GNU-toolchain linker workaround (see below)
```

**Event-driven, end to end.** At idle the host thread is blocked in `GetMessageW`. There are no timers, polling,
hooks, file watchers or indexing. Alt+Q comes in as `WM_HOTKEY`, and Orbit reads the selection from the
foreground Explorer over COM in about 3 ms. The ring renders frames only while a spring is moving (render →
`DwmFlush` → post the next frame), and goes back to sleep as soon as everything settles. Actions run on a worker
thread inside `catch_unwind`, so a bad file shows an error on the ring instead of crashing Orbit.

**Explorer right-click.** A per-user `HKCU\Software\Classes\{*,Directory}\shell\Orbit` verb starts
`orbit.exe --ring "%1"`. That process forwards its paths to the running instance through `WM_COPYDATA`, and the
host collects multi-select launches for 250 ms before opening a single ring.

**Plugins.** Each plugin is a folder holding a `plugin.json` plus any executable or script (`.exe`, `.ps1`,
`.cmd`, `.py`). Plugins run in their own process, so they can be written in any language, a crash can't take
Orbit down, and an uninstalled plugin costs nothing. Progress, messages and output files come back through
`progress:` / `message:` / `reveal:` lines on stdout. See [docs/PLUGINS.md](docs/PLUGINS.md). Install one with
`orbit --install-plugin examples-plugins\checksum`.

## Build

You need [rustup](https://rustup.rs) with the GNU host. No Visual Studio is required.

```powershell
rustup-init -y --default-host x86_64-pc-windows-gnu --profile minimal
.\build.ps1                      # debug build → target\debug\orbit.exe
.\build.ps1 -Release             # release build → dist\Orbit-Setup.exe + dist\Orbit-portable.zip
.\build.ps1 -Cargo test --workspace
.\build.ps1 -Cargo run -p orbit-ring --example demo        # live ring playground
.\build.ps1 -Cargo run -p orbit-ring --example snapshot    # render every ring state to target\snapshots
```

**Installer.** `Orbit-Setup.exe` is the release exe itself: run from anywhere, it offers to install, copies
itself, and registers autostart, the shortcut, the context menu and an Apps & Features entry. Everything is
per-user. Because the GNU toolchain has no `windres`, the exe carries no embedded icon. Shortcuts, Apps & Features
and the context menu use the `orbit.ico` written at install time, and the tray icon is drawn in code.

Two toolchain notes:
- `windows` is pinned to 0.58. It links through prebuilt import libraries, whereas 0.59+ uses raw-dylib, which
  needs binutils `as`, and that isn't shipped with the GNU toolchain.
- `.cargo/config.toml` switches `getrandom` to its `windows_legacy` backend for the same reason.

**Smart App Control:** with SAC on, Windows blocks unsigned binaries, including Orbit and every test build. To
ship to SAC-enabled PCs, sign the release with a publicly trusted certificate (e.g. Azure Trusted Signing).

## Optimization report

All figures were measured on this machine with the release build (Windows 11 26340).

| Metric | Target | Measured |
|---|---|---|
| Idle CPU | 0% | **0 ms CPU over 10 s** (blocked in `GetMessageW`) |
| Idle RAM | < 50 MB | **0.1–4 MB working set, ≈15 MB private** |
| Startup (launch → ready) | < 300 ms | **59–120 ms** (first launch of a new build ≈330 ms while Defender scans it) |
| Alt+Q → ring visible | < 100 ms | **51–91 ms** warm; **≈135 ms** for the first open after launch |
| Compress a 1080p JPEG (end to end) | — | 224 ms from keypress to file on disk |
| Exe size | — | 3.2 MB (1.5 MB zipped) |

What makes it fast:
- **Native draw path.** The ring is a layered window drawn by Direct2D into a premultiplied DIB. It has no
  framework, no XAML and no web view.
- **Cheap glass.** The blur is a screen capture downsampled to ¼ size, run through three box-blur passes and
  given a saturation lift. That costs a few ms per open instead of a GPU effect chain.
- **Warm-up after launch.** Right after the message loop starts, Orbit does the one-time setup (window,
  D2D/DirectWrite, fonts, shell image factory) and then trims its working set. That cut the first ring open from
  232 to 135 ms.
- **Thumbnails from Explorer's cache.** The centre icon only reads thumbnails Explorer has already cached
  (`SIIGBF_INCACHEONLY`). If none exists it falls back to the file icon rather than decoding the image on the hotkey path.
- **Trimming after every close.** Per-show bitmaps are freed when the ring closes and
  `K32EmptyWorkingSet` hands the pages back.
- **Lazy plugins.** Plugin manifests are read when you press Alt+Q, never watched in the background.
- **Parallel PDF work.** PDF image recompression is parallelised with rayon, and unused-object pruning uses a
  linear graph walk instead of lopdf's O(n²) one.

Known limits:
- The first open after launch is ~35 ms over target. That time is DLL paging and the first cross-process
  Explorer call.
- About 15 thread-pool threads stay parked after the warm-up. They use no CPU.

## Limitations

- **Office.** Word/PDF/PowerPoint conversions need Microsoft Office, with LibreOffice as the fallback. The
  LibreOffice path has only been compiled, not run, because LibreOffice isn't installed here.
- **Video.** Video uses Windows Media Foundation, with ffmpeg as the fallback for formats it can't open. The
  ffmpeg path has only been compiled, not run.
- **Image formats.** WEBP output is lossless only, a limit of the `image` crate. Animated GIFs keep only their first frame.
- **Context menu.** On Windows 11 the Orbit entry sits under *Show more options*. See the roadmap.
- **No cancel.** A running action can't be cancelled yet; Esc only hides the ring while the action finishes.

## Roadmap

1. **Signed releases.** Use Azure Trusted Signing so Orbit runs with Smart App Control on and SmartScreen stays quiet.
2. **Windows 11 top-level menu.** An `IExplorerCommand` handler in a sparse MSIX package would put "Orbit" in
   the modern right-click menu.
3. **Cancel and queue.** Let you cancel a running action, and queue several jobs with a compact progress pill.
4. **Settings.** Configurable hotkey, pinned and reordered actions, and default resize/quality values.
5. **Lossy WEBP and AVIF** output through an optional plugin, so the core stays small.
6. **Plugins.** OCR (Windows.Media.Ocr, offline), clipboard tools, background removal (local ONNX model) and
   image generation, all through the existing plugin protocol.
7. **Accessibility.** UI Automation for the ring, so screen readers announce nodes and results.
8. **Per-action icons in the ring centre**, e.g. a live page-count preview for PDFs.
