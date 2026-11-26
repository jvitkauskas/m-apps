m-apps: a first SD card for the M-VAVE FM-1 and SMK-37
=======================================================

Programs that run from the SD card, without changing the firmware on the unit.
The same files work on an FM-1 and on an SMK-37.

You need
- A unit with an m-card board (https://jvitkauskas.github.io/m-apps/card/),
  or a card slot wired by hand.
- m-bootloader 2.3 or newer on it, 3.0 being the latest:
  https://github.com/jvitkauskas/m-bootloader
- An SD card formatted FAT32.

Setting up the card
1. Copy the MBOOT folder and WIFI.TXT to the root of the card. This
   README.TXT can go too, or not.
2. For WiFi, open WIFI.TXT in a text editor and put in your network's name
   and password. You can also add networks later with the WiFi app.
3. Put the card in the unit.

Starting it
Hold PLAY (FM-1) or SEQ PLAY (SMK-37) while switching the unit on. In
m-bootloader's menu, SD CARD is at the top: choose it, and the launcher shows
the apps.

The apps
- MP3 Player: plays the MP3 files in the card's root.
- Slots: keeps firmware and its settings on the card, and swaps between them.
- Unbricker: repairs a broken FM-1 or SMK-37 with this unit and a USB-C cable.
- USB Disk: shows the card to a computer as a USB drive, to copy files.
- WiFi: keeps the networks in WIFI.TXT, and tests them.

Source, licences and news: https://github.com/jvitkauskas/m-apps
Each app's licences are in the NOTICE.TXT in its folder, the launcher's in
MBOOT/NOTICE.TXT.
