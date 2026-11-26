# SPDX-License-Identifier: MIT
"""Turn the constant tables of a C header into Rust: each `static const <type> NAME[...] = {...};` becomes a
`pub static NAME: [<type>; N] = [...];` with the same values. Used once for the tables the C apps had (fonts, art,
RF tables); a generator that made a C table now makes the Rust one itself. Comments before a table are kept."""
import argparse
import re
from pathlib import Path

TYPES = {'uint8_t': 'u8', 'int8_t': 'i8', 'uint16_t': 'u16', 'int16_t': 'i16', 'uint32_t': 'u32', 'int32_t': 'i32',
         'char': 'u8', 'unsigned char': 'u8', 'float': 'f32', 'short': 'i16', 'unsigned short': 'u16', 'int': 'i32',
         'unsigned': 'u32'}
TABLE = re.compile(r'((?:/\*.*?\*/\s*)*)(?:static\s+)?const\s+([a-z_0-9 ]+?)\s+([A-Za-z_0-9]+)((?:\s*\[[^\]]*\])+)\s*=\s*'
                   r'(\{.*?\});', re.S)


def tokens(body):
    return re.findall(r'\{|\}|"(?:[^"\\]|\\.)*"|\'(?:[^\'\\]|\\.)\'|[^,{}\s]+', body)


def value(t, rtype):
    t = t.strip()
    if t.startswith("'"):
        return str(ord(bytes(t[1:-1], 'utf-8').decode('unicode_escape')))
    t = re.sub(r'(?<=[0-9a-fA-F])[uUlL]+$', '', t)
    t = re.sub(r'^\((?:u?int\d+_t|unsigned|int)\)', '', t)
    if rtype == 'f32':
        t = t.rstrip('fF')
        if re.fullmatch(r'-?\d+', t):
            t += '.0'
        return t
    return t


def parse(toks, i, rtype):
    """A brace group from toks[i] == '{': its values, nested as lists."""
    assert toks[i] == '{'
    out, i = [], i+1
    while toks[i] != '}':
        if toks[i] == '{':
            sub, i = parse(toks, i, rtype)
            out.append(sub)
        else:
            out.append(value(toks[i], rtype))
            i += 1
    return out, i+1


def shape(v):
    return [len(v)] + (shape(v[0]) if v and isinstance(v[0], list) else [])


def pad(v, dims, zero):
    """C fills what's left out with zeros."""
    if len(dims) == 1:
        return v + [zero]*(dims[0]-len(v))
    return [pad(x, dims[1:], zero) for x in v] + [pad([], dims[1:], zero)]*(dims[0]-len(v))


def rust(v, indent='    '):
    if v and isinstance(v[0], list):
        return '[\n' + ''.join(f'{indent}{rust(x, indent+"    ")},\n' for x in v) + indent[:-4] + ']'
    lines, line = [], []
    for x in v:
        line.append(x)
        if len(line) == 16:
            lines.append(', '.join(line))
            line = []
    if line:
        lines.append(', '.join(line))
    if len(lines) == 1:
        return '[' + lines[0] + ']'
    return '[\n' + ''.join(f'{indent}{ln},\n' for ln in lines) + indent[:-4] + ']'


def dims_of(brackets, v):
    want = re.findall(r'\[([^\]]*)\]', brackets)
    got = shape(v)
    dims = []
    for k, w in enumerate(want):
        w = w.strip()
        dims.append(int(eval(w, {}, {})) if w and re.fullmatch(r'[0-9 +*()/-]+', w) else got[k] if k < len(got) else 0)
    return dims


def convert(text, names=None, public=True):
    out = []
    for m in TABLE.finditer(text):
        comment, ctype, name, brackets, body = m.groups()
        ctype = ' '.join(ctype.split())
        if names and name not in names:
            continue
        rtype = TYPES[ctype]
        toks = tokens(body)
        if any(t.startswith('"') for t in toks):
            continue
        v, _ = parse(toks, 0, rtype)
        dims = dims_of(brackets, v)
        zero = '0.0' if rtype == 'f32' else '0'
        v = pad(v, dims, zero)
        ty = rtype
        for d in reversed(dims):
            ty = f'[{ty}; {d}]'
        for c in re.findall(r'/\*(.*?)\*/', comment or '', re.S):
            for ln in ' '.join(c.split()).split('\n'):
                out.append(f'/// {ln.strip()}')
        out.append(f'{"pub " if public else ""}static {name}: {ty} = {rust(v)};\n')
    return '\n'.join(out)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('header', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--names', nargs='*', help='only these tables')
    p.add_argument('--licence', default='MIT', help='the SPDX licence for the output')
    p.add_argument('--note', default='', help='a line for the top, after the licence')
    a = p.parse_args()
    head = f'// SPDX-License-Identifier: {a.licence}\n'
    if a.note:
        head += ''.join(f'//! {ln}\n' for ln in a.note.split('\n'))
    head += f'//! Made by tools/c_tables.py from {a.header.name}.\n#![allow(clippy::all)]\n\n'
    a.output.write_text(head + convert(a.header.read_text(), a.names))


if __name__ == '__main__':
    main()
