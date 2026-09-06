"""Current Repomon Command Mesh, plus three identity-preserving explorations."""
from pathlib import Path
from xml.etree import ElementTree as ET
import json,subprocess,math,base64
OUT=Path(__file__).resolve().parent
REPO=OUT.parents[2]
BG='#F6F5F0'; INK='#253B47'; ORANGE='#EF7846'; MUTED='#6F7B7C'
def rect(x,y,w,h,r=0,fill=INK):return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>'
def txt(x,y,t,s=20,weight=400,anchor='start',fill=INK):return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{s}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}">{t}</text>'
def svg(b,w=256,h=256):return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{b}</svg>'
def place(mark,x,y,s,mono=False):return f'<g transform="translate({x} {y}) scale({s/256})">{mark.replace(ORANGE,INK) if mono else mark}</g>'

# Read the actual shipped vector. Normalize around its command square, without redrawing it.
root=ET.parse(REPO/'apps/desktop/public/favicon.svg').getroot()
ns={'s':'http://www.w3.org/2000/svg'}
source=root.find('s:g[@class="fg"]',ns)
rectangles=[]
for r in source:
    x,y,w,h=(float(r.attrib[k]) for k in ('x','y','width','height'))
    rectangles.append(((x-627)*.28+128,(y-626)*.28+128,w*.28,h*.28))
source_mark=''.join(rect(*r) for r in rectangles)+rect(112.6,112.6,30.8,30.8,0,ORANGE)

# Boundary extraction preserves the union of overlapping source rectangles. The softened
# study rounds the actual contour, rather than rounding each overlapping rectangle.
xs=sorted({round(v,6) for x,y,w,h in rectangles for v in (x,x+w)})
ys=sorted({round(v,6) for x,y,w,h in rectangles for v in (y,y+h)})
filled=set()
for i in range(len(xs)-1):
    for j in range(len(ys)-1):
        cx=(xs[i]+xs[i+1])/2;cy=(ys[j]+ys[j+1])/2
        if any(x-1e-6<cx<x+w+1e-6 and y-1e-6<cy<y+h+1e-6 for x,y,w,h in rectangles):filled.add((i,j))
edges=set()
for i,j in filled:
    a=(xs[i],ys[j]);b=(xs[i+1],ys[j]);c=(xs[i+1],ys[j+1]);d=(xs[i],ys[j+1])
    for neighbour,p,q in [((i,j-1),a,b),((i+1,j),b,c),((i,j+1),c,d),((i-1,j),d,a)]:
        if neighbour not in filled:edges.add((p,q))
loops=[]
while edges:
    start,end=next(iter(edges));edges.remove((start,end));pts=[start];cur=end
    while cur!=start:
        pts.append(cur)
        outgoing=[e for e in edges if e[0]==cur]
        if len(outgoing)!=1:raise ValueError('Ambiguous source boundary')
        e=outgoing[0];edges.remove(e);cur=e[1]
    # Drop collinear grid intersections, retaining all true corners.
    keep=[]
    for i,p in enumerate(pts):
        a=pts[i-1];b=pts[(i+1)%len(pts)]
        if abs((p[0]-a[0])*(b[1]-p[1])-(p[1]-a[1])*(b[0]-p[0]))>1e-5:keep.append(p)
    loops.append(keep)
def rounded_contour(pts,r=3.5):
    sections=[]
    for i,p in enumerate(pts):
        a=pts[i-1];b=pts[(i+1)%len(pts)]
        da=math.dist(a,p);db=math.dist(b,p);rr=min(r,da*.4,db*.4)
        before=(p[0]+(a[0]-p[0])*rr/da,p[1]+(a[1]-p[1])*rr/da)
        after=(p[0]+(b[0]-p[0])*rr/db,p[1]+(b[1]-p[1])*rr/db)
        sections.append((before,p,after))
    d=f'M{sections[0][0][0]} {sections[0][0][1]}'
    for i,(before,p,after) in enumerate(sections):
        if i:d+=f'L{before[0]} {before[1]}'
        d+=f'Q{p[0]} {p[1]} {after[0]} {after[1]}'
    return d+'Z'
