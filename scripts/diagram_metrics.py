"""Measured glyph advances and kern pairs for the diagram font stack.

This is data, not logic: `scripts/diagram_check.py` imports `text_width`
from here so that its overlap checker measures a label the way a browser
does, kerning included, instead of guessing from a per-character table.

Stack::

    -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif

Measured with
Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/145.0.7632.6 Safari/537.36
on Windows 11 (headless), via canvas ``measureText`` at 100px for the stack
above.  All values are in units of 1/1000 em (measured px at 100px, times ten,
rounded to the nearest integer).

Resolved family
---------------
On this machine the stack resolves to **Segoe UI**.  ``-apple-system``,
``BlinkMacSystemFont``, ``Roboto``, ``'Helvetica Neue'`` and
``'Segoe UI Variable Text'`` are all absent -- each measures byte-identically to
the generic ``sans-serif`` default (1892.82 px for the probe string under the
stack vs 2137.16 px for every absent family), while an explicit ``'Segoe UI'``
reproduces the stack's widths to the last float bit.  ``document.fonts.check``
is useless as a discriminator here: it returns ``True`` for every family,
including ones that are not installed.

Weight 600 is a real **Segoe UI Semibold** face, not a synthetic interpolation:
at 600 the digits are *proportional* ("1" is 402 while "0" is 555), whereas at
400 and 700 all ten digits are uniform (539 and 575).  Any diagram code that
assumes tabular figures must not use weight 600.

A pixel-level probe (render each glyph under ``'Segoe UI'`` and under an
unknown family, compare the ink) confirms that every one of the 116 characters
below is supplied by Segoe UI itself -- no visible fallback, and no glyph
measures zero.  (The probe's controls U+4E00, U+0915 and U+2603 were correctly
flagged as falling back, so the test has teeth.)

Kerning
-------
``measureText`` shapes the run with HarfBuzz and applies Segoe UI's GPOS kern
pairs, so a naive sum of per-character advances is **not** the string width.
The largest single pair is "P," at -171 units (-2.9 px at 17px).  Kerning here
decomposes exactly pairwise -- for every diagram label, the sum of the single
advances plus the sum of the adjacent-pair deltas equals the whole-string
measurement bit-for-bit -- so ``text_width`` below is exact rather than
approximate.  Use it; do not sum ``ADVANCE_*`` by hand.

``measureText`` is also exactly linear in font size on this stack (a 1000px
measurement divided by ten equals the 100px measurement bit-for-bit), so
scaling a 1000-em table by ``size_px / 1000`` is safe.

How to regenerate
-----------------
Open any page in a browser on the target platform and run, for each weight in
(400, 600, 700)::

    const c = document.createElement("canvas").getContext("2d");
    c.font = `${weight} 100px ${STACK}`;
    adv  = ch      => Math.round(c.measureText(ch).width * 10);
    kern = (a, b)  => Math.round(c.measureText(a + b).width * 10) - adv(a) - adv(b);

over the 116 characters in ADVANCE_400 and all ordered pairs of them; keep the
non-zero kern deltas. The whole-string width is then the sum of the singles
plus the sum of the adjacent-pair deltas, exactly -- which is what
``text_width`` computes and what ``scripts/diagram_check.py`` measures its
labels with.

One caveat this table cannot remove: it is Segoe UI's, because that is the face
this stack resolves to on Windows. The same SVG renders in SF on macOS and in
whatever the distribution ships on Linux, and individual glyph advances there
differ from Segoe UI's by up to about fifteen per cent. ``diagram_check`` pays
for that with a per-run allowance proportional to the run's own width on top of
its fixed margin, so a label that clears its neighbours here still clears them
in a face a few per cent wider.
"""

UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/145.0.7632.6 Safari/537.36"

FONT_STACK = (
    "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, "
    "'Helvetica Neue', Arial, sans-serif"
)

RESOLVED_FAMILY = "Segoe UI"

