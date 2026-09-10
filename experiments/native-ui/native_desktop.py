#!/usr/bin/python3
"""Disposable X11 visual-test desktop, never a personal session.

Run under dbus-run-session + xvfb-run. Provides a known screen-lock authority
because an Xvfb test session has no logind. The application still uses its real
fail-closed session checks. SetActive can exercise lock cancellation.
"""
import json
import os
from pathlib import Path
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
    keyboard.clear()
    config.write(directory / 'openbox.xml')
    processes = [subprocess.Popen(['openbox', '--config-file', str(directory/'openbox.xml')]),
                 subprocess.Popen(['xcompmgr', '-c'])]
    fixture = directory / 'fixture.png'
    source = Path(__file__).resolve().parents[2] / 'docs/images/capture-selection.jpg'
    subprocess.run(['convert', str(source), '-resize', '960x540!', str(fixture)], check=True)
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
