"""Repomon: original fleet identities with editable geometric construction."""
from pathlib import Path
import subprocess, json, math
from xml.etree import ElementTree as ET
OUT=Path(__file__).resolve().parent
BG='#F6F5F0'; INK='#253B47'; ACCENT='#EF7846'; MUTED='#677777'

def rect(x,y,w,h,rx=0,fill='currentColor'):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}"/>'
def circle(x,y,r,fill='currentColor'):
    return f'<circle cx="{x}" cy="{y}" r="{r}" fill="{fill}"/>'
def path(d,fill='currentColor',extra=''):
    return f'<path d="{d}" fill="{fill}" {extra}/>'
def wire(d,width=24):
    return path(d,'none',f'stroke="currentColor" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round"')
def txt(x,y,t,size=20,weight=400,fill=INK,anchor='start'):
    return f'<text x="{x}" y="{y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}">{t}</text>'
def svg(body,w=256,h=256,label='Repomon'):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{label}">{body}</svg>'
def grid(parts):
    return f'<g fill="none" stroke="#738F95" stroke-width="0.85">{parts}</g>'
def gc(x,y,r): return f'<circle cx="{x}" cy="{y}" r="{r}"/>'
def gl(d): return f'<path d="{d}"/>'
def gr(x,y,w,h,rx=0): return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}"/>'

items=[]
# Three independent agent endpoints route to one shared command node.
c_paths='M48 56H100A40 40 0 0 1 140 96A32 32 0 0 0 172 128H196 M48 128H196 M48 200H100A40 40 0 0 0 140 160A32 32 0 0 1 172 128'
c_mark=wire(c_paths,24)+''.join(rect(28,y-20,40,40,10) for y in [56,128,200])+circle(196,128,29,ACCENT)
c_grid=gc(100,96,40)+gc(172,96,32)+gc(100,160,40)+gc(172,160,32)+gc(196,128,29)+''.join(gr(28,y-20,40,40,10) for y in [56,128,200])+gl('M12 56H244M12 128H244M12 200H244M140 16V240')
items.append(dict(id='01-conductor',name='Conductor',lead='Many agents. One control point.',detail='Separate repo lanes meet a shared command node.',geometry='Three lanes · R40 / R32 routing · one R29 hub',mark=c_mark,grid=c_grid))
# A monogram whose stem is deliberately composed of three equal independent blocks.
r_body=rect(40,32,36,192)+path('M76 32H140A60 60 0 0 1 140 152H116L188 224H236L154 142A60 60 0 0 0 140 32Z')
# Use a single conventional circular bowl union plus a controlled diagonal leg.
r_body=rect(40,32,36,192)+rect(58,32,82,120)+circle(140,92,60)+path('M108 136H156L236 224H188Z')
r_cut=rect(76,68,64,48,0,'black')+circle(140,92,24,'black')+rect(32,88,44,12,0,'black')+rect(32,156,44,12,0,'black')
r_mark=f'<defs><mask id="r-mask"><rect width="256" height="256" fill="white"/>{r_cut}</mask></defs><g mask="url(#r-mask)">{r_body}</g>'
r_grid=gc(140,92,60)+gc(140,92,24)+gr(40,32,36,192)+gr(76,68,64,48)+gl('M16 32H244M16 88H244M16 100H244M16 156H244M16 168H244M16 224H244M108 136L188 224M156 136L236 224')
items.append(dict(id='02-fleet-r',name='Fleet R',lead='Three lanes become Repomon.',detail='A recognizable R with an independently segmented spine.',geometry='R60 bowl · R24 counter · three spine modules',mark=r_mark,grid=r_grid))
# Three distinct terminal lanes enclosed by one common control boundary.
l_frame=wire('M212 80V48A20 20 0 0 0 192 28H64A20 20 0 0 0 44 48V208A20 20 0 0 0 64 228H192A20 20 0 0 0 212 208V176',20)
l_mark=l_frame+''.join(rect(82,y,88,24,12) for y in [70,116,162])+circle(212,128,22,ACCENT)
l_grid=gr(44,28,168,200,20)+''.join(gr(82,y,88,24,12)+gc(94,y+12,12)+gc(158,y+12,12) for y in [70,116,162])+gc(212,128,22)+gl('M16 82H240M16 128H240M16 174H240M128 8V248')
items.append(dict(id='03-mission',name='Mission',lead='Every session in one view.',detail='A fleet of terminal lanes, with one attention signal.',geometry='Three equal capsules · one frame · detached signal',mark=l_mark,grid=l_grid))
# Four repo corners form one shared interior and stay distinct via consistent gaps.
q_mark=''; q_grid=''
for a in [0,90,180,270]:
    q_mark+=f'<g transform="rotate({a} 128 128)">'+wire('M42 106V62A20 20 0 0 1 62 42H106',28)+'</g>'
    q_grid+=f'<g transform="rotate({a} 128 128)">'+gc(62,62,20)+gl('M42 128V42H128')+'</g>'
