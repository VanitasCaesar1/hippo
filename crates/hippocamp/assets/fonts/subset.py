"""Rebuilds the card's fonts from DejaVu 2.37.

    python3 -m venv venv && venv/bin/pip install fonttools
    venv/bin/python subset.py <dejavu-fonts-ttf-2.37/ttf>

Keeps printable ASCII and the few marks the card draws, drops hinting and
layout tables, and turns the GPOS pair kerning of the kept glyphs into a
legacy `kern` table (format 0), which ttf-parser reads without shaping.
"""

import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont, newTable
from fontTools.ttLib.tables._k_e_r_n import KernTable_format_0

UNICODES = list(range(0x20, 0x7F)) + [0xA0, 0xB7, 0xD7, 0x2013, 0x2014, 0x2026]
HERE = Path(__file__).resolve().parent


def pair_kerning(font):
    """Every (left, right) -> x advance adjustment of the GPOS kern feature."""
    pairs = {}
    gpos = font["GPOS"].table
    lookups = set()
    for rec in gpos.FeatureList.FeatureRecord:
        if rec.FeatureTag == "kern":
            lookups.update(rec.Feature.LookupListIndex)
    for i in sorted(lookups):
        lookup = gpos.LookupList.Lookup[i]
        for sub in lookup.SubTable:
            if lookup.LookupType == 9:
                sub = sub.ExtSubTable
            if getattr(sub, "LookupType", 2) != 2:
                continue
            first = sub.Coverage.glyphs
            if sub.Format == 1:
                for left, pset in zip(first, sub.PairSet):
                    for rec in pset.PairValueRecord:
                        v = rec.Value1
                        adv = getattr(v, "XAdvance", 0) if v else 0
                        if adv:
                            pairs.setdefault((left, rec.SecondGlyph), adv)
            elif sub.Format == 2:
                c1 = sub.ClassDef1.classDefs
                c2 = sub.ClassDef2.classDefs
                order = font.getGlyphOrder()
                rights = {}
                for g in order:
                    rights.setdefault(c2.get(g, 0), []).append(g)
                for left in first:
                    row = sub.Class1Record[c1.get(left, 0)]
                    for k, rec in enumerate(row.Class2Record):
                        v = rec.Value1
                        adv = getattr(v, "XAdvance", 0) if v else 0
                        if not adv:
                            continue
                        for right in rights.get(k, []):
                            pairs.setdefault((left, right), adv)
    return pairs


def build(src, dst):
    full = TTFont(src)
    pairs = pair_kerning(full)
    opts = subset.Options()
    opts.layout_features = []
    opts.hinting = False
    opts.desubroutinize = True
    opts.name_IDs = ["*"]
    opts.name_legacy = True
    opts.name_languages = ["*"]
    opts.drop_tables += ["GPOS", "GSUB", "GDEF", "FFTM"]
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=UNICODES)
    sub.subset(full)
    kept = set(full.getGlyphOrder())
    table = KernTable_format_0()
    table.version = 0
    table.coverage = 1
    table.format = 0
    table.tupleIndex = None
    table.kernTable = {p: v for p, v in pairs.items() if p[0] in kept and p[1] in kept}
    kern = newTable("kern")
    kern.version = 0
    kern.kernTables = [table]
    full["kern"] = kern
    full.save(dst)
    print(f"{dst.name}: {len(kept)} glyphs, {len(table.kernTable)} kerning pairs")


def main():
    ttf = Path(sys.argv[1])
    for name in ("DejaVuSans", "DejaVuSans-Bold"):
        build(ttf / f"{name}.ttf", HERE / f"{name}.ttf")


if __name__ == "__main__":
    main()
