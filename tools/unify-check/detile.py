#!/usr/bin/env python3
"""detile.py in.rgba out.rgba W H [pitch]: undo Intel X-tiling of a KMS framebuffer that was read as if linear.
X-tiled: 512-byte x 8-row tiles (128 px at 4 bytes/px), stored tile after tile. Pixels whose bytes fell in the
row padding the grab dropped come out as magenta (and are listed as unknown)."""
import sys
src, dst, W, H = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
pitch = int(sys.argv[5]) if len(sys.argv) > 5 else ((W * 4 + 511) // 512) * 512
data = open(src, 'rb').read()
assert len(data) == W * H * 4, (len(data), W * H * 4)
tiles_per_row = pitch // 512
out = bytearray(W * H * 4)
unknown = 0
rowbytes = W * 4
for y in range(H):
    ty, iy = divmod(y, 8)
    base_row = ty * tiles_per_row
    for tx in range((W * 4 + 511) // 512):
        mem = (base_row + tx) * 4096 + iy * 512
        lrow, lcol = divmod(mem, pitch)
        x0 = tx * 128
        n = min(128, W - x0) * 4
        o = (y * W + x0) * 4
        # the 512 bytes of this tile row sit at linear (lrow, lcol..lcol+512), possibly wrapping to the next linear row
        got = 0
        while got < n:
            if lrow >= H:
                out[o + got:o + n] = b'\xff\x00\xff\xff' * ((n - got) // 4); unknown += (n - got) // 4; break
            take = min(n - got, pitch - lcol)
            avail = max(0, min(take, rowbytes - lcol))
            if avail > 0:
                out[o + got:o + got + avail] = data[lrow * rowbytes + lcol: lrow * rowbytes + lcol + avail]
            if avail < take:
                miss = take - avail
                out[o + got + avail:o + got + take] = b'\xff\x00\xff\xff' * (miss // 4); unknown += miss // 4
            got += take; lcol += take
            if lcol >= pitch: lrow += 1; lcol = 0
open(dst, 'wb').write(out)
print(f"pitch {pitch}, unknown px {unknown} ({100*unknown/(W*H):.1f}%)")
