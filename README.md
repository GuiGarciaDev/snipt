# Snipt

A tiny Win+V-style popup for **your own** snippets: text, templates and links.
Press a shortcut, type a few letters, hit Enter, and the snippet is pasted where you were typing.

- **200 KB** single `.exe`. No installer, no runtime, no web view.
- **~2.5 MB** private memory while running.
- Pure Rust + Win32 + GDI, with a single dependency (`windows-sys`, plain API bindings). Icons come from the Segoe Fluent Icons font built into Windows.

## Usage

| Key | Action |
|---|---|
| `Ctrl+Shift+Space` | Open Snipt (tries `Win+Shift+V` first, then `Ctrl+Shift+Space`, then `Ctrl+Alt+V`) |
| type | Fuzzy search by name, or text inside the snippet |
| `↑` `↓` `Tab` `PgUp` `PgDn` | Navigate |
| `Enter` / click | Paste into the app you were using |
| `Shift+Enter` | Open a link in the browser instead of pasting it |
| `Ctrl+E` | Edit the selected snippet |
| `Ctrl+N` | New snippet (what you typed in the search becomes its title) |
| `Esc` | Clear the search, press again to close |

### Managing snippets

Click the **gear** in the bottom-right corner to switch to manage mode:

- **Enter** or a click edits a snippet, and **+ New** creates one.
- **Del** or the trash icon deletes a snippet after an inline confirmation.
- **Esc** or the gear goes back to normal mode.

The editor has a title, a type (URL, Template or Text) and the content. Template placeholders can be inserted with one click. URLs are validated before saving. Shortcuts in the editor: `Ctrl+Enter` saves, `Tab` switches fields, `Ctrl+T` changes the type, and `Esc` cancels. The editor stays open if you switch to another app, so you can copy text in from elsewhere.

The tray icon menu has **Manage snippets**, **Open snippets file**, **Start with Windows** and **Exit**.

## Snippets file

Everything is stored in `%APPDATA%\Snipt\snippets.txt`, a plain text file that's easy to back up or sync with git, and fine to edit by hand:

```text
@ GitHub profile [url]
https://github.com/you

@ Email sign-off
Best regards,
{cursor}
```

- Each `@ Name` line starts a snippet. The optional tag `[url]`, `[text]` or `[template]` sets its type; without it, Snipt detects the type.
- Variables: `{date}`, `{time}`, `{datetime}`, `{clipboard}` and `{cursor}` (where the cursor ends up after pasting).

## How it works

1. `RegisterHotKey` handles the global shortcut, and Snipt remembers which window had focus.
2. A borderless popup opens at the text cursor (or the mouse pointer) on the right monitor. Its height fits the number of snippets, it gets dark mode and rounded corners from DWM, and it is drawn double-buffered with GDI. Every clickable area is recorded while painting, so clicks always match what's on screen.
3. On Enter, Snipt gives focus back to that window, puts the snippet on the clipboard, sends `Ctrl+V` with `SendInput`, and moves the cursor for `{cursor}`.
4. 500 ms later your previous clipboard text is restored. Snippets are marked with `ExcludeClipboardContentFromMonitorProcessing`, so they don't fill up your Win+V history.
5. The editor uses native `EDIT` controls, styled dark, so undo, selection, IME and paste work as usual.

## Build

```bash
cargo build --release
```

The binary is `target/release/snipt.exe`. The release profile is tuned for size (`opt-level = "z"`, LTO, `panic = "abort"`, stripped).

## Known limitations

- Windows blocks pasting into apps running as administrator unless Snipt also runs as administrator.
- Only plain text is restored to the clipboard after pasting. If you had copied an image or rich text, the snippet stays on the clipboard instead.
- The shortcut can't be configured yet.
- Saving from the editor rewrites the file and drops `[url]`/`[template]` tags where Snipt would detect the same type anyway. Comments at the top of the file are kept.
