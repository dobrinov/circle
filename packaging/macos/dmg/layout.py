"""Writes the DMG window's layout (.DS_Store) into a mounted, writable
volume: the background picture, icon positions, no toolbar.

usage: layout.py MOUNT APP_NAME

The background must already be at MOUNT/.background/background.tiff. Finder
finds it through an alias to that file on this very volume, which is why the
layout is written per build (with the ds_store and mac_alias packages).
"""
import os
import sys

from ds_store import DSStore
from mac_alias import Alias

mount, app = sys.argv[1], sys.argv[2]
background = Alias.for_file(os.path.join(mount, ".background", "background.tiff"))

with DSStore.open(os.path.join(mount, ".DS_Store"), "w+") as d:
    d["."]["bwsp"] = {
        "ShowStatusBar": False,
        "ShowToolbar": False,
        "ShowTabView": False,
        "ShowPathbar": False,
        "ShowSidebar": False,
        "ContainerShowSidebar": False,
        "PreviewPaneVisibility": False,
        "SidebarWidth": 180,
        "WindowBounds": "{{200, 120}, {640, 400}}",
    }
    d["."]["vSrn"] = ("long", 1)
    d["."]["icvl"] = ("type", b"icnv")
    d["."]["icvp"] = {
        "viewOptionsVersion": 1,
        "backgroundType": 2,
        "backgroundImageAlias": background.to_bytes(),
        "backgroundColorRed": 1.0,
        "backgroundColorGreen": 1.0,
        "backgroundColorBlue": 1.0,
        "gridOffsetX": 0.0,
        "gridOffsetY": 0.0,
        "gridSpacing": 100.0,
        "arrangeBy": "none",
        "showIconPreview": False,
        "showItemInfo": False,
        "labelOnBottom": True,
        "textSize": 13.0,
        "iconSize": 96.0,
        "scrollPositionX": 0.0,
        "scrollPositionY": 0.0,
    }
    d[app]["Iloc"] = (170, 190)
    d["Applications"]["Iloc"] = (470, 190)
