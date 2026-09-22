# /// script
# dependencies = ["fonttools==4.62.1"]
# ///
"""Materialize the committed native Inter faces from the source variable fonts."""
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = Path(__file__).resolve().parents[2]
SHARED = ROOT / "crates/zork-ui/assets/fonts"
OUTPUT = SHARED / "static"
OUTPUT.mkdir(parents=True, exist_ok=True)

for source, italic in [
    (SHARED / "InterVariable.ttf", False),
    (SHARED / "InterVariable-Italic.ttf", True),
]:
    for weight, style in [(400, "Regular"), (500, "Medium"), (600, "SemiBold"), (700, "Bold")]:
        suffix = "-Italic" if italic else ""
        dest = OUTPUT / f"Inter-{weight}{suffix}.ttf"
        if dest.exists() and dest.stat().st_mtime >= source.stat().st_mtime:
            continue
        font = instantiateVariableFont(TTFont(source), {"opsz": 14, "wght": weight}, inplace=True)
        face = ("Italic" if weight == 400 else style + " Italic") if italic else style
        for record in font["name"].names:
            value = {1: "Inter Variable", 2: face, 16: "Inter Variable", 17: face,
                     6: "Inter-" + face.replace(" ", "")}.get(record.nameID)
            if value is not None:
                record.string = value.encode(record.getEncoding())
        font.save(dest)
        print(dest.relative_to(ROOT), flush=True)
