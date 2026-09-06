"""Repomon junction structure exploration, following user's A preference."""
from pathlib import Path
import subprocess,json
from xml.etree import ElementTree as ET
OUT=Path(__file__).resolve().parent
BG='#F6F5F0'; INK='#253B47'; ACCENT='#EF7846'; MUTED='#68797C'
def rect(x,y,w,h,r=0,fill=INK):return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>'
def circle(x,y,r,fill=INK):return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"/>'
def wire(d,w=22):return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>'
def txt(x,y,t,s=20,weight=400,anchor='start',fill=INK):return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{s}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}">{t}</text>'
def svg(b,w=256,h=256):return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{b}</svg>'
def place(mark,x,y,s,mono=False):
    mark=mark.replace('manifold-cut',f'manifold-cut-{x}-{y}-{s}-{mono}')
    return f'<g transform="translate({x} {y}) scale({s/256})">{mark.replace(ACCENT,INK) if mono else mark}</g>'
nodes=lambda:''.join(rect(26,y-18,36,36,10) for y in [56,128,200])
items=[]
# 1: lanes dock independently, not at one merged bottleneck.
p=wire('M44 56H112A40 40 0 0 1 152 96H184 M44 128H184 M44 200H112A40 40 0 0 0 152 160H184',18)+nodes()+circle(190,128,35,ACCENT)
items.append(('01','Ports','Independent lanes dock at one hub.',p))
# 2: three channels cut into a single closed circular body.
outer=rect(30,40,104,176)+circle(134,128,88)
cuts=rect(12,78,112,32,0,'black')+circle(124,94,16,'black')+rect(12,146,112,32,0,'black')+circle(124,162,16,'black')
p=f'<defs><mask id="manifold-cut"><rect width="256" height="256" fill="white"/>{cuts}</mask></defs><g mask="url(#manifold-cut)">{outer}</g>'+rect(156,103,50,50,14,ACCENT)
items.append(('02','Manifold','Three channels inside one solid mark.',p))
# 3: unequal arrival points; upper entry at 120, lower entry at 164.
p=wire('M44 56H68A26 26 0 0 1 94 82V102A26 26 0 0 0 120 128H204 M44 128H204 M44 200H112A26 26 0 0 0 138 174V154A26 26 0 0 1 164 128',22)+nodes()+rect(180,103,50,50,14,ACCENT)
items.append(('03','Handoff','Staggered paths share one control point.',p))
# 4: rotate the preferred flow into an upright, compact fleet emblem.
up=wire('M46 56H98A40 40 0 0 1 138 96A32 32 0 0 0 170 128H192 M46 128H192 M46 200H98A40 40 0 0 0 138 160A32 32 0 0 1 170 128',22)+nodes()+rect(166,102,52,52,15,ACCENT)
p=f'<g transform="rotate(-90 128 128)">{up}</g>'
items.append(('04','Uplink','An upright fleet feeding shared control.',p))
# 5: planar 45 degree rails create a sharper directional emblem.
p=wire('M44 56H88L160 128H202 M44 128H202 M44 200H88L160 128',24)+nodes()+rect(176,102,52,52,12,ACCENT)
items.append(('05','Switch','A sharper, more directional connection.',p))
# 6: a repository-like enclosure with three inlet lanes and a protected core.
p=wire('M102 40H174A44 44 0 0 1 218 84V172A44 44 0 0 1 174 216H102',24)+wire('M42 72H106A40 40 0 0 1 146 112V128 M42 128H164 M42 184H106A40 40 0 0 0 146 144V128',20)
p+=''.join(rect(24,y-17,34,34,9) for y in [72,128,184])+rect(138,102,52,52,15,ACCENT)
items.append(('06','Harbor','Connected agents within one workspace.',p))
manifest=[]
for num,name,caption,mark in items:
    stem=f'{num}-{name.lower()}'
    for mono in [False,True]:
        (OUT/f'{stem}{"-mono" if mono else ""}.svg').write_text(svg(place(mark,0,0,256,mono)))
    study=rect(0,0,1200,850,0,BG)+txt(56,60,f'{num} / {name}',25,600)+txt(1144,60,'REPOMON',17,600,'end')
    study+=place(mark,390,136,420)+txt(600,632,'repomon',62,600,'middle')+txt(600,752,caption,23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(study,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{num} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))
body=rect(0,0,1740,1400,0,BG)+txt(60,65,'REPOMON / JUNCTION EXPLORATION',18,600)+txt(60,122,'Six ways to bring the fleet together.',40,500)
for i,(num,name,caption,mark) in enumerate(items):
    col=i%3; row=i//3; x=30+col*560; y=164+row*545
    body+=place(mark,x+146,y+20,268)
    body+=txt(x+280,y+338,f'{num} / {name}',27,600,'middle')+txt(x+280,y+379,caption,19,anchor='middle',fill=MUTED)
    body+=place(mark,x+190,y+420,64,True)+place(mark,x+300,y+432,40,True)
body+=txt(60,1347,'Same palette. New structures. Editable vectors, with a one-color check under each mark.',18,fill=MUTED)
(OUT/'overview.svg').write_text(svg(body,1740,1400))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for f in OUT.glob('*.svg'):ET.parse(f)
print('Six structures rendered and SVG syntax checked.')
