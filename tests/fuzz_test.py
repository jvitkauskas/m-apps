# SPDX-License-Identifier: MIT
"""Fuzzes the parsers that read what comes from a card or the radio, with libFuzzer, for a few seconds each
(FUZZ_SECONDS). Each harness is tests/fuzz/<name>_fuzz.rs, built with the pi32v2 Rust compiler (PI32V2_RUST, as the
build has it: a nightly that has x86_64's std too) with coverage (LLVM's sancov) and overflow checks and debug
assertions on, and linked by the computer's clang (FUZZ_CC), whose -fsanitize=fuzzer gives libFuzzer's main. A panic
aborts, which libFuzzer counts as a crash. Without either, says so and passes; FUZZ_REQUIRE=1 makes that a failure (CI).
A crash leaves its input in build/fuzz/<name>/, and the run fails. tests/fuzz/found/<name>/ keeps the inputs that found
a bug, which every run starts from.

  fuzz_test.py [SECONDS] [NAME...]"""
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT/'build'/'fuzz'
LIB = OUT/'lib'
CC = os.environ.get('FUZZ_CC', 'clang')
RUSTC = ROOT/os.environ.get('PI32V2_RUST', 'pi32v2-rust')/'bin'/'rustc'
# -O1, as the C fuzzers were built. libFuzzer's coverage: edges (level 3, and 4 for indirect calls), as 8-bit counters
# inline, a table of where each is (-print_pcs, -print_coverage), and comparisons, so it can find the bytes a parser
# compares with. No AddressSanitizer: the code fuzzed is safe Rust, bounds-checked, but for kit/card_sim.rs's pointer
# into the image, which it checks against the card's size first (README). One codegen unit, as the apps have: with more,
# pi32v2-rust 1.100.0-1's rustc crashes (SIGSEGV) running sancov on them.
RUST_FLAGS = ['--edition', '2024', '-Copt-level=1', '-Ccodegen-units=1', '-Cdebuginfo=1', '-Cdebug-assertions=on', '-Coverflow-checks=on',
              '-Cpanic=abort', '-Cpasses=sancov-module', '-Cllvm-args=-sanitizer-coverage-level=4',
              '-Cllvm-args=-sanitizer-coverage-inline-8bit-counters', '-Cllvm-args=-sanitizer-coverage-pc-table',
              '-Cllvm-args=-sanitizer-coverage-trace-compares']
# Linked by lld (clang's -fuse-ld=lld), as the computer's rustc links the tests with its own (rust-lld), which this one
# hasn't: rustc has the linker keep every #[no_mangle] function of the kit and the drivers (symbols.o), and GNU ld then
# wants the symbols that their start-up for the unit (app_start, installer_main) takes from the linker script, where lld
# drops those functions as unused. (LTO would drop them too, but loses the kit's coverage.)
LINK_FLAGS = [f'-Clinker={shutil.which(CC) or CC}', '-Clink-arg=-fsanitize=fuzzer', '-Clink-arg=-fuse-ld=lld']
MB = ROOT/'m-bootloader/rust/src'

def jpeg_seeds(folder):
    """The sample pictures in apps/jpeg/images.rs."""
    text = (ROOT/'apps/jpeg/images.rs').read_text()
    for i, m in enumerate(re.finditer(r'Aligned\(\[([0-9,\s]+)\]\)', text)):
        data = bytes(int(x) for x in m.group(1).split(',') if x.strip())
        if data[:2] == b'\xff\xd8':
            (folder/f'image{i}.jpg').write_bytes(data)

def wifi_seeds(folder):
    (folder/'plain.txt').write_text('# mine\nuse=Home\n\nssid=Home\npassword=correct horse\n\nssid=Cafe\npassword=\n')
    (folder/'crlf.txt').write_bytes(b'\xef\xbb\xbf# x\r\nssid=A\r\n# its password\r\npassword=aaaaaaaa\r\nband=5\r\nuse=A')

