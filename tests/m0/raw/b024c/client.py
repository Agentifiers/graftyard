import socket, time, sys, json
host = sys.argv[1]; hold = float(sys.argv[2]) if len(sys.argv) > 2 else 0
t0 = time.time()
s = socket.create_connection((host, 22), timeout=20)
t1 = time.time()
s.settimeout(20)
hello = b""
while not hello.endswith(b"\n"):
    c = s.recv(4096)
    if not c: break
    hello += c
t2 = time.time()
out = {"connect_ms": round((t1-t0)*1000), "hello_ms": round((t2-t1)*1000), "hello": hello.decode(errors="replace").strip()[:300]}
# echo: 1 MB in 64 KB chunks
payload = bytes(range(256)) * 4096
sent = 0; got = b""; t3 = time.time()
for i in range(0, len(payload), 65536):
    s.sendall(payload[i:i+65536]); sent += 65536
    while len(got) < sent:
        c = s.recv(1 << 20)
        if not c: break
        got += c
t4 = time.time()
out.update(echo_ok=(got == payload), echo_bytes=len(got), echo_ms=round((t4-t3)*1000))
# hold: ping every 30 s
pings = []
end = time.time() + hold
while time.time() < end:
    time.sleep(min(30, max(0, end - time.time())))
    tp = time.time(); s.sendall(b"ping\n"); r = s.recv(64); pings.append((round(tp - t0), r == b"ping\n", round((time.time()-tp)*1000)))
out["pings"] = pings
out["held_s"] = round(time.time() - t0)
s.shutdown(socket.SHUT_WR)
tail = s.recv(64)
out["close_clean"] = tail == b""
print(json.dumps(out))
