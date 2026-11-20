# Vendored for the A1 drivers

`pako_deflate.min.cjs` is `dist/pako_deflate.min.js` from pako 2.1.0 (npm `pako-2.1.0.tgz`, license MIT and Zlib, in
`pako-LICENSE`). `../wire.mjs` deflates pack entries with it, so a1.mjs on the orb and Load34 in workerd write the same
bytes: workerd's `node:zlib` at level 1 writes different bytes from Node's (measured 2026-11-20, 3,071 against 2,459
bytes for `load34.test.mjs`'s fixed input), and pako at level 1 matched Node's zlib on 255 of 255 inputs. Test harness
only; nothing that ships imports it.
