#!/usr/bin/env python3
# roll-up, the shard's /roll: plants in handleRoll and record; each must fail `go test -run Roll`. Run from the worktree root.
import subprocess
F = 'containers/git/server/main.go'
M = [('no repack slot', '\trepacking.Lock()\n\tdefer repacking.Unlock()\n\tl := lock(net)\n\tl.RLock()\n\tpacks, err',
      '\tl := lock(net)\n\tl.RLock()\n\tpacks, err'),
     ('single pack allowed', '\tif len(sums) < 2 {\n\t\tfail(w, 400, "a roll takes', '\tif len(sums) < 1 {\n\t\tfail(w, 400, "a roll takes'),
     ('first input dropped', '\tfor _, name := range names {\n\t\tfmt.Fprintf(&stdin, "pack-%s.pack\\n", name)',
      '\tfor _, name := range names[1:] {\n\t\tfmt.Fprintf(&stdin, "pack-%s.pack\\n", name)'),
     ('other packs rolled in', '\t\t\tfmt.Fprintf(&stdin, "^%s\\n", pack)', '\t\t\tfmt.Fprintf(&stdin, "%s\\n", pack)'),
     ('name not recorded', '\treturn os.Rename(tmp, filepath.Join(r, "graft", sum+".name"))', '\treturn nil')]
for n, o, new in M:
    s = open(F).read(); assert s.count(o) == 1, n
    open(F, 'w').write(s.replace(o, new))
    r = subprocess.run(['go', 'test', './...', '-run', 'Roll', '-count=1'], cwd='containers/git/server',
                       capture_output=True)
    open(F, 'w').write(s)
    print(('CAUGHT' if r.returncode else 'MISSED'), n)
