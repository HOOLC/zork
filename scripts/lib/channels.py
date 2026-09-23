"""Product channel metadata shared by packaging, installation and recovery."""
from pathlib import Path
from typing import NamedTuple


class Channel(NamedTuple):
    name: str
    prefix: str
    client_directory: str
    icon: str


CHANNELS = {
    'release': Channel('Zork', 'ing.zork', 'Library/Application Support/Zork/client', 'Zork.icns'),
    'dev': Channel('Zork Dev', 'ing.zork-dev', 'Zork/client-dev', 'ZorkDev.icns'),
    'test': Channel('Zork Test', 'ing.zork-test', 'Zork/client-test', 'ZorkDev.icns'),
}


def app_name(channel):
    return CHANNELS[channel].name


def id_prefix(channel):
    return CHANNELS[channel].prefix


def client_root(channel):
    return Path.home() / CHANNELS[channel].client_directory
