"""Generate crates/cpu-45gs02/src/table.rs from the MEGA65 user guide's
instruction_sets/{6502,4510,45GS02}.{opc,cycles} (data only).

    python tools/gen_45gs02_table.py DIR crates/cpu-45gs02/src/table.rs

DIR holds the six files from github.com/MEGA65/mega65-user-guide
(instruction_sets/); they are not part of this repository.
"""
import os
import re
import sys

D = sys.argv[1]
OUT = sys.argv[2]


def lines(name):
    return [l.rstrip('\n') for l in open(os.path.join(D, name), encoding='utf-8') if l.strip()]


def parse_opc(name):
    out = {}
    for l in lines(name):
        code, rest = l.split(None, 1)
        out[code.upper()] = rest.strip()
    return out


def parse_cycles(name):
    out = {}
    for l in lines(name):
        code, c = l.split()
        out[code.upper()] = c
    return out


MODES = [
    (r'^#\$nnnn$', 'Imm16'),
    (r'^#\$nn$', 'Imm'),
    (r'^\(\$nn,X\)$', 'Izx'),
    (r'^\(\$nn\),Y$', 'Izy'),
    (r'^\(\$nn\),Z$', 'Izz'),
    (r'^\(\$nn,SP\),Y$', 'Isy'),
    (r'^\(\$nnnn\)$', 'Ind'),
    (r'^\(\$nnnn,X\)$', 'Iax'),
    (r'^\$nn,\$rr$', 'Zrel'),
    (r'^\$nn,X$', 'Zpx'),
    (r'^\$nn,Y$', 'Zpy'),
    (r'^\$nn$', 'Zp'),
    (r'^\$nnnn,X$', 'Absx'),
    (r'^\$nnnn,Y$', 'Absy'),
    (r'^\$nnnn$', 'Abs'),
    (r'^\$rrrr$', 'Rel16'),
    (r'^\$rr$', 'Rel'),
    (r'^A$', 'Acc'),
    (r'^$', 'Imp'),
]


def mode(operand):
    for pat, m in MODES:
        if re.match(pat, operand):
            return m
    raise SystemExit('mode? ' + operand)


def split(text):
    parts = text.split(None, 1)
    return parts[0].upper(), (parts[1] if len(parts) > 1 else '')


def cyc(c):
    m = re.match(r'^(\d*)([a-z]*)$', c)
    n, f = m.group(1), m.group(2)
    return (int(n) if n else None), f


o45 = parse_opc('4510.opc')
c45 = parse_cycles('4510.cycles')
o65 = parse_opc('6502.opc')
c65 = parse_cycles('6502.cycles')

# Fixes to evident slips in the guide's 4510 table: an entry without a
# count, counts far from every sibling of the same mode, a branch flagged
# like a read. Each is set to its siblings' value.
FIX45 = {
    '0F': '4rb', '1F': '4rb', '2F': '4rb', '3F': '4rb',  # BBR0-3 as BBR4-7
    '30': '2b',                     # BMI as the other branches
    'E1': '5rp', 'F1': '5rp',       # SBC (zp,X) / (zp),Y as ADC/CMP
    '41': '5rp', '61': '5rp',       # EOR/ADC (zp,X) as AND/CMP
}

rows = []
for i in range(256):
    code = '%02X' % i
    mn, operand = split(o45[code])
    if mn == 'RTS' and operand.startswith('#'):
        mn = 'RTN'  # RTS #imm8: its own mnemonic in the decoder
    md = mode(operand)
    if mn in ('ASL', 'LSR', 'ROL', 'ROR', 'INC', 'DEC', 'NEG', 'ASR') and operand == 'A':
        md = 'Acc'
    cs = FIX45.get(code, c45[code])
    n45, f45 = cyc(cs)
    mn65, op65 = split(o65[code])
    same = (mn65 == mn and mode(op65) == md)
    n65, f65 = cyc(c65[code]) if same else (n45, '')
    if same and 'p' in f45 and 'p' not in f65:
        pass
    rows.append((code, mn, md, n45, f45, n65, f65, same))

