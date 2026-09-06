"""Four focused refinements of the user's selected lane-hub and repo-grid directions."""
from pathlib import Path
import subprocess,json
OUT=Path(__file__).resolve().parent
BG='#F6F5F0'; INK='#253B47'; ORANGE='#EF7846'
def rect(x,y,w,h,r=0,fill=INK):return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{fill}"/>'
def wire(d,w=24):return f'<path d="{d}" fill="none" stroke="{INK}" stroke-width="{w}" stroke-linecap="round" stroke-linejoin="round"/>'
def txt(x,y,t,s=20,weight=400,anchor='start',fill=INK):return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{s}" font-weight="{weight}" text-anchor="{anchor}" fill="{fill}">{t}</text>'
def svg(b,w=256,h=256):return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{b}</svg>'
def place(mark,x,y,size,mono=False):return f'<g transform="translate({x} {y}) scale({size/256})">{mark.replace(ORANGE,INK) if mono else mark}</g>'
items=[]
# A: three independent lanes, using tangent quarter circles to reach a shared square core.
a=wire('M48 56H96A40 40 0 0 1 136 96A32 32 0 0 0 168 128H194 M48 128H194 M48 200H96A40 40 0 0 0 136 160A32 32 0 0 1 168 128',24)
a+=''.join(rect(28,y-20,40,40,11) for y in (56,128,200))+rect(167,101,54,54,15,ORANGE)
items.append(('A','Lane Hub','Closest to 01','Three independent work lanes meet one command module.',a))
# B: four mirrored routes; independent square repo endpoints and one shared core.
b=''
for sx,sy in [(1,1),(-1,1),(1,-1),(-1,-1)]:
    b+=f'<g transform="translate(128 128) scale({sx} {sy}) translate(-128 -128)">'+wire('M48 48H72A32 32 0 0 1 104 80V104A24 24 0 0 0 128 128',22)+rect(28,28,40,40,11)+'</g>'
b+=rect(101,101,54,54,15,ORANGE)
items.append(('B','Repo Junction','01 + 04','Four repository endpoints connect to the same control center.',b))
# C: four independent corners with diagonal inward links. Geometric frame is preserved.
c=''
for angle in (0,90,180,270):
    c+=f'<g transform="rotate({angle} 128 128)">'+wire('M44 98V66A22 22 0 0 1 66 44H98',26)+wire('M53 53L108 108',20)+'</g>'
c+=rect(98,98,60,60,16,ORANGE)
items.append(('C','Command Mesh','A connected 04','Separate workspace corners form one connected system.',c))
# D: shorter corner modules frame an isolated core; gaps keep repositories distinct.
d=''
for angle in (0,90,180,270):
    d+=f'<g transform="rotate({angle} 128 128)">'+wire('M48 98V70A22 22 0 0 1 70 48H98',28)+'</g>'
d+=rect(91,91,74,74,20,ORANGE)
items.append(('D','Shared Core','Closest to 04','Independent workspaces surround one prominent command core.',d))

manifest=[]
for letter,name,family,meaning,mark in items:
    stem=f'{letter.lower()}-{name.lower().replace(" ","-")}'
    (OUT/f'{stem}.svg').write_text(svg(mark))
    (OUT/f'{stem}-mono.svg').write_text(svg(mark.replace(ORANGE,INK)))
    card=rect(0,0,1200,850,0,BG)+txt(56,60,f'{letter} / {name}',25,600)+txt(1144,60,family,17,anchor='end')
    card+=place(mark,390,134,420)+txt(600,628,'repomon',62,600,anchor='middle')
    card+=txt(600,748,meaning,23,anchor='middle')
    (OUT/f'{stem}-study.svg').write_text(svg(card,1200,850))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=f'{letter} / {name}',src=f'{stem}-study.png',output=f'{stem}-study.png'))

body=rect(0,0,1760,1040,0,BG)+txt(60,68,'REPOMON / FOCUSED EXPLORATION',18,600)+txt(60,126,'Connected lanes. Shared command.',40,500)
for i,(letter,name,family,meaning,mark) in enumerate(items):
    x=40+i*430
    body+=place(mark,x+67,208,296)
    body+=txt(x+215,568,f'{letter} / {name}',27,600,anchor='middle')+txt(x+215,610,family,18,anchor='middle',fill='#6C7A7B')
    body+=place(mark,x+125,688,76,True)+place(mark,x+251,701,48,True)
    body+=txt(x+215,819,'ONE-COLOR CHECK',12,anchor='middle',fill='#6C7A7B')
body+=txt(60,944,'A keeps the flow of 01. D keeps the separation of 04. B and C explore the connection between them.',20)
body+=txt(60,995,'Original editable SVG marks · Same palette throughout · Platform icons follow selection.',17,fill='#6C7A7B')
(OUT/'overview.svg').write_text(svg(body,1760,1040))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
print('Created four SVG concepts, monochrome variants, study boards, and overview.')
