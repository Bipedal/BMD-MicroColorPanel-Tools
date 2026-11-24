Micro Color Panel helper
========================

Lets the DaVinci Resolve Micro Color Panel control other programs on Windows 10 and 11.
Not affiliated with or endorsed by Blackmagic Design or Adobe.

1. Extract this folder to a place where it can stay, e.g. C:\Tools\MicroPanel.
2. Connect the panel by USB, or pair it over Bluetooth in Windows' settings.
3. Start micropanel.exe. The mapping editor opens in your browser. Later you find it behind the icon next
   to the clock (Windows may hide it behind the ^): "Edit mappings...". Starting micropanel.exe again opens
   it too.

Windows may say that it protected your PC: the program isn't signed. "More info" > "Run anyway" starts it.
The zip's .sha256 file on the download page shows whether yours is the one GitHub built.

Lightroom Classic
-----------------
In the editor: Settings > Install the plugin. Then restart Lightroom.
By hand: copy the folder MicroPanel.lrplugin to %APPDATA%\Adobe\Lightroom\Modules.

Please
------
- Never hold AUTO COLOR and STOP together: that resets the panel and clears its Bluetooth pairing.
- Keep Windows' own driver for the panel. Never install WinUSB (Zadig): DaVinci Resolve needs Windows' own.
- While DaVinci Resolve runs, the helper gives the panel to it.

Updating: choose Quit from the icon's menu, replace micropanel.exe, start it. If the editor then says the
Lightroom plugin is out of date: Settings > Replace the plugin.
Removing: Settings > Remove the plugin (if you use Lightroom); untick "Start with Windows" in the icon's menu;
Quit; delete this folder and %APPDATA%\MicroPanel (your layouts).

Full instructions: https://github.com/Bipedal/BMD-MicroColorPanel-Tools/blob/main/docs/GUIDE.md