# 45GS02 compound opcodes: Q (prefix 42 42), flat (EA), flat Q (42 42 EA).
oq = parse_opc('45GS02.opc')
cq = parse_cycles('45GS02.cycles')
qrows = []
for code, text in sorted(oq.items()):
    mn, operand = split(text)
    c = cq.get(code, '')
    n, f = cyc(c) if c else (None, '')
    qrows.append((code, mn, operand, n, f))

out = []
out.append('//! Opcode table, generated from the MEGA65 user guide\'s')
out.append('//! `instruction_sets/4510.opc`, `4510.cycles`, `6502.opc`, `6502.cycles`,')
out.append('//! `45GS02.opc` and `45GS02.cycles` (github.com/MEGA65/mega65-user-guide,')
out.append('//! fetched 2026-11-11) by a script of this project. The 4510 counts of')
out.append('//! BBR0–BBR3, BMI and the (zp,X)/(zp),Y forms of EOR, ADC and SBC are')
out.append('//! set to their siblings\' values where the guide\'s table has an evident')
out.append('//! slip (a missing count, 3 cycles for an indirect mode).')
out.append('//!')
out.append('//! Cycle flags (the guide\'s legend): `R` and `I` add a cycle at 40 MHz,')
out.append('//! `P` adds one when indexing crosses a page, `B` one for a taken branch')
out.append('//! (and one more when it crosses a page), `D` subtracts one at 3.5 MHz.')
out.append('')
out.append('use crate::Mode;')
mns = sorted(set(r[1] for r in rows))
out.append('')
out.append('/// The 4510 mnemonics (`RTN` is RTS #imm8, `EOM` the 6502 NOP).')
out.append('#[derive(Clone, Copy, Debug, PartialEq, Eq)]')
out.append('pub enum Mn {')
for m in mns:
    out.append('    %s,' % m.capitalize())
out.append('}')
out.append('')
out.append('impl Mn {')
out.append("    pub fn name(self) -> &'static str {")
out.append('        match self {')
for m in mns:
    out.append('            Mn::%s => "%s",' % (m.capitalize(), 'RTS' if m == 'RTN' else m))
out.append('        }')
out.append('    }')
out.append('}')
out.append('')
out.append('pub const R: u8 = 1;')
out.append('pub const P: u8 = 2;')
out.append('pub const B: u8 = 4;')
out.append('pub const D: u8 = 8;')
out.append('pub const I: u8 = 16;')
out.append('')
out.append('/// One opcode of the 4510 personality.')
out.append('#[derive(Clone, Copy, Debug)]')
out.append('pub struct Op {')
out.append('    pub mn: Mn,')
out.append('    pub mode: Mode,')
out.append('    /// Cycles at 3.5 and 40 MHz (before the flags).')
out.append('    pub fast: u8,')
out.append('    pub fast_flags: u8,')
out.append('    /// Cycles at 1 and 2 MHz: the 6502\'s for its own opcodes, else the')
out.append('    /// 4510\'s.')
out.append('    pub slow: u8,')
out.append('    pub slow_flags: u8,')
out.append('}')
out.append('')


def flags(f):
    names = [('r', 'R'), ('p', 'P'), ('b', 'B'), ('d', 'D'), ('i', 'I')]
    v = [N for c, N in names if c in f]
    return ' | '.join(v) if v else '0'


out.append('pub static OPS: [Op; 256] = [')
for code, mn, md, n45, f45, n65, f65, same in rows:
    out.append('    // $%s' % code)
    out.append('    Op { mn: Mn::%s, mode: Mode::%s, fast: %d, fast_flags: %s, slow: %d, slow_flags: %s },' % (
        mn.capitalize(), md, n45, flags(f45), n65, flags(f65)))
out.append('];')
out.append('')
out.append('/// The compound instructions: prefix bytes and opcode as written in the')
out.append('/// guide (`4242xx` Q, `EAxx` flat, `4242EAxx` flat Q), the mnemonic and')
out.append('/// the cycles with their flags.')
out.append('pub const COMPOUND: &[(u32, &str, u8, u8)] = &[')
for code, mn, operand, n, f in qrows:
    out.append('    (0x%s, "%s %s", %d, %s),' % (code, mn, operand, n if n is not None else 0, flags(f)))
out.append('];')
open(OUT, 'w', encoding='utf-8', newline='\n').write('\n'.join(out) + '\n')
print('rows', len(rows), 'compound', len(qrows))
