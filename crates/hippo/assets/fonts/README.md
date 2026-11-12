# The card's fonts

`DejaVuSans.ttf` and `DejaVuSans-Bold.ttf` are subsets of DejaVu Sans 2.37
(<https://github.com/dejavu-fonts/dejavu-fonts>, release `version_2_37`),
embedded in the binary so `hippo card` draws the same pixels on every
machine. They keep printable ASCII and a few marks,
no hinting and no layout tables; the GPOS pair kerning of the kept glyphs is
turned into a legacy `kern` table. `subset.py` rebuilds them.

The licence is `LICENSE-DejaVu.txt`: the Bitstream Vera licence, with
DejaVu's changes in the public domain. Release tarballs carry it beside the
binary.
