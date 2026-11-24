# Evidence: test scripts

For programmers. Nothing here is needed to use the [helper](../docs/GUIDE.md).

The standalone scripts that produced the recordings in [`logs/`](../logs) and so the statements in
[`PANEL_PROTOCOL.md`](../PANEL_PROTOCOL.md). Run them from the repository's main folder, e.g. `python tests/dump_caps.py`,
with DaVinci Resolve and the helper closed. Most guided tests end on a panel gesture: the same key pressed five times
within three seconds.

`test_lua_*.py` are different: they test the Lightroom plugin, without Lightroom or a panel (`pip install lupa`).
