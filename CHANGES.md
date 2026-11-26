# Changes

## Since v0.1.0 (not released yet)

**Layers work differently.** In a layer, a control the layer doesn't set now does nothing, so a layer only does what you
gave it. Keys that open or close layers always work. To keep the old behaviour for a layer, tick **Unset controls keep
their normal job** next to its name (in the settings file: `[layers]` `name = { inherit = true }`). The starting
layout's layers keep their normal jobs.

Editor
- The apps on the left are a tree with their parts and tools; above the drawing a path shows where a control falls back to.
- Drag a control onto another to move it (a knob with its press), to swap, or with Ctrl to copy; rest on a layer key or a
  place on the way to drop it there.
- Layers can be renamed.
- The binding panel is more compact; its search and filters stay in view while the list scrolls.
- A knob whose turn changes a setting offers to make its press reset it, also when the press is unused.
- Open editor pages reload themselves after the helper is updated.

Helper
- Fixed: the tray icon, editor and panel could stop reacting when the helper's own tray menu was in front.
- Restart: tray menu **Restart**, and **Settings > Restart the helper** (one or two keys held together; works even when
  the helper hangs). A hang is now noted in the log.

Lightroom Classic (plugin 0.3.7: **Settings > Replace the plugin**, then restart Lightroom)
- Snapshots: next, previous, and a new one named with the time.
- Remove tool: use generative AI on/off.
- Shortcuts: paste settings from previous; copy after to before, before to after, swap; new snapshot (asks for a name).
- Fixed: names with some accented letters, and decimals on systems with a comma as decimal sign.

## v0.1.0

First release.