ADVANCE_400 = {
    " ": 274, "!": 284, "\"": 392, "#": 591, "$": 539, "%": 818, "&": 800, "'": 230, "(": 302,
    ")": 302, "*": 417, "+": 684, ",": 217, "-": 400, ".": 217, "/": 390, "0": 539, "1": 539,
    "2": 539, "3": 539, "4": 539, "5": 539, "6": 539, "7": 539, "8": 539, "9": 539, ":": 217,
    ";": 217, "<": 684, "=": 684, ">": 684, "?": 448, "@": 955, "A": 645, "B": 573, "C": 619,
    "D": 701, "E": 506, "F": 488, "G": 686, "H": 710, "I": 266, "J": 357, "K": 580, "L": 471,
    "M": 898, "N": 748, "O": 754, "P": 560, "Q": 754, "R": 598, "S": 531, "T": 524, "U": 687,
    "V": 621, "W": 934, "X": 590, "Y": 553, "Z": 570, "[": 302, "\\": 379, "]": 302, "^": 684,
    "_": 415, "`": 268, "a": 509, "b": 588, "c": 462, "d": 589, "e": 523, "f": 313, "g": 589,
    "h": 566, "i": 242, "j": 242, "k": 497, "l": 242, "m": 861, "n": 566, "o": 586, "p": 588,
    "q": 589, "r": 348, "s": 424, "t": 339, "u": 566, "v": 479, "w": 723, "x": 459, "y": 484,
    "z": 452, "{": 302, "|": 239, "}": 302, "~": 684, "\u00b0": 377, "\u00b1": 684,
    "\u00b2": 366, "\u00b7": 217, "\u00b9": 351, "\u00d7": 684, "\u03a3": 516, "\u03ba": 524,
    "\u03c1": 586, "\u2013": 500, "\u2014": 1000, "\u2018": 229, "\u2019": 229, "\u201c": 377,
    "\u201d": 377, "\u2026": 733, "\u207b": 366, "\u2192": 863, "\u2212": 684, "\u2264": 685,
    "\u2265": 685
}

ADVANCE_600 = {
    " ": 275, "!": 304, "\"": 438, "#": 591, "$": 555, "%": 840, "&": 715, "'": 258, "(": 332,
    ")": 332, "*": 434, "+": 694, ",": 241, "-": 402, ".": 241, "/": 414, "0": 555, "1": 402,
    "2": 555, "3": 555, "4": 576, "5": 555, "6": 558, "7": 536, "8": 555, "9": 558, ":": 241,
    ";": 241, "<": 694, "=": 694, ">": 694, "?": 444, "@": 955, "A": 671, "B": 604, "C": 621,
    "D": 717, "E": 518, "F": 502, "G": 697, "H": 735, "I": 292, "J": 396, "K": 611, "L": 489,
    "M": 924, "N": 767, "O": 756, "P": 584, "Q": 756, "R": 623, "S": 544, "T": 552, "U": 703,
    "V": 642, "W": 966, "X": 619, "Y": 577, "Z": 587, "[": 332, "\\": 405, "]": 332, "^": 694,
    "_": 415, "`": 289, "a": 522, "b": 603, "c": 470, "d": 603, "e": 531, "f": 345, "g": 603,
    "h": 582, "i": 261, "j": 261, "k": 525, "l": 261, "m": 886, "n": 583, "o": 597, "p": 603,
    "q": 603, "r": 370, "s": 431, "t": 361, "u": 583, "v": 507, "w": 756, "x": 501, "y": 508,
    "z": 464, "{": 332, "|": 278, "}": 332, "~": 694, "\u00b0": 378, "\u00b1": 694,
    "\u00b2": 383, "\u00b7": 241, "\u00b9": 371, "\u00d7": 694, "\u03a3": 540, "\u03ba": 545,
    "\u03c1": 601, "\u2013": 500, "\u2014": 1000, "\u2018": 256, "\u2019": 256, "\u201c": 429,
    "\u201d": 429, "\u2026": 813, "\u207b": 383, "\u2192": 863, "\u2212": 694, "\u2264": 695,
    "\u2265": 695
}

