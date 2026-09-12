#!/usr/bin/python3
"""Disposable X11 visual-test desktop, never a personal session.

Run under dbus-run-session + xvfb-run. Provides a known screen-lock authority
because an Xvfb test session has no logind. The application still uses its real
fail-closed session checks. SetActive can exercise lock cancellation.
"""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import xml.etree.ElementTree as ET

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GdkPixbuf, GLib


class ScreenSaver(dbus.service.Object):
    active = False

    @dbus.service.method('org.freedesktop.ScreenSaver', out_signature='b')
    def GetActive(self):
        return self.active

    @dbus.service.method('org.freedesktop.ScreenSaver', in_signature='b')
    def SetActive(self, active):
        self.active = bool(active)


def main():
    if not os.environ.get('DISPLAY') or not os.environ.get('DBUS_SESSION_BUS_ADDRESS'):
        raise RuntimeError('Run only inside a disposable Xvfb and DBus session')
    directory = Path(sys.argv[1]).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    DBusGMainLoop(set_as_default=True)
    bus = dbus.SessionBus()
    if bus.name_has_owner('org.freedesktop.ScreenSaver'):
        raise RuntimeError('Refusing to replace an existing screen-lock service')
    name = dbus.service.BusName('org.freedesktop.ScreenSaver', bus=bus)
    saver = ScreenSaver(name, '/org/freedesktop/ScreenSaver')
    # Do not let Openbox's default PrintScreen binding steal the app's shortcut.
    config = ET.parse('/etc/xdg/openbox/rc.xml')
    keyboard = config.find('{http://openbox.org/3.4/rc}keyboard')
    for binding in list(keyboard):
        if 'print' in binding.get('key', '').lower():
            keyboard.remove(binding)
    config.write(directory / 'openbox.xml')
    # Composite real alpha, without xcompmgr's whole-window synthetic shadows.
    # Its -c mode leaves rectangular shadows behind sparse RGBA overlays in
    # this lab; those are not part of either application's rendered chrome.
    processes = [subprocess.Popen(['openbox', '--config-file', str(directory/'openbox.xml')]),
                 subprocess.Popen(['xcompmgr', '-n'])]
    fixture = directory / 'fixture.png'
    # Reuse the React harness's neutral source image, not a screenshot containing
    # Captures controls that could be mistaken for native application chrome.
    source = Path(__file__).resolve().parents[2] / 'apps/desktop/ui/src/dev/previewBackend.ts'
    template = re.search(r'function sampleCapture\(.*?const svg = `(.+?)`;',
                         source.read_text(), re.DOTALL)
    if template is None:
        raise RuntimeError('React sampleCapture fixture changed; update the lab extractor')
    svg = template.group(1).replace('${width}', '960').replace('${height}', '540')
    loader = GdkPixbuf.PixbufLoader.new_with_type('svg')
    loader.write(svg.encode())
    loader.close()
    loader.get_pixbuf().savev(str(fixture), 'png', [], [])
    window = Gtk.Window(title='Reference content — Captures comparison')
    window.set_default_size(980, 610)
    window.move(210, 80)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
    box.set_border_width(16)
    box.pack_start(Gtk.Label(label='Shared test content · 960 × 540 · No private desktop data'), False, False, 0)
    box.pack_start(Gtk.Image.new_from_pixbuf(GdkPixbuf.Pixbuf.new_from_file(str(fixture))), True, True, 0)
    window.add(box)
    window.show_all()
    (directory / 'environment.json').write_text(json.dumps({
        key: os.environ[key] for key in ['DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'XAUTHORITY']
    }))
    def finish(*_):
        Gtk.main_quit()
        return False
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, finish)
    try:
        Gtk.main()
    finally:
        for process in processes:
            process.terminate()
            process.wait()
    return saver


if __name__ == '__main__':
    main()
