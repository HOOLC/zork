#!/usr/bin/env python3
"""Generate macOS ICNS assets from the approved brand artwork (macOS only)."""
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET


def generate(source, destination):
    with tempfile.TemporaryDirectory(prefix='zork-icon-') as scratch:
        iconset = Path(scratch) / 'Icon.iconset'
        if source.suffix == '.svg':
            # macOS supplies the icon mask and outer plate. The preview SVG's
            # rounded tile must become full bleed, as in the main app artwork.
            tree = ET.parse(source)
            background = tree.getroot().find('{http://www.w3.org/2000/svg}rect')
            if background is None:
                raise ValueError(f'{source}: app icon requires a background rectangle')
            background.attrib.pop('rx', None)
            background.attrib.pop('ry', None)
            unmasked = Path(scratch) / 'unmasked.svg'
            tree.write(unmasked, encoding='utf-8', xml_declaration=True)
            source = unmasked
            rendered = Path(scratch) / 'source.png'
            subprocess.run(['swift', str(Path(__file__).with_name('render-macos-icon.swift')),
                            str(source), str(rendered)], check=True)
            source = rendered
        iconset.mkdir()
        for size in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = str(size * scale)
                name = f'icon_{size}x{size}' + ('@2x' if scale == 2 else '') + '.png'
                subprocess.run(['sips', '-z', pixels, pixels, str(source),
                                '--out', str(iconset / name)], check=True, capture_output=True)
        subprocess.run(['iconutil', '-c', 'icns', str(iconset),
                        '-o', str(destination)], check=True)


def main():
    assets = Path(__file__).resolve().parents[2] / 'crates/zork-ui/assets/app'
    for source, output in [('icon.png', 'Zork.icns'), ('station.svg', 'ZorkStation.icns'),
                           ('supervisor.svg', 'ZorkSupervisor.icns'),
                           ('service-watch.svg', 'ZorkServiceWatch.icns'),
                           ('browser.svg', 'ZorkBrowser.icns'),
                           *[(f'browser-{role.lower()}.svg', f'ZorkBrowser{role}.icns')
                             for role in ('Helper', 'Alerts', 'GPU', 'Plugin', 'Renderer', 'Network', 'Storage')]]:
        generate(assets / source, assets / output)


if __name__ == '__main__':
    main()
