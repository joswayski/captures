#!/usr/bin/python3
"""Exercise explicit feedback against loopback; production is never contacted."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time

from native_check import cmd, find, wait, click, screen_bounds, capture
from process_metrics import stop


class Handler(BaseHTTPRequestHandler):
    requests = []
    first_received = threading.Event()
    release_first = threading.Event()

    def do_POST(self):
        size = int(self.headers.get('content-length', '0'))
        self.requests.append(json.loads(self.rfile.read(size)))
        first = len(self.requests) == 1
        if first:
            self.first_received.set()
            if not self.release_first.wait(timeout=10):
                return
        body = (b'{"ok":true}' if not first else
                b'{"error":"Temporary test failure; retry is available."}')
        self.send_response(200 if not first else 500)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def type_into(name, text, frame):
    node = wait(lambda: find(name, 'text', frame))
    bounds = screen_bounds(node, frame)
    cmd('xdotool', 'mousemove', bounds.x + bounds.width // 2,
        bounds.y + min(bounds.height // 2, 24), 'click', 1,
        'key', 'ctrl+a', 'type', '--clearmodifiers', text)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--lab', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path)
    args = parser.parse_args()
    os.environ.update(json.loads((args.lab / 'environment.json').read_text()))
    import pyatspi

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    root = Path(__file__).resolve().parent
    binary = root / 'target/release/captures-linux-native'
    with tempfile.TemporaryDirectory(prefix='captures-feedback-ui-') as temporary:
        env = dict(os.environ,
                   CAPTURES_NATIVE_DATA=temporary,
                   CAPTURES_FEEDBACK_URL=f'http://127.0.0.1:{server.server_port}/feedback')
        process = subprocess.Popen([str(binary), '--preferences'], env=env,
                                   stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                   text=True, start_new_session=True)
        try:
            wait(lambda: find('Captures Preferences', 'frame'))
            click('About preferences section', 'Captures Preferences')
            click('Send feedback', 'Captures Preferences', pointer=True)
            frame = 'Send feedback to Captures'
            wait(lambda: find(frame, 'frame'))
            click('Idea', frame)
            type_into('Feedback message', 'Pointer hover misses preview controls.', frame)
            type_into('Feedback contact', 'tester@example.invalid', frame)
            click('Send feedback', frame, pointer=True)
            wait(Handler.first_received.is_set)
            message = find('Feedback message', 'text', frame)
            contact = find('Feedback contact', 'text', frame)
            idea = find('Idea', frame=frame)
            send = find('Send feedback', 'push button', frame)
            assert not message.getState().contains(pyatspi.STATE_EDITABLE)
            assert not contact.getState().contains(pyatspi.STATE_EDITABLE)
            assert not idea.getState().contains(pyatspi.STATE_SENSITIVE)
            assert not send.getState().contains(pyatspi.STATE_SENSITIVE)
            if args.artifacts:
                capture(args.artifacts, 'feedback-pending', frame)

            Handler.release_first.set()
            wait(lambda: send.getState().contains(pyatspi.STATE_SENSITIVE))
            for node in (message, contact, idea, send):
                node.clearCache()
            message = find('Feedback message', 'text', frame)
            contact = find('Feedback contact', 'text', frame)
            idea = find('Idea', frame=frame)
            send = find('Send feedback', 'push button', frame)
            assert message.getState().contains(pyatspi.STATE_EDITABLE)
            assert contact.getState().contains(pyatspi.STATE_EDITABLE)
            assert idea.getState().contains(pyatspi.STATE_SENSITIVE)
            if args.artifacts:
                capture(args.artifacts, 'feedback-error-retry', frame)

            click('Send feedback', frame, pointer=True)
            wait(lambda: find('Thanks — your feedback was sent.', frame=frame))
            wait(lambda: len(Handler.requests) == 2)
            for payload in Handler.requests:
                assert payload['message'] == 'Pointer hover misses preview controls.'
                assert payload['contact'] == 'tester@example.invalid'
                assert payload['category'] == 'idea'
                assert payload['source'] == 'desktop' and payload['os'] == 'linux'
            assert not message.getState().contains(pyatspi.STATE_EDITABLE)
            assert not contact.getState().contains(pyatspi.STATE_EDITABLE)
            assert not send.getState().contains(pyatspi.STATE_SENSITIVE)
            assert find('Close', 'push button', frame)
            if args.artifacts:
                capture(args.artifacts, 'feedback-success', frame)
            print('PASS feedback: pending freeze; failed-submit recovery and retained draft; exact retry payload; explicit sent state', flush=True)
        finally:
            stop(process)
            time.sleep(.2)
    server.shutdown()
    server.server_close()
    thread.join(timeout=2)


if __name__ == '__main__':
    main()