ADVANCE_700 = {
    " ": 276, "!": 327, "\"": 493, "#": 592, "$": 575, "%": 867, "&": 850, "'": 293, "(": 369,
    ")": 369, "*": 455, "+": 707, ",": 271, "-": 404, ".": 271, "/": 443, "0": 575, "1": 575,
    "2": 575, "3": 575, "4": 575, "5": 575, "6": 575, "7": 575, "8": 575, "9": 575, ":": 271,
    ";": 271, "<": 707, "=": 707, ">": 707, "?": 438, "@": 954, "A": 703, "B": 641, "C": 624,
    "D": 737, "E": 532, "F": 520, "G": 711, "H": 766, "I": 317, "J": 445, "K": 649, "L": 511,
    "M": 957, "N": 790, "O": 758, "P": 614, "Q": 758, "R": 653, "S": 561, "T": 586, "U": 723,
    "V": 667, "W": 1005, "X": 655, "Y": 607, "Z": 607, "[": 369, "\\": 436, "]": 369, "^": 707,
    "_": 415, "`": 314, "a": 538, "b": 620, "c": 480, "d": 619, "e": 541, "f": 383, "g": 619,
    "h": 602, "i": 284, "j": 284, "k": 559, "l": 284, "m": 916, "n": 605, "o": 611, "p": 620,
    "q": 619, "r": 398, "s": 440, "t": 389, "u": 605, "v": 542, "w": 797, "x": 552, "y": 538,
    "z": 479, "{": 369, "|": 326, "}": 369, "~": 707, "\u00b0": 380, "\u00b1": 707,
    "\u00b2": 404, "\u00b7": 271, "\u00b9": 394, "\u00d7": 707, "\u03a3": 569, "\u03ba": 570,
    "\u03c1": 618, "\u2013": 500, "\u2014": 1000, "\u2018": 290, "\u2019": 290, "\u201c": 493,
    "\u201d": 493, "\u2026": 912, "\u207b": 404, "\u2192": 863, "\u2212": 707, "\u2264": 708,
    "\u2265": 708
}

# Safe width for a character that is not in the table: the widest advance
# seen at that weight.
FALLBACK_400 = 1000
FALLBACK_600 = 1000
FALLBACK_700 = 1005

# Ordered kern pairs, units/1000 em, non-zero entries only.  Key is the two
# characters concatenated.  A pair absent from the dict kerns by zero.

KERN_400 = {
    "AC": -20, "AG": -20, "AJ": 46, "AO": -20, "AT": -72, "AU": -20, "AV": -68, "AW": -68,
    "AY": -68, "AZ": 29, "Av": -20, "Aw": -20, "Ay": -20, "BT": -45, "BY": -32, "CC": -27,
    "CG": -27, "CO": -27, "DA": -24, "DT": -45, "DV": -24, "DW": -24, "DX": -29, "DZ": -24,
    "FA": -59, "FJ": -32, "Fa": -34, "GT": -24, "GV": -20, "GW": -20, "GX": -20, "Gy": -20,
    "Gz": -20, "JA": -24, "JJ": -39, "KC": -44, "KG": -44, "KJ": 44, "KO": -44, "KV": 20,
    "KW": 20, "KX": 24, "KY": 20, "KZ": 20, "Kc": -13, "Kd": -13, "Ke": -13, "Kg": -13,
    "Ko": -24, "Kq": -24, "Kt": -23, "Kv": -34, "Kw": -34, "Kx": 15, "Ky": -44, "LA": 29,
    "LC": -34, "LG": -32, "LJ": 49, "LO": -34, "LT": -68, "LU": -20, "LV": -59, "LW": -59,
    "LY": -59, "LZ": 29, "Lt": -13, "Lv": -49, "Lw": -49, "Ly": -49, "OA": -24, "OJ": -5,
    "OT": -45, "OV": -24, "OW": -24, "OX": -24, "OY": -24, "OZ": -24, "PA": -68, "PJ": -63,
    "PX": -24, "Pa": -34, "Pc": -37, "Pd": -37, "Pe": -37, "Pg": -37, "Po": -37, "Pq": -37,
    "QA": -24, "QT": -45, "QV": -24, "QW": -24, "QX": -24, "QY": -24, "QZ": -24, "RC": -14,
    "RG": -14, "RJ": 28, "RO": -10, "RT": -45, "RY": -19, "Rc": -29, "Rd": -29, "Re": -29,
    "Rg": -29, "Ro": -29, "Rq": -26, "St": -32, "Sv": -24, "Sw": -15, "Sx": -15, "Sy": -29,
    "Sz": -29, "TA": -68, "TC": -46, "TG": -46, "TJ": -55, "TO": -46, "TS": -20, "TT": 20,
    "TV": 21, "TW": 21, "TY": 20, "Ta": -112, "Tc": -98, "Td": -98, "Te": -98, "Tf": -47,
    "Tg": -98, "Tm": -87, "Tn": -87, "To": -98, "Tp": -87, "Tq": -98, "Tr": -87, "Ts": -75,
    "Tv": -50, "Tw": -55, "Tx": -88, "Ty": -55, "Tz": -55, "UA": -24, "VA": -59, "VC": -24,
    "VG": -24, "VJ": -34, "VO": -24, "VT": 19, "Va": -73, "Vc": -63, "Vd": -63, "Ve": -63,
    "Vg": -63, "Vm": -39, "Vn": -39, "Vo": -63, "Vp": -37, "Vq": -63, "Vr": -37, "Vs": -32,
    "Vu": -24, "WA": -39, "WT": 19, "Wa": -39, "Wc": -39, "Wd": -39, "We": -39, "Wg": -39,
    "Wo": -39, "Wq": -39, "XC": -11, "XG": -11, "XJ": 47, "XO": -11, "XT": 16, "YA": -78,
    "YC": -39, "YG": -39, "YJ": -32, "YO": -39, "YT": 19, "Ya": -88, "Yc": -88, "Yd": -88,
    "Ye": -88, "Yf": -13, "Yg": -88, "Ym": -69, "Yn": -69, "Yo": -88, "Yp": -68, "Yq": -88,
    "Yr": -69, "Ys": -65, "ZJ": 40, "ZT": 19, "Zy": -26
}

