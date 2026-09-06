"""Repomon: centered command-hub studies."""
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
def place(mark,x,y,s,mono=False):return f'<g transform="translate({x} {y}) scale({s/256})">{mark.replace(ACCENT,INK) if mono else mark}</g>'
def mirror(shape):return shape+f'<g transform="translate(256 0) scale(-1 1)">{shape}</g>'
core=lambda:rect(100,100,56,56,16,ACCENT)
items=[]
# C1: six independent lanes mirror around the exact canvas center.
half=wire('M38 56H58A30 30 0 0 1 88 86V98A30 30 0 0 0 118 128H128 M38 128H128 M38 200H58A30 30 0 0 0 88 170V158A30 30 0 0 1 118 128',20)
half+=''.join(rect(20,y-17,34,34,10) for y in [56,128,200])
items.append(('C1','Bilateral','Two fleets. One shared command.',mirror(half)+core()))
# C2: four terminal endpoints, with mirrored circular bends into a common center.
quarter=wire('M46 46H70A34 34 0 0 1 104 80V104A24 24 0 0 0 128 128',22)+rect(28,28,36,36,11)
p=''
for sx,sy in [(1,1),(-1,1),(1,-1),(-1,-1)]:
    p+=f'<g transform="translate(128 128) scale({sx} {sy}) translate(-128 -128)">{quarter}</g>'
items.append(('C2','Fourfold','Four workspaces meet at the core.',p+core()))
# C3: an enclosing pair of arcs provides an orbit around the centered control.
half=wire('M56 44H88A40 40 0 0 1 128 84V128 M56 212H88A40 40 0 0 0 128 172V128',22)
half+=rect(38,26,36,36,11)+rect(38,194,36,36,11)
p=mirror(half)+core()
items.append(('C3','Canopy','Upper and lower lanes share one center.',p))
# C4: independent corner workspaces connect diagonally to the central command.
p=''
for angle in [0,90,180,270]:
    p+=f'<g transform="rotate({angle} 128 128)">'+wire('M42 96V68A26 26 0 0 1 68 42H96',24)+wire('M52 52L112 112',20)+'</g>'
items.append(('C4','Command Square','A connected workspace frame.',p+rect(98,98,60,60,17,ACCENT)))
manifest=[]
for code,name,caption,mark in items:
    stem=f'{code.lower()}-{name.lower().replace(" ","-")}'
    for mono in [False,True]:
        (OUT/f'{stem}{"-mono" if mono else ""}.svg').write_text(svg(place(mark,0,0,256,mono)))
    study=rect(0,0,1200,850,0,BG)+txt(56,60,f'{code} / {name}',25,600)+txt(1144,60,'REPOMON',17,600,'end')
    study+=place(mark,390,134,420)+txt(600,632,'repomon',62,600,'middle')+txt(600,752,caption,23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(study,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{code} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))
body=rect(0,0,1760,1040,0,BG)+txt(60,66,'REPOMON / CENTERED CONTROL',18,600)+txt(60,124,'The fleet around one shared core.',40,500)
for i,(code,name,caption,mark) in enumerate(items):
    x=40+i*430
    body+=place(mark,x+67,208,296)+txt(x+215,562,f'{code} / {name}',26,600,'middle')
    body+=txt(x+215,606,caption,18,anchor='middle',fill=MUTED)
    body+=place(mark,x+126,694,76,True)+place(mark,x+250,707,48,True)
    body+=txt(x+215,824,'ONE-COLOR CHECK',12,anchor='middle',fill=MUTED)
body+=txt(60,944,'The orange hub sits at the exact geometric center of every mark.',20)
body+=txt(60,995,'Editable SVG symbols · Connected repositories and agents · Platform icons follow selection.',17,fill=MUTED)
(OUT/'overview.svg').write_text(svg(body,1760,1040))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for f in OUT.glob('*.svg'):ET.parse(f)
print('Four centered-hub concepts rendered and SVG syntax checked.')
