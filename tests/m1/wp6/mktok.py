import hashlib, secrets, string, sys
A = string.digits + string.ascii_uppercase + string.ascii_lowercase
def crc32(b):
    import zlib; return zlib.crc32(b) & 0xffffffff
def tok():
    s = ''.join(secrets.choice(A) for _ in range(30)).encode()
    v = crc32(s); c = ['0']*6
    for i in range(5, -1, -1):
        c[i] = A[v % 62]; v //= 62
    t = "ghp_" + s.decode() + ''.join(c)
    h = hashlib.sha256(b"graftyard/token/" + s).hexdigest()
    return t, h
for _ in range(int(sys.argv[1])):
    print(*tok())
