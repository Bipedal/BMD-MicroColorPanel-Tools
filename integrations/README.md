# The released program

The source of `micropanel.exe` as it is in the [releases](https://github.com/Bipedal/BMD-MicroColorPanel-Tools/releases):
the [helper](../helper) with its part for Lightroom Classic. To *use* it, see the [user guide](../docs/GUIDE.md).

| Folder | |
|---|---|
| `app/` | puts the helper and its integrations together; icon and version details |
| `lightroom/` | Lightroom Classic: actions, places, lights, the link to the [plugin](../lightroom), and the plugin's installer |

`cargo build --release` here makes `target/release/micropanel(.exe)`. How integrations work:
[`INTEGRATIONS.md`](../helper/INTEGRATIONS.md).