KERN_600 = {
    "\"r": -23, "\"s": -29, "'r": -27, "'s": -40, "(j": 104, "*A": -74, "*J": -68, "*c": -45,
    "*d": -45, "*e": -45, "*g": -45, "*o": -45, "*q": -45, ",\u2018": -101, ",\u2019": -101,
    ",\u201c": -101, ",\u201d": -101, ".\u2018": -101, ".\u2019": -98, ".\u201c": -101,
    ".\u201d": -93, "A*": -62, "A,": 31, "A;": 31, "AC": -14, "AG": -12, "AJ": 42, "AO": -14,
    "AT": -71, "AU": -15, "AV": -56, "AW": -34, "AY": -76, "AZ": 21, "At": -16, "Av": -21,
    "Aw": -14, "Ay": -19, "A\u2018": -68, "A\u2019": -92, "A\u201c": -68, "A\u201d": -92,
    "BT": -36, "BY": -29, "C?": 5, "CC": -28, "CG": -28, "CO": -13, "CQ": -25, "D,": -57,
    "D.": -57, "DA": -16, "DT": -41, "DX": -28, "DZ": -22, "D\u2026": -57, "EA": 9, "EJ": 29,
    "ET": 5, "EW": 17, "EX": 11, "F,": -73, "F.": -73, "FA": -60, "FJ": -29, "FS": -12,
    "FT": 9, "Fa": -34, "Ff": 7, "F\u2026": -68, "GT": -22, "GV": -12, "Gy": -12, "J,": -45,
    "J.": -45, "JA": -23, "JJ": -29, "Ja": -12, "J\u2026": -45, "K,": 24, "K;": 24, "KC": -37,
    "KG": -37, "KJ": 37, "KO": -37, "KQ": -37, "KX": 19, "KZ": 20, "Kc": -12, "Kd": -12,
    "Ke": -12, "Kg": -12, "Ko": -12, "Kq": -12, "Kt": -24, "Kv": -36, "Kw": -25, "Ky": -43,
    "L*": -101, "L?": -45, "LA": 26, "LC": -29, "LG": -29, "LJ": 40, "LO": -30, "LQ": -30,
    "LT": -60, "LU": -17, "LV": -57, "LW": -29, "LY": -66, "LZ": 29, "Lt": -12, "Lv": -48,
    "Lw": -31, "Ly": -36, "L\u2018": -65, "L\u2019": -57, "L\u201c": -65, "L\u201d": -54,
    "O,": -47, "O.": -42, "OA": -14, "OJ": -7, "OT": -43, "OX": -21, "OY": -16, "OZ": -22,
    "O\u2026": -41, "P,": -165, "P.": -155, "PA": -69, "PJ": -64, "PW": 18, "PX": -26,
    "Pa": -29, "Pc": -34, "Pd": -34, "Pe": -34, "Pg": -34, "Po": -34, "Pq": -33,
    "P\u2026": -144, "Q,": -42, "Q.": -52, "QA": -12, "QT": -43, "QX": -19, "QY": -9,
    "QZ": -22, "Q\u2026": -57, "R;": 40, "RC": -12, "RG": -12, "RJ": 26, "RO": -10, "RQ": -10,
    "RT": -23, "RY": -15, "Rc": -25, "Rd": -25, "Re": -26, "Rg": -26, "Ro": -27, "Rq": -25,
    "St": -29, "Sv": -22, "Sw": -12, "Sy": -24, "T,": -66, "T.": -89, "T:": -10, "T;": -10,
    "TA": -73, "TC": -41, "TG": -41, "TJ": -60, "TO": -42, "TQ": -42, "TT": 20, "TV": 24,
    "TW": 20, "TX": -2, "TY": 17, "Ta": -97, "Tc": -97, "Td": -97, "Te": -97, "Tf": -44,
    "Tg": -97, "Tm": -79, "Tn": -79, "To": -97, "Tp": -77, "Tq": -97, "Tr": -82, "Ts": -75,
    "Tu": -79, "Tv": -45, "Tw": -51, "Tx": -80, "Ty": -51, "Tz": -52, "T\u2019": 20,
    "T\u201d": 20, "T\u2026": -80, "UA": -21, "V,": -100, "V.": -106, "VA": -55, "VC": -21,
    "VG": -21, "VJ": -44, "VO": -4, "VQ": -17, "VS": -12, "VT": 20, "Va": -73, "Vc": -64,
    "Vd": -64, "Ve": -64, "Vg": -62, "Vm": -36, "Vn": -34, "Vo": -64, "Vp": -36, "Vq": -64,
    "Vr": -36, "Vs": -35, "Vu": -33, "V\u2026": -101, "W,": -59, "W.": -62, "WA": -36,
    "WT": 17, "Wa": -39, "Wc": -25, "Wd": -25, "We": -25, "Wg": -25, "Wo": -25, "Wq": -22,
    "W\u2026": -57, "X,": 31, "X.": 28, "X;": 35, "XC": -13, "XG": -13, "XJ": 41, "XO": -13,
    "XQ": -13, "XT": 18, "X\u2026": 25, "Y,": -97, "Y.": -102, "YA": -76, "YC": -23, "YG": -23,
    "YJ": -43, "YO": -23, "YQ": -23, "YS": -12, "YT": 20, "Ya": -94, "Yc": -89, "Yd": -89,
    "Ye": -89, "Yf": -14, "Yg": -89, "Ym": -67, "Yn": -67, "Yo": -89, "Yp": -68, "Yq": -89,
    "Yr": -67, "Ys": -61, "Yu": -67, "Y\u2026": -86, "ZJ": 33, "ZT": 20, "Zy": -25, "[j": 100,
    "ba": -12, "bf": -5, "bx": -16, "cJ": 34, "cT": -45, "cY": -34, "e\"": -46, "e'": -60,
    "f)": 55, "f,": -57, "f-": -45, "f.": -57, "f:": 40, "f;": 40, "f?": 31, "f]": 55,
    "fb": 12, "fh": 9, "ft": 19, "fv": 20, "fw": 20, "fx": 9, "fy": 18, "f}": 35,
    "f\u2018": 43, "f\u2019": 40, "f\u201c": 43, "f\u201d": 40, "f\u2026": -57, "gj": 17,
    "jj": 16, "k,": 40, "k-": -62, "k.": 40, "k:": 40, "k;": 40, "kc": -18, "kd": -12,
    "ke": -18, "kg": -18, "ko": -18, "kq": -12, "kt": -7, "k\u2026": 37, "n\"": -46, "n'": -55,
    "o\"": -64, "o'": -75, "oa": -12, "of": -17, "ox": -16, "o\u2018": -36, "o\u2019": -71,
    "o\u201c": -39, "o\u201d": -71, "pa": -12, "pf": -17, "px": -16, "p\u2018": -68,
    "p\u2019": -68, "p\u201c": -32, "p\u201d": -68, "qj": 47, "r,": -79, "r-": -57, "r.": -82,
    "r:": 40, "r;": 40, "rc": -9, "rd": -9, "re": -9, "rf": 21, "rg": -9, "ro": -9, "rq": -12,
    "rs": 6, "rt": 29, "rv": 40, "rw": 38, "rx": 28, "ry": 40, "rz": 20, "r\u2018": 80,
    "r\u2019": 60, "r\u201c": 80, "r\u201d": 60, "r\u2026": -75, "t-": -51, "t?": -32,
    "tc": -9, "td": -9, "te": -6, "tg": -6, "to": -6, "tq": -6, "tx": 14, "u\"": -29,
    "u'": -36, "v,": -59, "v.": -62, "va": -17, "vc": -6, "vd": -7, "ve": -8, "vg": -8,
    "vo": -8, "vq": -9, "v\u2026": -57, "w,": -42, "w.": -45, "wc": -4, "wd": -5, "we": -5,
    "wg": -4, "wo": -4, "wq": -5, "w\u2026": -45, "xc": -12, "xd": -12, "xe": -12, "xg": -12,
    "xo": -12, "xq": -12, "y\"": 13, "y'": 17, "y,": -52, "y.": -59, "y?": -20, "yc": -7,
    "yd": -7, "ye": -7, "yf": 4, "yg": -7, "yo": -7, "yq": -7, "yt": 2, "y\u2026": -56,
    "{j": 90, "\u03ba,": 18, "\u03ba-": -32, "\u03ba.": 18, "\u03ba:": 37, "\u03ba;": 37,
    "\u03ba\u2026": 18, "\u03c1\"": -64, "\u03c1'": -64, "\u03c1\u2018": -30,
    "\u03c1\u2019": -59, "\u03c1\u201c": -35, "\u03c1\u201d": -59, "\u2018A": -104,
    "\u2018C": -36, "\u2018J": -75, "\u2018T": 40, "\u2018c": -62, "\u2018d": -79,
    "\u2018e": -62, "\u2018g": -62, "\u2018o": -62, "\u2018s": -39, "\u2018\u2018": -86,
    "\u2019,": -45, "\u2019.": -45, "\u2019A": -72, "\u2019J": -79, "\u2019T": 40,
    "\u2019a": -50, "\u2019c": -98, "\u2019d": -79, "\u2019e": -79, "\u2019g": -79,
    "\u2019o": -79, "\u2019q": -75, "\u2019s": -68, "\u2019\u2019": -86, "\u2019\u2026": -45,
    "\u201c,": -46, "\u201c.": -46, "\u201cA": -104, "\u201cJ": -79, "\u201cT": 40,
    "\u201cc": -62, "\u201cd": -62, "\u201ce": -62, "\u201cg": -65, "\u201cs": -39,
    "\u201c\u2026": -46, "\u201d,": -45, "\u201d.": -45, "\u201dA": -68, "\u201dT": 40,
    "\u201dc": -15, "\u201dd": -79, "\u201de": -79, "\u201dg": -79, "\u201do": -79,
    "\u201ds": -62, "\u201d\u2026": -45, "\u2026\u2018": -92, "\u2026\u2019": -86,
    "\u2026\u201c": -92, "\u2026\u201d": -86
}

