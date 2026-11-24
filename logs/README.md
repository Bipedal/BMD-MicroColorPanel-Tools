# Evidence: recordings

For programmers. Nothing here is needed to use the [helper](../docs/GUIDE.md).

The raw logs and USB captures cited in [`PANEL_PROTOCOL.md`](../PANEL_PROTOCOL.md) (JSONL, text, pcapng).

- The panel's serial number and Bluetooth address have been replaced by `X…` placeholders
  (or zero bytes in binary captures) everywhere, including the pcapng files. Placeholders
  have the same length, so the captures still open in Wireshark.
- Raw Bluetooth captures are not published; they contain pairing keys. The decoded GATT
  table (`logs/btgatt_pairing_*.txt`) is included.
- The USB captures contain only the panel's own traffic, recorded with USBPcap. Their
  file headers still name the capture machine's CPU and Windows build.
