# Orbit plugins

A plugin is a folder with a `plugin.json` and the program it runs. The program can be written in any
language: an `.exe`, a PowerShell `.ps1`, a `.cmd`/`.bat` or a Python `.py`. Orbit starts it in its own
hidden process each time an action is picked. If it crashes or hangs, Orbit keeps running.

```
orbit --install-plugin path\to\plugin-folder      (or a .zip of it)
```

Installed plugins live in `%APPDATA%\Orbit\plugins\<id>\`. The tray menu's "Open plugins folder" opens it.
To uninstall a plugin, delete its folder. Nothing runs in the background: manifests are read only when
the ring opens, and plugin processes exist only while an action runs.

## plugin.json

```json
{
  "id": "checksum",
  "name": "Checksum",
  "version": "1.0.0",
  "description": "Copies the SHA-256 hash of the selected files to the clipboard.",
  "glyph": "EA86",
  "actions": [
    {
      "id": "sha256",
      "label": "Copy SHA-256",
      "glyph": "E8C8",
      "match": { "extensions": ["*"], "folders": false, "min": 1, "max": 100 },
      "command": "sha256.ps1",
      "args": ["{files}"],
      "mode": "all",
      "output": { "suffix": "_out", "ext": "" }
    }
  ]
}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | `[a-z0-9-]+`. Also used as the install folder name. |
| `name` | yes | Display name. Used for the sub-ring when several actions match. |
| `version`, `description` | no | Informational. |
| `glyph` | no | Sub-ring icon. Defaults to the puzzle piece (`EA86`). |
| `actions[].id` | yes | `[a-z0-9-]+`, unique within the plugin. The ring id is `plugin:<id>:<action-id>`. |
| `actions[].label` | yes | Short label shown on hover. |
| `actions[].glyph` | no | A Segoe Fluent Icons codepoint in hex (`"E8D2"`) or one literal character (`"Σ"`). |
| `actions[].match` | no | Which selections show the action (below). Defaults to any 1+ files. |
| `actions[].command` | yes | A file inside the plugin folder, an absolute path, or a tool on `PATH` (`"ffmpeg"`). |
| `actions[].args` | no | Arguments, with placeholders (below). |
| `actions[].mode` | no | `"each"` (default): one run per selected path. `"all"`: one run for the whole selection. |
| `actions[].output` | no | How `{out}` is named: `suffix` (default `"_out"`) and `ext` (default: same as the input). |

`match`: `extensions` is a list like `["png", "jpg"]` or `["*"]`, case-insensitive, and a leading dot is
fine. `folders: true` also accepts folders. `min`/`max` bound the number of selected items. **Every** selected
item must match, otherwise the action is not shown.

How the command is launched: `.ps1` with `powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File`,
`.cmd`/`.bat` through `cmd.exe` (with Rust's batch-safe argument escaping), and `.py` with `py` or
`python` from `PATH`. Anything else runs directly. The process is always hidden (no console window),
its working directory is the plugin folder, and stdin is empty.

## Placeholders

| Placeholder | Value (first file of the run) |
|---|---|
| `{file}` | Full path: `C:\Pics\cat.png` |
| `{files}` | All paths in this run. As an argument on its own, it becomes **one argument per file**. Inside a longer argument they are joined with spaces. |
| `{dir}` | Parent folder: `C:\Pics` |
| `{name}` / `{stem}` / `{ext}` | `cat.png` / `cat` / `png` |
| `{out}` | A free output path next to the input: `C:\Pics\cat<suffix>.<ext>`. Orbit adds ` (2)`, ` (3)`… instead of overwriting anything. |
| `{plugin_dir}` | The plugin's install folder. |

In `each` mode a run has exactly one file. Values are substituted in a single pass, so a path that
happens to contain `{out}` is never expanded again.

## Protocol (stdout)

Lines your program prints to stdout are read as they arrive. Orbit understands three kinds of line
and ignores everything else, so ordinary logging is harmless:

```
progress: 0.42        fraction 0..1 of this run (a comma decimal is accepted too)
message: 3 pages read  one short line for the ring centre; the last one wins
reveal: C:\out\a.txt   a file or folder to select in Explorer afterwards (relative = plugin folder)
```

- Any `{out}` path that exists after the run is revealed automatically.
- In `each` mode, Orbit combines the progress of each run into one overall bar.
- Exit code 0 means success. With a non-zero exit code the action fails and the ring shows the last
  non-empty line of stderr, or `Plugin failed (exit N)` if stderr was empty. In `each` mode the
  first failure stops the remaining files.
- With no `message:` line, the ring shows "Done".

## Example: checksum (ships in `examples-plugins/checksum`)

`orbit --install-plugin examples-plugins\checksum` adds "Copy SHA-256" for any file. It runs
`Get-FileHash`, copies the hash (or `hash  name` lines for several files) with `Set-Clipboard`, and prints
`message: SHA-256 copied`.

## How future plugins plug in

Heavy features ship as plugins so the core stays small. Each one is a manifest plus a program.

**OCR** (one text file per image):
```json
{ "id": "ocr", "label": "Extract text", "glyph": "E8D2",
  "match": { "extensions": ["png", "jpg", "jpeg", "tif", "bmp"] },
  "command": "ocr.exe", "args": ["{file}", "{out}"], "output": { "suffix": "_text", "ext": "txt" } }
```
`ocr.exe` writes the text to `{out}` and prints `message: 312 words`. Orbit selects the `.txt`.

**Background removal** (a Python model, all images in one process so the model loads once):
```json
{ "id": "remove-bg", "label": "Remove background", "glyph": "E7C5",
  "match": { "extensions": ["png", "jpg", "jpeg", "webp"], "max": 50 },
  "command": "remove_bg.py", "args": ["{files}"], "mode": "all" }
```
The script prints `progress:` after each image and `reveal: <path>` for each `*_nobg.png` it writes.

**AI tools** (summarise a document with a local model, or a cloud API the user configured):
```json
{ "id": "summarize", "label": "Summarize", "glyph": "E8BD",
  "match": { "extensions": ["pdf", "docx", "txt", "md"], "max": 1 },
  "command": "summarize.cmd", "args": ["{file}", "{out}"], "output": { "suffix": "_summary", "ext": "md" } }
```
Clipboard tools work like the checksum example. Image generation can use `"folders": true`
with `"extensions": []` and write its results into `{file}` (the folder).

A plugin with several actions that match the same selection appears as one node, named after the plugin,
that opens a sub-ring.

## Trust model

- Plugins are ordinary local programs that run with the user's own permissions. Install only plugins
  you trust, as you would any `.exe`.
- A plugin gets onto a machine only when the user runs `orbit --install-plugin` or copies a folder into
  the plugins folder. Orbit never downloads, updates or fetches anything. A plugin that wants the network
  makes that call itself.
- When installing, Orbit checks the manifest and rejects zip entries that would escape the target
  folder (zip-slip). It also checks that every relative command exists inside the plugin and has no `..`.
  A new version replaces the old one by swapping folders, so a failed install leaves the old version untouched.
- Plugins run out of process: a crash or a bad exit code becomes an error message on the ring, never a
  crash of Orbit.