KERN_700 = {
    "\"r": -20, "\"s": -25, "'r": -30, "'s": -50, "(j": 93, "*A": -65, "*J": -60, "*c": -40,
    "*d": -40, "*e": -40, "*g": -40, "*o": -40, "*q": -40, ",\u2018": -101, ",\u2019": -101,
    ",\u201c": -101, ",\u201d": -101, ".\u2018": -101, ".\u2019": -101, ".\u201c": -101,
    ".\u201d": -90, "A*": -60, "A,": 29, "A;": 29, "AC": -15, "AG": -10, "AJ": 38, "AO": -15,
    "AT": -70, "AU": -17, "AV": -55, "AW": -32, "AY": -75, "AZ": 11, "At": -20, "Av": -22,
    "Aw": -15, "Ay": -20, "A\u2018": -60, "A\u2019": -90, "A\u201c": -60, "A\u201d": -90,
    "BT": -24, "BY": -25, "C?": 11, "CC": -30, "CG": -30, "CO": -12, "CQ": -22, "D,": -50,
    "D.": -50, "DA": -15, "DT": -35, "DX": -30, "DZ": -20, "D\u2026": -50, "EA": 14, "EJ": 24,
    "ET": 9, "EV": 5, "EW": 20, "EX": 20, "F,": -70, "F.": -70, "FA": -54, "FJ": -25,
    "FS": -10, "FT": 12, "Fa": -30, "Ff": 9, "F\u2026": -59, "GT": -20, "GV": -10, "Gy": -10,
    "J,": -40, "J.": -40, "JA": -30, "JJ": -25, "Ja": -10, "J\u2026": -40, "K,": 30, "K;": 30,
    "K?": 11, "KC": -29, "KG": -29, "KJ": 29, "KO": -29, "KQ": -29, "KT": 5, "KX": 20,
    "KZ": 20, "Kc": -10, "Kd": -10, "Ke": -10, "Kg": -10, "Ko": -10, "Kq": -10, "Kt": -25,
    "Kv": -35, "Kw": -25, "Ky": -42, "L*": -100, "L?": -40, "LA": 22, "LC": -25, "LG": -25,
    "LJ": 28, "LO": -25, "LQ": -25, "LT": -66, "LU": -20, "LV": -57, "LW": -35, "LY": -71,
    "LZ": 29, "Lt": -10, "Lv": -45, "Lw": -30, "Ly": -35, "L\u2018": -60, "L\u2019": -50,
    "L\u201c": -60, "L\u201d": -50, "O,": -50, "O.": -39, "OA": -15, "OJ": -10, "OT": -40,
    "OX": -25, "OY": -20, "OZ": -20, "O\u2026": -36, "P,": -171, "P.": -150, "PA": -59,
    "PG": 5, "PJ": -66, "PW": 17, "PX": -22, "Pa": -25, "Pc": -30, "Pd": -30, "Pe": -30,
    "Pg": -30, "Po": -30, "Pq": -30, "P\u2026": -126, "Q,": -39, "Q.": -39, "QA": -10,
    "QT": -40, "QX": -20, "QY": -15, "QZ": -20, "Q\u2026": -50, "R;": 40, "RC": -10, "RG": -10,
    "RJ": 24, "RO": -10, "RQ": -10, "RT": -20, "RY": -10, "Rc": -25, "Rd": -25, "Re": -25,
    "Rg": -25, "Ro": -25, "Rq": -25, "St": -25, "Sv": -20, "Sw": -10, "Sy": -25, "T,": -71,
    "T.": -91, "T:": -9, "T;": -9, "TA": -70, "TC": -35, "TG": -35, "TJ": -66, "TO": -37,
    "TQ": -37, "TT": 20, "TV": 29, "TW": 20, "TX": -2, "TY": 20, "Ta": -85, "Tc": -90,
    "Td": -90, "Te": -90, "Tf": -40, "Tg": -90, "Tm": -69, "Tn": -69, "To": -90, "Tp": -64,
    "Tq": -90, "Tr": -75, "Ts": -75, "Tu": -69, "Tv": -40, "Tw": -45, "Tx": -70, "Ty": -45,
    "Tz": -39, "T\u2019": 20, "T\u201d": 20, "T\u2026": -70, "UA": -22, "UJ": -18, "V,": -100,
    "V.": -100, "V:": -20, "V;": -20, "V?": 8, "VA": -52, "VC": -20, "VG": -20, "VJ": -56,
    "VO": -2, "VQ": -12, "VS": -10, "VT": 20, "Va": -75, "Vc": -65, "Vd": -65, "Ve": -65,
    "Vg": -60, "Vm": -35, "Vn": -30, "Vo": -65, "Vp": -35, "Vq": -65, "Vr": -35, "Vs": -38,
    "Vu": -27, "V\u2026": -88, "W,": -60, "W.": -60, "W:": -10, "W;": -10, "WA": -35, "WT": 14,
    "Wa": -40, "Wc": -27, "Wd": -27, "We": -27, "Wg": -27, "Wo": -27, "Wq": -20,
    "W\u2026": -50, "X,": 29, "X.": 29, "X;": 29, "XC": -15, "XG": -15, "XJ": 33, "XO": -15,
    "XQ": -15, "XT": 20, "X\u2026": 22, "Y,": -111, "Y.": -111, "YA": -75, "YC": -25,
    "YG": -25, "YJ": -56, "YO": -25, "YQ": -25, "YS": -10, "YT": 20, "Ya": -90, "Yc": -90,
    "Yd": -90, "Ye": -90, "Yf": -15, "Yg": -90, "Ym": -65, "Yn": -65, "Yo": -90, "Yp": -67,
    "Yq": -90, "Yr": -65, "Ys": -55, "Yu": -65, "Y\u2026": -75, "ZJ": 24, "ZT": 20, "Zy": -25,
    "[j": 83, "ba": -10, "bf": -5, "bx": -20, "cJ": 34, "cT": -40, "cY": -30, "e\"": -40,
    "e'": -70, "f)": 38, "f*": 21, "f,": -50, "f-": -40, "f.": -50, "f:": 40, "f;": 40,
    "f?": 30, "f]": 38, "fb": 15, "fh": 9, "fk": 5, "fl": 5, "ft": 19, "fv": 20, "fw": 20,
    "fx": 9, "fy": 20, "f}": 29, "f\u2018": 40, "f\u2019": 40, "f\u201c": 40, "f\u201d": 40,
    "f\u2026": -50, "gj": 9, "jj": 14, "k,": 40, "k-": -55, "k.": 40, "k:": 40, "k;": 40,
    "kc": -14, "kd": -10, "ke": -14, "kg": -14, "ko": -14, "kq": -10, "kt": -6, "kz": 8,
    "k\u2026": 32, "n\"": -40, "n'": -60, "o\"": -56, "o'": -80, "oa": -10, "of": -15,
    "oj": -2, "ox": -20, "o\u2018": -40, "o\u2019": -80, "o\u201c": -40, "o\u201d": -80,
    "pa": -10, "pf": -15, "px": -20, "p\u2018": -80, "p\u2019": -80, "p\u201c": -40,
    "p\u201d": -80, "qj": 44, "r,": -80, "r-": -50, "r.": -80, "r:": 40, "r;": 40, "rc": -4,
    "rd": -4, "re": -4, "rf": 25, "rg": -4, "rh": 3, "ri": 4, "rm": 3, "rn": 3, "ro": -4,
    "rq": -10, "rs": 6, "rt": 29, "ru": 3, "rv": 40, "rw": 34, "rx": 27, "ry": 40, "rz": 20,
    "r\u2018": 80, "r\u2019": 60, "r\u201c": 80, "r\u201d": 60, "r\u2026": -66, "t-": -45,
    "t?": -40, "tc": -4, "td": -4, "te": -4, "tg": -4, "to": -4, "tq": -4, "tx": 14,
    "u\"": -25, "u'": -40, "v,": -60, "v.": -60, "va": -15, "vc": -7, "vd": -7, "ve": -10,
    "vg": -10, "vo": -10, "vq": -10, "v\u2026": -50, "w,": -40, "w.": -40, "wc": -5, "wd": -5,
    "we": -5, "wg": -5, "wo": -5, "wq": -5, "w\u2026": -40, "xc": -17, "xd": -17, "xe": -17,
    "xg": -17, "xo": -17, "xq": -17, "y\"": 11, "y'": 20, "y,": -55, "y.": -55, "y?": 1,
    "yc": -10, "yd": -10, "ye": -10, "yf": 8, "yg": -10, "yo": -10, "yq": -10, "yt": 2,
    "y\u2026": -49, "{j": 78, "\u03ba,": 16, "\u03ba-": -28, "\u03ba.": 16, "\u03ba:": 32,
    "\u03ba;": 32, "\u03ba\u2026": 16, "\u03c1\"": -56, "\u03c1'": -56, "\u03c1\u2018": -26,
    "\u03c1\u2019": -51, "\u03c1\u201c": -31, "\u03c1\u201d": -51, "\u2018A": -100,
    "\u2018C": -40, "\u2018J": -80, "\u2018T": 40, "\u2018c": -60, "\u2018d": -80,
    "\u2018e": -60, "\u2018g": -60, "\u2018o": -60, "\u2018s": -40, "\u2018\u2018": -90,
    "\u2019,": -40, "\u2019.": -40, "\u2019A": -60, "\u2019J": -80, "\u2019T": 40,
    "\u2019a": -50, "\u2019c": -100, "\u2019d": -80, "\u2019e": -80, "\u2019g": -80,
    "\u2019o": -80, "\u2019q": -80, "\u2019s": -81, "\u2019\u2019": -90, "\u2019\u2026": -40,
    "\u201c,": -40, "\u201c.": -40, "\u201cA": -100, "\u201cJ": -80, "\u201cT": 40,
    "\u201cc": -60, "\u201cd": -60, "\u201ce": -60, "\u201cg": -60, "\u201cs": -40,
    "\u201c\u2026": -40, "\u201d,": -40, "\u201d.": -40, "\u201dA": -60, "\u201dT": 40,
    "\u201dc": -13, "\u201dd": -80, "\u201de": -80, "\u201dg": -80, "\u201do": -80,
    "\u201ds": -60, "\u201d\u2026": -40, "\u2026\u2018": -80, "\u2026\u2019": -75,
    "\u2026\u201c": -80, "\u2026\u201d": -75
}

ADVANCE = {400: ADVANCE_400, 600: ADVANCE_600, 700: ADVANCE_700}
FALLBACK = {400: FALLBACK_400, 600: FALLBACK_600, 700: FALLBACK_700}
KERN = {400: KERN_400, 600: KERN_600, 700: KERN_700}


def text_width(s, size_px, weight=400, kerning=True):
    """Advance width of `s` in px at `size_px` for `weight`.

    Matches Chrome's canvas ``measureText`` for this font stack to well under
    a tenth of a pixel at diagram sizes.  Set ``kerning=False`` only to
    reproduce a naive advance sum -- it will be wrong by up to ~0.8 px on a
    real label.
    """
    adv = ADVANCE[weight]
    fb = FALLBACK[weight]
    total = sum(adv.get(ch, fb) for ch in s)
    if kerning and len(s) > 1:
        kt = KERN[weight]
        total += sum(kt.get(s[i:i + 2], 0) for i in range(len(s) - 1))
    return total * size_px / 1000.0
