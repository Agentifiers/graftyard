# The container's /spool, slowed: reads each body at RATE bytes/s per connection into nothing, then answers its length.
import http.server, os, socketserver, sys, time
RATE = float(os.environ.get("RATE", 60e6))
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def do_POST(self):
        n, t0 = 0, time.time()
        te = self.headers.get("transfer-encoding", "")
        def chunks():
            if "chunked" in te:
                while True:
                    size = int(self.rfile.readline().strip() or b"0", 16)
                    if size == 0:
                        self.rfile.readline(); return
                    yield self.rfile.read(size); self.rfile.readline()
            else:
                left = int(self.headers.get("content-length", 0))
                while left:
                    b = self.rfile.read(min(left, 1 << 20)); left -= len(b); yield b
        for b in chunks():
            n += len(b)
            ahead = n / RATE - (time.time() - t0)
            if ahead > 0: time.sleep(ahead)
        body = f"{n}".encode()
        self.send_response(200); self.send_header("content-length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
class S(socketserver.ThreadingMixIn, http.server.HTTPServer): daemon_threads = True
S(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