def fat_seeds(folder):
    """A small FAT32 volume: one reserved sector, one FAT of two sectors, clusters of a sector; /WIFI.TXT, a long-named
    file of its own, a file whose clusters' FAT entries are in the second FAT sector, and an app in /MBOOT/APPS/HELLO
    with BOOT.APP, META.TXT and ICON.QOI."""
    clusters, sector, fat_sectors = 140, 512, 2
    image = bytearray((1+fat_sectors+clusters)*sector)
    boot = image
    boot[0:3] = b'\xeb\x58\x90'
    struct.pack_into('<HBHB', boot, 11, sector, 1, 1, 1)
    struct.pack_into('<IIHHIH', boot, 32, len(image)//sector, fat_sectors, 0, 0, 2, 0)
    boot[82:90] = b'FAT32   '
    boot[510:512] = b'\x55\xaa'
    fat = [0x0ffffff8, 0x0fffffff] + [0]*(fat_sectors*sector//4-2)
    def at(c):
        return (1+fat_sectors+c-2)*sector
    def entry(name, attr, start, size):
        return name.encode() + bytes([attr, 0, 0]) + bytes(6) + struct.pack('<H', start >> 16) + bytes(4) + \
            struct.pack('<HI', start & 0xffff, size)
    def lfn(long_name, short):
        s = 0
        for ch in short.encode():
            s = ((s >> 1) | ((s & 1) << 7)) + ch & 0xff
        units = [ord(c) for c in long_name] + [0]
        units += [0xffff]*(-len(units) % 13)
        pieces = [units[i:i+13] for i in range(0, len(units), 13)]
        out = b''
        for i in reversed(range(len(pieces))):
            u = pieces[i]
            e = bytearray(32)
            e[0] = (i+1) | (0x40 if i == len(pieces)-1 else 0)
            for k, place in enumerate([1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]):
                struct.pack_into('<H', e, place, u[k])
            e[11], e[13] = 0x0f, s
            out += bytes(e)
        return out
    def put(c, data, chain=1):
        for k in range(chain):
            fat[c+k] = c+k+1 if k < chain-1 else 0x0fffffff
            image[at(c+k):at(c+k)+sector] = data[k*sector:(k+1)*sector].ljust(sector, b'\0')
    wifi = b'use=Home\n\nssid=Home\npassword=correct horse\n'
    notes = b'Notes kept on the card.\n'*60
    put(2, entry('MBOOT      ', 0x10, 3, 0) + entry('WIFI    TXT', 0x20, 4, len(wifi)) +
        lfn('A long name.txt', 'ALONGN~1TXT') + entry('ALONGN~1TXT', 0x20, 11, len(wifi)) +
        entry('NOTES   TXT', 0x20, 130, len(notes)))
    put(11, wifi)
    put(130, notes, 3)
    put(3, entry('.          ', 0x10, 3, 0) + entry('..         ', 0x10, 0, 0) + entry('APPS       ', 0x10, 5, 0))
    put(4, wifi)
    put(5, entry('.          ', 0x10, 5, 0) + entry('..         ', 0x10, 3, 0) + entry('HELLO      ', 0x10, 6, 0))
    meta = b'name=Hello\nabout=Says hello\nversion=1.0\n'
    icon = b'qoif' + struct.pack('>II', 2, 2) + bytes([4, 0]) + bytes([0xfe, 255, 0, 0, 0xc2]) + bytes(7) + b'\x01'
    app = b'\x00'*600
    put(6, entry('.          ', 0x10, 6, 0) + entry('..         ', 0x10, 5, 0) + entry('BOOT    APP', 0x20, 7, len(app)) +
        entry('META    TXT', 0x20, 9, len(meta)) + entry('ICON    QOI', 0x20, 10, len(icon)))
    put(7, app, 2)
    put(9, meta)
    put(10, icon)
    image[sector:(1+fat_sectors)*sector] = struct.pack(f'<{fat_sectors*sector//4}I', *fat)
    (folder/'card.img').write_bytes(bytes(image))

def qoi_seeds(folder):
    for path in sorted(ROOT.glob('apps/*/ICON.QOI')):
        (folder/f'{path.parent.name}.qoi').write_bytes(path.read_bytes())

def meta_seeds(folder):
    for path in sorted(ROOT.glob('apps/*/META.TXT')):
        (folder/f'{path.parent.name}.txt').write_bytes(path.read_bytes())

def frames_seeds(folder):
    """A beacon with a name, a channel and an RSN element; a DHCP offer; an ARP request; an EAPOL key message."""
    rsn = bytes([48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0])
    beacon = bytes([0x80, 0]) + bytes(14) + bytes([0x10, 0x20, 0x30, 0x40, 0x50, 0x60]) + bytes(12) + bytes([0x11, 0]) + \
        bytes([0, 4]) + b'Home' + bytes([3, 1, 6]) + rsn
    (folder/'beacon').write_bytes(b'\x00' + beacon)
    hdr = bytes([0x08, 0x02, 0, 0, 2, 0x11, 0x22, 0x33, 0x44, 0x55, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60]) + bytes(8)
    llc = bytes([0xaa, 0xaa, 3, 0, 0, 0])
    dhcp = bytearray(240+12)
    dhcp[0], dhcp[1], dhcp[2] = 2, 1, 6
    dhcp[4:8] = bytes.fromhex('12345678')
    dhcp[16:20] = bytes([192, 168, 1, 5])
    dhcp[236:240] = bytes([0x63, 0x82, 0x53, 0x63])
    dhcp[240:252] = bytes([53, 1, 2, 54, 4, 192, 168, 1, 1, 255, 0, 0])
    udp = struct.pack('>HHHH', 67, 68, 8+len(dhcp), 0) + bytes(dhcp)
    ipv4 = bytearray(struct.pack('>BBHHHBBH4s4s', 0x45, 0, 20+len(udp), 0, 0, 64, 17, 0, bytes([192, 168, 1, 1]),
                                 bytes([255, 255, 255, 255]))) + udp
    (folder/'dhcp').write_bytes(b'\x01' + hdr + llc + b'\x08\x00' + bytes(ipv4))
    dns = struct.pack('>HHHH', 53, 5353, 8+12, 0) + bytes(12)
    ipv4 = struct.pack('>BBHHHBBH4s4s', 0x45, 0, 20+len(dns), 0, 0, 64, 17, 0, bytes([192, 168, 1, 1]),
                       bytes([192, 168, 1, 5])) + dns
    (folder/'udp').write_bytes(b'\x08' + hdr + llc + b'\x08\x00' + ipv4)
    arp = struct.pack('>HHBBH', 1, 0x800, 6, 4, 1) + bytes([0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 192, 168, 1, 1]) + \
        bytes(6) + bytes([192, 168, 1, 5])
    (folder/'arp').write_bytes(b'\x08' + hdr + llc + b'\x08\x06' + arp)
    mgmt = lambda fc: bytes([fc, 0]) + bytes(2) + bytes([2, 0x11, 0x22, 0x33, 0x44, 0x55]) + \
        bytes([0x10, 0x20, 0x30, 0x40, 0x50, 0x60])*2 + bytes(2)
    (folder/'auth').write_bytes(bytes([0x00]) + mgmt(0xb0) + struct.pack('<HHH', 0, 2, 0))
    (folder/'assoc').write_bytes(bytes([0x10]) + mgmt(0x10) + struct.pack('<HHH', 0x11, 0, 0xc001) +
                                 bytes([1, 4, 0x82, 0x84, 0x8b, 0x96]))
    key = bytes([2, 3, 0, 95, 2, 0x00, 0x8a, 0, 16]) + bytes(8) + bytes(range(32)) + bytes(16+8+8+16+2)
    (folder/'eapol').write_bytes(b'\x00' + hdr + llc + b'\x88\x8e' + key)

# name: its seeds, and the longest input (16 KiB if not given)
FUZZERS = {
    'jpeg': (jpeg_seeds,),
    'wifi_list': (wifi_seeds,),
    'frames': (frames_seeds,),
    'qoi': (qoi_seeds,),
    'meta': (meta_seeds,),
    'fat': (fat_seeds, 96*1024),
}


def rustc(*args):
    return subprocess.run([RUSTC, *RUST_FLAGS, *args], cwd=ROOT, capture_output=True, text=True)


def libraries():
    """The drivers and the kit, as tests/host.py's are (the FM-1's, a card in memory), with coverage too; again only
    when a source has changed."""
    sources = [*MB.rglob('*.rs'), *(ROOT/'kit').glob('*.rs'), RUSTC, Path(__file__)]
    newest = max(p.stat().st_mtime for p in sources)
    LIB.mkdir(parents=True, exist_ok=True)
    for name, args in [('mboot', ['--cfg', 'panel_test', MB/'lib.rs']),
                       ('kit', ['--cfg', 'board="fm1"', '--extern', f'mboot={LIB}/libmboot.rlib', ROOT/'kit/lib.rs'])]:
        rlib = LIB/f'lib{name}.rlib'
        if rlib.exists() and rlib.stat().st_mtime >= newest:
            continue
        built = rustc('--crate-name', name, '--crate-type', 'rlib', '-L', LIB, *args, '-o', rlib)
        if built.returncode:
            sys.exit(f'fuzz: {name} failed to build\n{built.stderr}')
        newest = max(newest, rlib.stat().st_mtime)


def tools():
    """What's missing to fuzz, and why, or None: a Rust harness built with coverage, linked with libFuzzer, and run
    once."""
    if not RUSTC.exists():
        return f'the pi32v2 Rust compiler not found at {RUSTC} (PI32V2_RUST names it)', ''
    if not shutil.which(CC):
        return f'{CC} not found (FUZZ_CC names another clang)', ''
    probe = OUT/'probe'
    (OUT/'probe.rs').write_text('#![no_main]\n#[unsafe(no_mangle)]\n'
                                'pub extern "C" fn LLVMFuzzerTestOneInput(_: *const u8, _: usize) -> i32 {\n    0\n}\n')
    built = rustc(*LINK_FLAGS, OUT/'probe.rs', '-o', probe)
    if built.returncode:
        return f'{RUSTC} and {CC} with libFuzzer and lld could not build a fuzzer (FUZZ_CC names another clang)', \
            built.stderr
    ran = subprocess.run([probe, '-runs=1'], capture_output=True, text=True)
    if ran.returncode:
        return f'a fuzzer built with {CC} did not run', ran.stderr
    return None, ''


def main():
    args = sys.argv[1:]
    seconds = int(args.pop(0)) if args and args[0].isdigit() else int(os.environ.get('FUZZ_SECONDS', '10'))
    names = args or list(FUZZERS)
    OUT.mkdir(parents=True, exist_ok=True)
    missing, why = tools()
    if missing:
        print(f'fuzz: skipped, {missing}')
        if why:
            print(why[-2000:])
        # FUZZ_REQUIRE=1 (CI): no fuzzing is a failure, not a pass.
        if os.environ.get('FUZZ_REQUIRE') == '1':
            sys.exit(1)
        return
    libraries()
    failed = []
    for name in names:
        seeds, *most = FUZZERS[name]
        folder = OUT/name
        corpus = folder/'corpus'
        corpus.mkdir(parents=True, exist_ok=True)
        seeds(corpus)
        # Inputs that found a bug once: run first, every time.
        for found in sorted((ROOT/'tests/fuzz/found'/name).glob('*')):
            shutil.copy(found, corpus/found.name)
        binary = folder/name
        built = rustc('--crate-name', f'{name}_fuzz', '--crate-type', 'bin', '--extern', f'kit={LIB}/libkit.rlib',
                      '-L', LIB, *LINK_FLAGS, f'tests/fuzz/{name}_fuzz.rs', '-o', binary)
        if built.returncode:
            sys.exit(f'fuzz {name}: failed to build\n{built.stderr}')
        if built.stderr:
            print(built.stderr)
        run = subprocess.run([binary, corpus, f'-max_total_time={seconds}', f'-max_len={most[0] if most else 16384}',
                              '-timeout=10', f'-artifact_prefix={folder}/', '-print_final_stats=1'],
                             capture_output=True, text=True)
        stats = [l for l in run.stderr.splitlines() if l.startswith('stat::number_of_executed_units')]
        runs = stats[-1].split(':')[-1].strip() if stats else '?'
        # Coverage when the corpus had been run, and at the end: what this run added.
        progress = [m for m in re.finditer(r'^#\d+\s+(\w+)\s+cov: (\d+) ft: (\d+)', run.stderr, re.M)]
        start = next((m for m in progress if m.group(1) == 'INITED'), None)
        cov = f', cov {start.group(2)} ft {start.group(3)} -> cov {progress[-1].group(2)} ft {progress[-1].group(3)}' \
            if start else ''
        if run.returncode:
            failed.append(name)
            print(f'fuzz {name}: FAILED after {runs} runs; the input is in {folder}/')
            lines = run.stderr.splitlines()
            # The panic's place and what it said, libFuzzer's verdict, the stack, and where the input went.
            said = {i+1 for i, l in enumerate(lines) if ' panicked at ' in l}
            print('\n'.join(l for i, l in enumerate(lines) if i in said or 'panicked' in l or 'ERROR' in l or
                            'SUMMARY' in l or ' #' in l[:6] or 'in ' in l[:40] or 'Test unit written' in l)[:3000])
        else:
            print(f'fuzz {name}: OK, {runs} runs in {seconds} s{cov}')
    if failed:
        sys.exit(1)


if __name__ == '__main__':
    main()
