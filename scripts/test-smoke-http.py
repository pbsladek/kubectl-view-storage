#!/usr/bin/env python3
"""Real local HTTP fixture regression: setup must never query DNS."""
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler
import threading
import unittest
from unittest import mock

from smoke_http import LoopbackHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'fixture response'
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class LoopbackFixtureTests(unittest.TestCase):
    def test_real_http_works_when_reverse_dns_is_unavailable(self):
        # A lookup would otherwise block before either smoke's per-command
        # timeout starts. Fail immediately if setup attempts that lookup.
        with mock.patch("socket.getfqdn", side_effect=AssertionError("unexpected DNS lookup")):
            server = LoopbackHTTPServer(Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            connection = HTTPConnection("127.0.0.1", server.server_port, timeout=2)
            try:
                self.assertEqual(server.server_name, "127.0.0.1")
                connection.request("GET", "/fixture")
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.read(), b'fixture response')
            finally:
                connection.close()
                server.shutdown()
                server.server_close()
                thread.join(timeout=2)
                self.assertFalse(thread.is_alive())


if __name__ == "__main__":
    unittest.main()
