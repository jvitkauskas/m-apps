# SPDX-License-Identifier: MIT
"""tools/card_zip.py: a folder as a zip that Python's zipfile reads back the same, made the same way each time."""
import io
from pathlib import Path
import sys
import tempfile
import zipfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
from card_zip import make

with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    card = folder/'CARD'
    (card/'MBOOT'/'APPS'/'MP3').mkdir(parents=True)
    files = {'WIFI.TXT': b'ssid=Home\n', 'MBOOT/MENU.BIN': bytes(range(256))*4, 'MBOOT/APPS/MP3/BOOT.APP': b'x'*700}
    for name, data in files.items():
        (card/name).write_bytes(data)
    first = make(card)
    (card/'WIFI.TXT').touch()
    assert make(card) == first, 'the same files make the same bytes'
    with zipfile.ZipFile(io.BytesIO(first)) as z:
        assert z.namelist() == ['MBOOT/', 'MBOOT/APPS/', 'MBOOT/APPS/MP3/', 'MBOOT/APPS/MP3/BOOT.APP', 'MBOOT/MENU.BIN',
                                'WIFI.TXT'], z.namelist()
        assert all(i.date_time == (1980, 1, 1, 0, 0, 0) for i in z.infolist())
        assert z.testzip() is None
        for name, data in files.items():
            assert z.read(name) == data, name
    for bad in ('notes.txt', 'LONGNAME9.BIN', 'A.TEXT'):
        (card/bad).write_bytes(b'x')
        try:
            make(card)
            raise AssertionError(f'{bad} was taken')
        except ValueError:
            pass
        (card/bad).unlink()
print('card_zip OK')
