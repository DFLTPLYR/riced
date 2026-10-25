# Lua background example

From the repository root, after rebuilding riced:

```sh
riced -p ./config/riced/example
# Equivalent shorthand:
riced -c example
```

`-p` selects an existing project containing `config.lua`. `-c NAME` looks first
in `./config/riced/NAME`, then `$XDG_CONFIG_HOME/riced/NAME` (normally
`~/.config/riced/NAME`). Names allow letters, digits, `_`, and `-` only.
Neither option changes the process working directory. Libraries, themes, and
generated preferences use the selected project rather than your normal config.
Run only one riced daemon per session; IPC is still session-wide.

This is a working migration example, **not full parity with background.rs**:

- One background layer per output, ignoring bar exclusive zones.
- Right-click menu, wallpaper picker, local selection and native fade animation.
- Native file picker through `self:pick_file()`, using the same `rfd` backend as
  the legacy shell. No Zenity executable is required.
- Per-output JSON preferences in `apps/background/state/<hex-output-name>.json`.
  Edit `wallpaper` (absolute image path) or `color` externally to see live updates.
- Saves use conflict detection; “Reload / discard edits” explicitly resolves a
  conflict by discarding unsaved edits. Closing/reloading the app cancels work.

`background.disabled = true` suppresses legacy background creation at startup
(restart after changing it). That also removes the legacy desktop-widget host.
The sample uses `pointer_events = true` to opt into layer-local gestures; child
buttons consume their own input before background selection starts.

Selection is local to each output and ends on pointer exit. Cross-output drag,
legacy desktop widgets, measured menu clamping, native Add Top / Settings
commands, free-form wallpaper baking, cover-fit controls, and image-backed
wallpaper shaders are not migrated. Images currently use contain fitting.
Invalid preference types/colors are not an app-level schema: use valid strings.
The first frame uses defaults while preferences load asynchronously.

## Picker troubleshooting

Rebuild riced after adding the helper, then restart the example. The native
dialog uses the same backend as the existing shell picker. If it fails to open,
check the desktop portal/backend in your session and riced's terminal output.
Some backend failures are reported as dismissal by `rfd` rather than as an error.
