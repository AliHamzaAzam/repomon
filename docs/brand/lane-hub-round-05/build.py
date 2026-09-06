"""Focused lane-hub refinements of the user's selected A direction."""
from pathlib import Path
import json, subprocess
from xml.etree import ElementTree as ET
OUT=Path(__file__).resolve().parent
BG='#F6F5F0'; INK='#253B47'; ACCENT='#EF7846'
def rect(x,y,w,h,r=0,fill=INK):return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>'
def circle(x,y,r,fill=INK):return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"/>'
def wire(d,w):return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>'
def txt(x,y,t,s=20,weight=400,anchor='start',fill=INK):return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{s}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}">{t}</text>'
def svg(body,w=256,h=256):return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>'
def place(mark,x,y,s,mono=False):return f'<g transform="translate({x} {y}) scale({s/256})">{mark.replace(ACCENT,INK) if mono else mark}</g>'
items=[]
# A1: balanced, slightly slimmer implementation of the selected silhouette.
d='M46 56H98A40 40 0 0 1 138 96A32 32 0 0 0 170 128H194 M46 128H194 M46 200H98A40 40 0 0 0 138 160A32 32 0 0 1 170 128'
mark=wire(d,22)+''.join(rect(28,y-18,36,36,10) for y in [56,128,200])+rect(168,102,52,52,15,ACCENT)
items.append(('A1','Balanced','Closest to your choice',mark))
# A2: compact three-pronged dock. Shorter routes carry more weight at small sizes.
d='M50 64H100A36 36 0 0 1 136 100A28 28 0 0 0 164 128H190 M50 128H190 M50 192H100A36 36 0 0 0 136 156A28 28 0 0 1 164 128'
mark=wire(d,28)+''.join(rect(28,y-21,42,42,12) for y in [64,128,192])+rect(162,100,56,56,16,ACCENT)
items.append(('A2','Compact','Bolder, tighter proportions',mark))
# A3: three horizontal repositories connect through a rounded shared bus.
d='M46 56H112A24 24 0 0 1 136 80V176A24 24 0 0 1 112 200H46 M46 128H192'
mark=wire(d,24)+''.join(rect(28,y-19,38,38,9) for y in [56,128,200])+rect(166,102,52,52,12,ACCENT)
items.append(('A3','Shared Bus','A more architectural junction',mark))
# A4: circular agent nodes retain the tangent curved routing of A1.
d='M46 56H98A40 40 0 0 1 138 96A32 32 0 0 0 170 128H192 M46 128H192 M46 200H98A40 40 0 0 0 138 160A32 32 0 0 1 170 128'
mark=wire(d,22)+''.join(circle(46,y,20) for y in [56,128,200])+circle(192,128,29,ACCENT)
items.append(('A4','Round','A softer agent-to-control network',mark))
manifest=[]
for code,name,caption,mark in items:
    stem=f'{code.lower()}-{name.lower().replace(" ","-")}'
    (OUT/f'{stem}.svg').write_text(svg(mark))
    (OUT/f'{stem}-mono.svg').write_text(svg(mark.replace(ACCENT,INK)))
    study=rect(0,0,1200,850,0,BG)+txt(56,60,f'{code} / {name}',25,600)+txt(1144,60,caption,17,anchor='end')
    study+=place(mark,390,134,420)+txt(600,632,'repomon',62,600,anchor='middle')
    study+=txt(600,752,'Three independent work lanes. One command point.',23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(study,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{code} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))
body=rect(0,0,1760,1010,0,BG)+txt(60,66,'REPOMON / LANE HUB REFINEMENTS',18,600)+txt(60,124,'Three lanes. One command point.',40,500)
for i,(code,name,caption,mark) in enumerate(items):
    x=40+i*430
    body+=place(mark,x+67,208,296)+txt(x+215,550,f'{code} / {name}',27,600,anchor='middle')
    body+=txt(x+215,594,caption,18,anchor='middle',fill='#6C7A7B')
    body+=place(mark,x+126,677,76,True)+place(mark,x+250,690,48,True)
    body+=txt(x+215,807,'ONE-COLOR CHECK',12,anchor='middle',fill='#6C7A7B')
body+=txt(60,926,'Same idea and palette throughout. Compare the curve, node shape, and visual weight.',20)
body+=txt(60,976,'Editable SVG symbols · Final wordmark and Mac / Windows icons follow selection.',17,fill='#6C7A7B')
(OUT/'overview.svg').write_text(svg(body,1760,1010))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for p in OUT.glob('*.svg'): ET.parse(p)
print('Rendered four focused A variants and verified SVG syntax.')
