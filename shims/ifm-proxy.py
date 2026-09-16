#!/usr/bin/env python3
"""ifm-proxy — absorb a provider quirk so a working model becomes usable.

IFM's endpoint rejects any multi-turn conversation whose assistant messages lack a
`reasoning` field:

    Add a supported thinking field to each assistant message in the multi-turn
    conversation history.

opencode does not send one, so every IFM run died on the turn that would have written
code. The model is fine; the pairing is broken. This proxy sits between them and injects
`reasoning: ""` where it is missing.

This is the general shape of an adapter shim: a provider-specific transform that makes a
(model x harness) pair viable without patching either side. See beads farmerbob-pdr
(arm identity is model x harness) and farmerbob-5g2 (adapter layer).

    ./ifm-proxy.py [port]         then point the provider baseURL at http://127.0.0.1:port/v1
"""
import http.server, json, socketserver, sys, urllib.request, urllib.error

UPSTREAM = "https://api.ifm.ai"
PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8713


def patch(body: bytes) -> bytes:
    """Add the reasoning field IFM requires on replayed assistant turns."""
    try:
        d = json.loads(body)
    except Exception:
        return body
    n = 0
    for m in d.get("messages", []):
        if m.get("role") == "assistant" and "reasoning" not in m:
            m["reasoning"] = m.pop("reasoning_content", "") or ""
            n += 1
    if n:
        print(f"  patched {n} assistant message(s)", file=sys.stderr, flush=True)
    return json.dumps(d).encode()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):  # quiet
        pass

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        body = patch(body)
        req = urllib.request.Request(UPSTREAM + self.path, data=body, method="POST")
        for h in ("authorization", "content-type", "accept"):
            if self.headers.get(h):
                req.add_header(h, self.headers[h])
        try:
            with urllib.request.urlopen(req, timeout=600) as r:
                self.send_response(r.status)
                for k, v in r.headers.items():
                    if k.lower() not in ("transfer-encoding", "content-length", "connection"):
                        self.send_header(k, v)
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                while chunk := r.read(8192):          # stream SSE through untouched
                    self.wfile.write(b"%x\r\n%s\r\n" % (len(chunk), chunk))
                    self.wfile.flush()
                self.wfile.write(b"0\r\n\r\n")
        except urllib.error.HTTPError as e:
            payload = e.read()
            self.send_response(e.code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    def do_GET(self):
        req = urllib.request.Request(UPSTREAM + self.path)
        if self.headers.get("authorization"):
            req.add_header("authorization", self.headers["authorization"])
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                payload = r.read()
                self.send_response(r.status)
                self.send_header("Content-Type", r.headers.get("Content-Type", "application/json"))
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
        except urllib.error.HTTPError as e:
            payload = e.read()
            self.send_response(e.code)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


if __name__ == "__main__":
    print(f"ifm-proxy: 127.0.0.1:{PORT} -> {UPSTREAM}", file=sys.stderr, flush=True)
    Server(("127.0.0.1", PORT), Handler).serve_forever()
