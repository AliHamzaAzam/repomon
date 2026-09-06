"""Abstract Repomon identities: independent forms held around a shared center."""
from pathlib import Path
import math,json,subprocess
from xml.etree import ElementTree as ET
OUT=Path(__file__).resolve().parent
BG='#F7F5F0'; INK='#253B47'; ORANGE='#EF7846'; MUTED='#727D7C'
def rect(x,y,w,h,r=0,fill=INK):return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>'
def circle(x,y,r,fill=INK):return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"/>'
def path(d,fill=INK,extra=''):return f'<path d="{d}" fill="{fill}" {extra}/>'
def wire(d,w=24):return path(d,'none',f'stroke="{INK}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"')
def txt(x,y,t,s=20,weight=400,anchor='start',fill=INK):return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{s}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}">{t}</text>'
def svg(b,w=256,h=256):return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{b}</svg>'
def masked(mark,cut,id):return f'<defs><mask id="{id}"><rect width="256" height="256" fill="white"/>{cut}</mask></defs><g mask="url(#{id})">{mark}</g>'
def place(mark,x,y,s,mono=False):
    for name in ['phase-cut','duet-cut','fold-cut']:
        mark=mark.replace(name,f'{name}-{x}-{y}-{s}-{mono}')
    return f'<g transform="translate({x} {y}) scale({s/256})">{mark.replace(ORANGE,INK) if mono else mark}</g>'
items=[]
# 01: paired crescent currents enclosing a lens defined by two circular arcs.
upper=path('M24 102A122 122 0 0 1 232 90A176 176 0 0 0 24 102Z')
lens=path('M99 128A34 34 0 0 1 157 128A34 34 0 0 1 99 128Z',ORANGE)
mark=upper+f'<g transform="translate(0 256) scale(1 -1)">{upper}</g>'+lens
items.append(('01','Drift','Independent currents, held in balance.',mark))
# 02: two rounded, leaning masses share a center of gravity.
block='<g transform="translate(0 -8)">'+path('M52 32H134L196 104H80A40 40 0 0 1 40 64V44A12 12 0 0 1 52 32Z')+'</g>'
mark=block+f'<g transform="rotate(180 128 128)">{block}</g>'+circle(128,128,22,ORANGE)
items.append(('02','Counterweight','A shared center of gravity.',mark))
# 03: a circular field divided into parallel activity bands, with a focal core.
cuts='<g transform="rotate(-32 128 128)">'+''.join(rect(0,y,256,16,0,'black') for y in [68,120,172])+'</g>'
mark=masked(circle(128,128,96),cuts,'phase-cut')+circle(128,128,26,ORANGE)
items.append(('03','Phase','Parallel activity forms one whole.',mark))
# 04: two deep rounded folds enclose the same square core without touching it.
base=rect(40,40,176,176,58)
cuts=rect(77,77,102,102,25,'black')+rect(114,16,28,224,0,'black')
mark='<g transform="rotate(-35 128 128)">'+masked(base,cuts,'duet-cut')+'</g>'+rect(102,102,52,52,14,ORANGE)
items.append(('04','Duet','Separate forms, one protected center.',mark))
# 05: a faceted hexagonal band with two opposed cuts; the focal diamond is centered.
def polygon(radius,angle=30,fill=INK):
    coords=' '.join(f'{128+radius*math.cos(math.radians(angle+60*i)):.4f},{128+radius*math.sin(math.radians(angle+60*i)):.4f}' for i in range(6))
    return f'<polygon points="{coords}" fill="{fill}"/>'
cuts=polygon(57,30,'black')+'<g transform="rotate(30 128 128)">'+rect(114,10,28,236,0,'black')+'</g>'
mark=masked(polygon(105),cuts,'fold-cut')+'<g transform="rotate(30 128 128)">'+rect(104,104,48,48,6,ORANGE)+'</g>'
items.append(('05','Facet','A precise, folded collective.',mark))
# 06: two broad waves resonate around one quiet central capsule.
wave=wire('M30 68A52 52 0 0 1 104 68A52 52 0 0 0 178 68H226',30)
mark=wave+f'<g transform="rotate(180 128 128)">{wave}</g>'+rect(92,114,72,28,14,ORANGE)
items.append(('06','Resonance','Many rhythms, shared intent.',mark))

manifest=[]
for n,name,caption,mark in items:
    stem=f'{n}-{name.lower()}'
    for mono in [False,True]:
        (OUT/f'{stem}{"-mono" if mono else ""}.svg').write_text(svg(place(mark,0,0,256,mono)))
    board=rect(0,0,1200,850,0,BG)+txt(56,60,f'{n} / {name}',25,600)+txt(1144,60,'REPOMON',17,600,'end')
    board+=place(mark,390,124,420)+txt(600,632,'repomon',62,600,'middle')+txt(600,752,caption,23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(board,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{n} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))
body=rect(0,0,1740,1400,0,BG)+txt(60,65,'REPOMON / ABSTRACT IDENTITIES',18,600)+txt(60,123,'Independent forms. Shared intent.',40,500)
for i,(n,name,caption,mark) in enumerate(items):
    x=30+(i%3)*560;y=166+(i//3)*545
    body+=place(mark,x+146,y+20,268)+txt(x+280,y+338,f'{n} / {name}',27,600,'middle')
    body+=txt(x+280,y+380,caption,19,anchor='middle',fill=MUTED)
    body+=place(mark,x+190,y+420,64,True)+place(mark,x+300,y+432,40,True)
body+=txt(60,1347,'Centered orange core · Geometric vector construction · One-color studies under each mark',18,fill=MUTED)
(OUT/'overview.svg').write_text(svg(body,1740,1400))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for f in OUT.glob('*.svg'):ET.parse(f)
print('Six abstract identities rendered; SVG structure checked.')
