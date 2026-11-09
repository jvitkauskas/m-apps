# SPDX-License-Identifier: MIT
"""tools/app_tar.py: a folder as a tar that Python's tarfile reads back the same, made the same way each time."""
import io
from pathlib import Path
import sys
import tarfile
import tempfile
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tools'))
from app_tar import make

with tempfile.TemporaryDirectory() as folder:
    folder = Path(folder)
    app = folder/'APP'
    (app/'DATA').mkdir(parents=True)
    files = {'BOOT.APP': bytes(range(256))*5, 'META.TXT': b'name=Test\nversion=1.0\n', 'DATA/SAMPLES.BIN': b'x'*512}
    for name, data in files.items():
        (app/name).write_bytes(data)
    first = make(app)
    (app/'META.TXT').touch()
    assert make(app) == first, 'the same files make the same bytes'
    # each file's header and data in whole blocks, the folder's header, then the two zero blocks and nothing more
    assert len(first) == 512*(3+3+1+1+1) + 1024, len(first)
    assert first[-1024:] == bytes(1024)
    with tarfile.open(fileobj=io.BytesIO(first)) as tar:
        members = tar.getmembers()
        assert [m.name for m in members] == ['BOOT.APP', 'DATA', 'DATA/SAMPLES.BIN', 'META.TXT'], [m.name for m in members]
        assert all(m.mtime == 0 and m.uid == 0 and m.uname == '' for m in members)
        for name, data in files.items():
            assert tar.extractfile(name).read() == data, name
    for bad in ('boot.app', 'LONGNAME9.BIN', 'A.TEXT', 'SP ACE.BIN'):
        other = folder/'BAD'
        other.mkdir()
        (other/bad).write_bytes(b'x')
        try:
            make(other)
            raise AssertionError(f'{bad} was taken')
        except ValueError:
            pass
        (other/bad).unlink()
        other.rmdir()
print('app tar OK')
