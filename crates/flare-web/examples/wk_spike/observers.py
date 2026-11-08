# The network observers of the spike's adv-exfil and adv-proxy scenarios (started by adv.rs): an mDNS
# sniffer, a raw TCP accept logger, a logging CONNECT proxy that refuses every tunnel, and a dual-stack
# HTTP server. Each writes one line per event to <logdir>/<name>.log.
# python3 -I observers.py <logdir> <tcp port> <proxy port> <http port> <seconds>
import socket, struct, sys, threading, time
logdir = sys.argv[1]
def log(name, s):
    with open(f"{logdir}/{name}.log", "a") as f:
        f.write(f"{time.time():.3f} {s}\n")

def mdns():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
    s.bind(("", 5353))
    mreq = struct.pack("4sl", socket.inet_aton("224.0.0.251"), socket.INADDR_ANY)
    s.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
    while True:
        d, a = s.recvfrom(9000)
        if b"advdns" in d.lower():
            i = d.lower().find(b"advdns")
            log("mdns", f"{a} {d[i:i+30]!r}")

def tcp_accept(port):
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port)); s.listen(50)
    while True:
        c, a = s.accept()
        c.settimeout(1.5)
        try: d = c.recv(200)
        except Exception: d = b"<no bytes>"
        log("tcp", f"accept from {a} first={d[:60]!r}")
        c.close()

def proxy(port):
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port)); s.listen(50)
    while True:
        c, a = s.accept()
        def h(c=c):
            c.settimeout(3)
            try:
                d = c.recv(2000)
                log("proxy", d.split(b"\r\n")[0].decode(errors="replace"))
                c.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
            except Exception as e:
                log("proxy", f"err {e}")
            c.close()
        threading.Thread(target=h, daemon=True).start()

def http(port, fam, host):
    s = socket.socket(fam); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    if fam == socket.AF_INET6: s.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
    s.bind((host, port)); s.listen(50)
    while True:
        c, a = s.accept()
        def h(c=c, a=a):
            c.settimeout(3)
            try:
                d = c.recv(4000)
                lines = d.split(b"\r\n")
                hostl = [l for l in lines if l.lower().startswith(b"host:")]
                log("http", f"{a[0]} {lines[0].decode(errors='replace')} {hostl[:1]}")
                body = b"ok"
                c.sendall(b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 2\r\nConnection: close\r\n\r\n" + body)
            except Exception as e:
                log("http", f"err {e}")
            c.close()
        threading.Thread(target=h, daemon=True).start()

tcpp, proxyp, httpp = int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
for t in [threading.Thread(target=mdns), threading.Thread(target=tcp_accept, args=(tcpp,)),
          threading.Thread(target=proxy, args=(proxyp,)),
          threading.Thread(target=http, args=(httpp, socket.AF_INET, "0.0.0.0")),
          threading.Thread(target=http, args=(httpp, socket.AF_INET6, "::"))]:
    t.daemon = True; t.start()
log("start", "up")
time.sleep(int(sys.argv[5]))
