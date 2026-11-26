# SPDX-License-Identifier: MIT
"""spectrum_tables.rs: the FFT's sines and the Hann window, in Q15. `python3 apps/mp3/tables.py`."""
import math
from pathlib import Path

N = 512


def q15(x):
    return max(-32767, min(32767, round(x*32767)))


def rows(values):
    return ',\n'.join('    ' + ', '.join(str(v) for v in values[i:i+16]) for i in range(0, len(values), 16))


cos = [q15(math.cos(2*math.pi*k/N)) for k in range(N//2)]
sin = [q15(math.sin(2*math.pi*k/N)) for k in range(N//2)]
hann = [q15(0.5-0.5*math.cos(2*math.pi*n/(N-1))) for n in range(N//2)]
text = f'''// SPDX-License-Identifier: MIT
//! Made by tables.py.
pub const N: usize = {N};
pub static FFT_COS: [i16; {N//2}] = [
{rows(cos)},
];
pub static FFT_SIN: [i16; {N//2}] = [
{rows(sin)},
];
/// The first half; the window is symmetric.
pub static HANN: [i16; {N//2}] = [
{rows(hann)},
];
'''
(Path(__file__).parent/'spectrum_tables.rs').write_text(text)
