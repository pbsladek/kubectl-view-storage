"""Numeric-loopback HTTP fixtures without host-name resolution."""
from http.server import ThreadingHTTPServer
from socketserver import TCPServer


class LoopbackHTTPServer(ThreadingHTTPServer):
    def __init__(self, handler):
        super().__init__(("127.0.0.1", 0), handler)

    def server_bind(self):
        # HTTPServer.server_bind calls socket.getfqdn(), which can wait for
        # reverse DNS on macOS runners. These fixtures only need a bound port;
        # their local requests must not depend on the runner's DNS service.
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address