soft_mark=f'<path fill="{INK}" fill-rule="evenodd" d="'+''.join(rounded_contour(l) for l in loops)+'"/>'+rect(112.6,112.6,30.8,30.8,0,ORANGE)
def route(points,width,r=0):
    if not r:
        d='M'+'L'.join(f'{x} {y}' for x,y in points)
    else:
        d=f'M{points[0][0]} {points[0][1]}'
        for i,p in enumerate(points[1:-1],1):
            a=points[i-1];b=points[i+1];da=math.dist(a,p);db=math.dist(b,p);rr=min(r,da/2,db/2)
            t=(p[0]+(a[0]-p[0])*rr/da,p[1]+(a[1]-p[1])*rr/da);u=(p[0]+(b[0]-p[0])*rr/db,p[1]+(b[1]-p[1])*rr/db)
            d+=f'L{t[0]} {t[1]}Q{p[0]} {p[1]} {u[0]} {u[1]}'
        d+=f'L{points[-1][0]} {points[-1][1]}'
    return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{width}" stroke-linecap="butt" stroke-linejoin="miter"/>'
opened=route([(108,48),(48,48),(48,124),(84,124),(84,164)],22,8)
opened+=route([(88,88),(152,88),(152,48),(208,48),(208,208),(160,208)],22,8)
opened+=route([(176,124),(208,124)],22)
opened+=route([(48,172),(48,208),(128,208),(128,172),(172,172)],22,8)+rect(112,112,32,32,0,ORANGE)
stepped=route([(116,40),(40,40),(40,128),(80,128),(80,176)],26)
stepped+=route([(80,88),(152,88),(152,40),(216,40),(216,216),(168,216)],26)
stepped+=route([(184,128),(216,128)],26)
stepped+=route([(40,176),(40,216),(128,216),(128,176),(176,176)],26)+rect(111,111,34,34,0,ORANGE)
items=[('01','Source Mesh','Current geometry, flat color.',source_mark),('02','Soft Mesh','The same interlocks, gentler corners.',soft_mark),('03','Open Mesh','Wider channels and clearer separation.',opened),('04','Stepped Mesh','A bolder, more deliberate grid.',stepped)]
manifest=[]
for n,name,caption,mark in items:
    stem=f'{n}-{name.lower().replace(" ","-")}'
    for mono in [False,True]:(OUT/f'{stem}{"-mono" if mono else ""}.svg').write_text(svg(place(mark,0,0,256,mono)))
    board=rect(0,0,1200,850,0,BG)+txt(56,60,f'{n} / {name}',25,600)+txt(1144,60,'REPOMON',17,600,'end')
    board+=place(mark,390,124,420)+txt(600,632,'repomon',62,600,'middle')+txt(600,752,caption,23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(board,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{n} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))
body=rect(0,0,1760,1110,0,BG)+txt(60,66,'REPOMON / COMMAND MESH EVOLUTION',18,600)+txt(60,126,'Built from the identity you already have.',40,500)
icon=base64.b64encode((REPO/'docs/logo.png').read_bytes()).decode()
body+=f'<image x="1590" y="34" width="100" height="100" href="data:image/png;base64,{icon}"/>'+txt(1640,160,'CURRENT ICON',12,anchor='middle',fill=MUTED)
for i,(n,name,caption,mark) in enumerate(items):
    x=40+i*430
    body+=place(mark,x+57,228,316)+txt(x+215,606,f'{n} / {name}',26,600,'middle')+txt(x+215,650,caption,18,anchor='middle',fill=MUTED)
    body+=place(mark,x+128,728,76,True)+place(mark,x+249,741,48,True)+txt(x+215,860,'ONE-COLOR CHECK',12,anchor='middle',fill=MUTED)
body+=txt(60,997,'Interlocking paths. Unequal lengths. A square command node. The original mesh remains the anchor.',20)
body+=txt(60,1057,'01 uses the actual shipped vector geometry. 02–04 explore how that identity can evolve.',17,fill=MUTED)
(OUT/'overview.svg').write_text(svg(body,1760,1110))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for f in OUT.glob('*.svg'):ET.parse(f)
print(f'Rendered source and three evolutions from {len(rectangles)} actual source rectangles; {len(loops)} union contours.')