q_mark+=rect(99,99,58,58,12,ACCENT)
q_grid+=gr(99,99,58,58,12)+gl('M128 12V244M12 128H244M28 28L228 228M28 228L228 28')
items.append(dict(id='04-repo-grid',name='Repo Grid',lead='Separate repos. Shared command.',detail='Four independent workspaces surround a common core.',geometry='Four R20 corners · 90° repetition · central module',mark=q_mark,grid=q_grid))
# Eye-shaped frame: intersect two circles exactly; subtract a smaller lens.
e_defs='<defs><clipPath id="lens-outer">'+circle(128,196,124)+'</clipPath><clipPath id="lens-inner">'+circle(128,186,96)+'</clipPath><mask id="eye-mask"><rect width="256" height="256" fill="white"/><g clip-path="url(#lens-inner)">'+circle(128,70,96,'black')+'</g></mask></defs>'
e_mark=e_defs+'<g clip-path="url(#lens-outer)" mask="url(#eye-mask)">'+circle(128,60,124)+'</g>'+circle(82,128,12)+circle(128,128,18,ACCENT)+circle(174,128,12)
e_grid=gc(128,196,124)+gc(128,60,124)+gc(128,186,96)+gc(128,70,96)+gc(82,128,12)+gc(128,128,18)+gc(174,128,12)+gl('M8 128H248M128 8V248')
items.append(dict(id='05-overwatch',name='Overwatch',lead='See the fleet. Spot the exception.',detail='Multiple agents stay visible; the one needing you stands out.',geometry='Intersecting circles · three agent nodes · focal center',mark=e_mark,grid=e_grid))

def draw(c,x,y,s,mode='color'):
    unique=f'{x}-{y}-{s}-{mode}'
    mark=c['mark']
    for name in ['r-mask','lens-outer','lens-inner','eye-mask']:
        mark=mark.replace(name,name+unique)
    if mode=='mono': mark=mark.replace(ACCENT,'currentColor')
    if mode=='grid': mark='<g opacity="0.12">'+mark+'</g>'+grid(c['grid'])
    return f'<g transform="translate({x} {y}) scale({s/256})" color="{INK}">{mark}</g>'

manifest=[]
for c in items:
    stem=c['id']
    for mode in ['color','mono']:
        name=f'{stem}'+('-mono' if mode=='mono' else '')
        (OUT/f'{name}.svg').write_text(svg(draw(c,0,0,256,mode),label=c['name']))
    body=rect(0,0,1440,900,0,BG)+txt(64,65,'REPOMON / FLEET IDENTITIES',16,600)+txt(1376,65,stem[:2],18,anchor='end')
    body+=draw(c,140,154,420)+draw(c,850,154,420,'grid')
    body+=txt(350,635,'repomon',54,600,anchor='middle')+txt(1060,635,'GEOMETRIC CONSTRUCTION',14,fill=MUTED,anchor='middle')
    body+=path('M64 705H1376','none','stroke="#DADFD9"')+txt(64,765,c['name'],32,600)+txt(64,812,c['lead'],23)+txt(64,849,c['detail'],18,fill=MUTED)
    body+=txt(1376,818,c['geometry'],16,fill=MUTED,anchor='end')
    (OUT/f'{stem}-study.svg').write_text(svg(body,1440,900,c['name']+' study'))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=c['name'],src=f'{stem}-study.png',output=f'{stem}-study.png'))

body=rect(0,0,1800,1070,0,BG)+txt(62,67,'REPOMON',22,650)+txt(62,126,'Many repos. One mission control.',42,500)+txt(1738,67,'FLEET IDENTITIES / 03',15,anchor='end')
for i,c in enumerate(items):
    x=42+i*348
    body+=draw(c,x+40,220,256)
    body+=txt(x+168,540,c['id'][:2]+' / '+c['name'],23,500,anchor='middle')
    lines=[['Independent agents,','one control point.'],['Parallel work,','one Repomon.'],['All sessions,','one attention signal.'],['Separate workspaces,','one shared core.'],['Watch every agent.','Notice who needs you.']][i]
    for j,t in enumerate(lines): body+=txt(x+168,584+j*28,t,18,fill=MUTED,anchor='middle')
    body+=draw(c,x+70,708,76,'mono')+draw(c,x+187,720,48,'mono')
    body+=txt(x+168,839,'ONE-COLOR CHECK',12,fill=MUTED,anchor='middle')
body+=path('M62 918H1738','none','stroke="#DADFD9"')+txt(62,970,'Agent / workspace',17,fill=MUTED)+rect(250,953,18,18,4,INK)+txt(310,970,'Shared control / attention',17,fill=MUTED)+circle(552,962,10,ACCENT)
body+=txt(62,1020,'Editable SVG marks. The selected direction will be refined for macOS Liquid Glass and Windows.',18,fill=MUTED)
(OUT/'overview.svg').write_text(svg(body,1800,1070,'Five Repomon fleet logo concepts'))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
for p in OUT.glob('*.svg'): ET.parse(p)
print('Rendered five fleet identity studies, ten symbol masters, and overview; SVG XML verified.')
