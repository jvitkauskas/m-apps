# WIFI.TXT

The WiFi networks a unit may join, in a text file at the root of its SD card: `/WIFI.TXT`. Any firmware or app on an FM-1 or SMK-37 with a card can read it, so the networks are typed in once, on a computer or with m-apps' WiFi app, and every program that needs a network finds them in the same place.

```
# WiFi networks on this card. use= is the one to join.
use=Home

# at home
ssid=Home
password=correct horse

ssid=Workshop
password=shed-key-2041

ssid=Cafe
password=
```

## The file

- **Where:** `WIFI.TXT` in the root folder of the card's first FAT partition. Its short name is `WIFI.TXT`, so a program that reads only 8.3 names finds it.
- **Text:** UTF-8, with or without a byte order mark at the start. Lines end in LF or CRLF; the last one may have no end.
- **Lines:** each is blank, a comment, or a setting.
  - A **comment** starts with `#` as its first character.
  - A **setting** is `name=value`. The name is what comes before the first `=`, in lowercase. The value is everything after it, as it is: spaces at either end and further `=` are part of it, and nothing is trimmed or unquoted.

## Settings

| Setting | Means |
|---|---|
| `ssid=` | Starts a network: its name, 1 to 32 bytes, compared byte for byte. The lines after it, up to the next `ssid=`, are that network's. |
| `password=` | The network's WPA2 passphrase: 8 to 63 printable ASCII characters. Empty, or no `password=` line, means an open network. |
| `use=` | The name of the network to join. Without it, or if it names none of them, the first network. It can be anywhere in the file. |

A setting with a name not in this table is one a reader doesn't know. Settings that belong to a network, should one be added, go among that network's lines; settings for the whole file go before the first `ssid=`.

## Reading

- Skip blank lines, comments and settings you don't know.
- Skip a network whose name or password can't be (an empty name, more than 32 bytes, a zero byte, a password of 1 to 7 characters, or over 63). A `password=` before the first `ssid=` belongs to no network.
- A name given twice: the first one that can be joined counts, and the others take no place among the 12.
- `password=` given twice for a network: the last one counts. So does `use=` given twice.
- Take at least 12 networks and a 4 KiB file. m-apps' WiFi app takes 12 and up to 4096 bytes, and won't change a larger file. With more than 12 in the file, a network it adds goes at the end, so the next read may not take it.

## Writing

Someone may have written the file by hand, and another program may have added settings to it. So a program changing it changes only what it means to, and keeps everything else as it was:

- Keep comments, blank lines, settings you don't know, the byte order mark, each line's line end, and networks you skipped when reading.
- Change a password where it is. A network that had no `password=` line gets one just after its `ssid=`.
- Remove a network with its settings: its `ssid=`, `password=` and any others up to the next `ssid=`, and a blank line just before it, so adding a network and removing it leaves the file as it was. Other blank lines and comments stay. A name given twice goes everywhere it's given, or the next reader would find it again.
- Removing the last network can leave an empty file, which is a file with no networks.
- Set `use=` where it is. If there's none, add one before the first network.
- Add a new network at the end.
- Replace the file in one step, if you can: write the new text as a new file, then point the directory entry at it, so losing power leaves either the old file or the new one. m-apps' `kit/fat.rs` does this (`Volume::reserve`, `write`, `swap`).

`apps/wifi/wifilist.rs` in m-apps reads and writes it this way, and `tests/wifi_list_host.rs` shows what's kept.

## Keep in mind

The passwords are plain text: anyone with the card can read them.
