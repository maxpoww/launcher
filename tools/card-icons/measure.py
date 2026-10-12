import sys,re,statistics
W=H=0; px={}
for l in sys.stdin:
    m=re.match(r'(\d+),(\d+): \((\d+),(\d+),(\d+)',l)
    if not m:
        m2=re.search(r'ImageMagick pixel enumeration: (\d+),(\d+)',l)
        if m2: W,H=int(m2.group(1)),int(m2.group(2))
        continue
    x,y,r,g,b=map(int,m.groups())
    if r>255: r,g,b=r/257,g/257,b/257
    px[(x,y)]=max(r,g,b)   # value: the accent arrow is coloured
bg=statistics.median(px.values())
disc=lambda x,y: px[(x,y)]>bg+4
cols=[any(disc(x,y) for y in range(H)) for x in range(W)]
runs=[];s=None
for x,c in enumerate(cols+[False]):
    if c and s is None: s=x
    if not c and s is not None:
        if x-s>=20: runs.append((s,x-1))
        s=None
names=sys.argv[1].split(',')
for i,(x0,x1) in enumerate(runs[:len(names)]):
    ys=[y for y in range(H) if any(disc(x,y) for x in range(x0,x1+1))]
    y0,y1=min(ys),max(ys)
    cx,cy=(x0+x1+1)/2,(y0+y1+1)/2
    vals=[px[(x,y)] for x in range(x0,x1+1) for y in range(y0,y1+1)]
    base=statistics.median(vals); top=max(vals)
    if top-base<25: print(names[i],'no ink'); continue
    # the button's middle: the centre of its whole disc, edges and all (not of its box)
    d=lambda x,y: max(0.0,min(1.0,(px[(x,y)]-bg)/max(1.0,base-bg)))
    xs=range(max(0,x0-2),min(W,x1+3)); ys2=range(max(0,y0-2),min(H,y1+3))
    dt=sum(d(x,y) for x in xs for y in ys2)
    cx=sum(d(x,y)*(x+.5) for x in xs for y in ys2)/dt; cy=sum(d(x,y)*(y+.5) for x in xs for y in ys2)/dt
    a=lambda x,y: max(0.0,min(1.0,(px[(x,y)]-base)/(top-base)))
    cov_c=[max(a(x,y) for y in range(y0+2,y1-1)) for x in range(x0+2,x1-1)]
    cov_r=[max(a(x,y) for x in range(x0+2,x1-1)) for y in range(y0+2,y1-1)]
    def ext(c,o):
        idx=[k for k,v in enumerate(c) if v>=0.5]
        lo,hi=idx[0],idx[-1]
        lo_e=lo-(c[lo-1] if lo>0 else 0); hi_e=hi+1+(c[hi+1] if hi+1<len(c) else 0)
        return o+lo_e,o+hi_e
    l,r=ext(cov_c,x0+2); t,b=ext(cov_r,y0+2)
    tot=sum(a(x,y) for x in range(x0+2,x1-1) for y in range(y0+2,y1-1))
    mx=sum(a(x,y)*(x+.5) for x in range(x0+2,x1-1) for y in range(y0+2,y1-1))/tot-cx
    my=sum(a(x,y)*(y+.5) for x in range(x0+2,x1-1) for y in range(y0+2,y1-1))/tot-cy
    print(f"{names[i]:10s} weight {mx:+5.2f} {my:+5.2f} |", end=" ")
    print(f"{names[i]:10s} button {x1-x0+1}x{y1-y0+1}  ink {r-l:4.1f} x {b-t:4.1f}  area-side {((r-l)*(b-t))**.5:4.1f}  off right {((l+r)/2-cx):+5.2f}  down {((t+b)/2-cy):+5.2f}")
