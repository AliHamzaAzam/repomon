"""Original Repomon vector concepts. Guides derive from the actual primitives."""
from pathlib import Path
import math
import subprocess
import json

OUT = Path(__file__).resolve().parent
BG = '#F5F4EF'
INK = '#202923'

def circle(x,y,r,**attrs):
    extra=' '.join(f'{k.replace("_","-")}="{v}"' for k,v in attrs.items())
    return f'<circle cx="{x}" cy="{y}" r="{r}" {extra}/>'

def rect(x,y,w,h,**attrs):
    extra=' '.join(f'{k.replace("_","-")}="{v}"' for k,v in attrs.items())
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" {extra}/>'

def line(x1,y1,x2,y2):
    return f'<path d="M{x1} {y1}L{x2} {y2}"/>'

def svg(body,w=256,h=256,label='Repomon logo concept'):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{label}">{body}</svg>'

def text(x,y,value,size=20,fill=INK,weight=400,anchor='start'):
    return f'<text x="{x}" y="{y}" fill="{fill}" font-family="Helvetica Neue,Arial,sans-serif" font-size="{size}" font-weight="{weight}" text-anchor="{anchor}">{value}</text>'

def guides(body):
    return '<g fill="none" stroke="#73938B" stroke-width="0.8" opacity="0.68">'+body+'</g>'

def sector(a0,a1,r0=52,r1=96):
    def p(a,r): return (128+r*math.cos(math.radians(a)),128+r*math.sin(math.radians(a)))
    a,b,c,d=p(a0,r1),p(a1,r1),p(a1,r0),p(a0,r0)
    return f'M{a[0]} {a[1]}A{r1} {r1} 0 0 1 {b[0]} {b[1]}L{c[0]} {c[1]}A{r0} {r0} 0 0 0 {d[0]} {d[1]}Z'

concepts=[]
# 01: R constructed by the union of a circular bowl, stem and a 45-degree leg.
r_outer=rect(40,36,36,184)+rect(40,36,76,36)+circle(116,100,64)+ '<path d="M94 140H146L222 216H170Z"/>'
r_inner=circle(116,100,28,fill='black')
r_mark=f'<defs><mask id="r-cut"><rect width="256" height="256" fill="white"/>{r_inner}</mask></defs><g fill="currentColor" mask="url(#r-cut)">{r_outer}</g>'
r_guides=circle(116,100,64)+circle(116,100,28)+rect(40,36,36,184)+rect(40,36,76,36)+line(20,100,236,100)+line(116,12,116,236)+line(70,116,224,270)+line(112,106,240,234)
concepts.append(dict(id='01-switchyard',name='Switchyard',color='#E9673D',tag='A letter built for branching work.',formula='R64 / R28 · 36-unit stem · 45° branch',mark=r_mark,grid=r_guides))
# 02: union of three equal circles on an equilateral triangle, with one shared counter.
hub_centers=[(128+50*math.cos(math.radians(a)),128+50*math.sin(math.radians(a))) for a in [-90,30,150]]
hub_outer=''.join(circle(x,y,56) for x,y in hub_centers)
hub_mark='<defs><mask id="hub-cut"><rect width="256" height="256" fill="white"/>'+circle(128,128,34,fill='black')+'</mask></defs><g fill="currentColor" mask="url(#hub-cut)">'+hub_outer+'</g>'
hub_guides=''.join(circle(x,y,56) for x,y in hub_centers)+circle(128,128,34)+circle(128,128,50)
for i,(x,y) in enumerate(hub_centers):
    nx,ny=hub_centers[(i+1)%3]
    hub_guides+=line(x,y,nx,ny)+line(128,128,x,y)
concepts.append(dict(id='02-common',name='Common',color='#2869CC',tag='Independent agents. One shared center.',formula='Three R56 circles · R34 counter · 120° centers',mark=hub_mark,grid=hub_guides))
# 03: an owl-like lookout reduced to a circular body, circular brow cut and eyes.
owl_cut=circle(128,20,64,fill='black')+circle(92,126,23,fill='black')+circle(164,126,23,fill='black')+'<path d="M118 157H138L128 172Z" fill="black"/>'
owl_mark=f'<defs><mask id="owl-cut"><rect width="256" height="256" fill="white"/>{owl_cut}</mask></defs><g fill="currentColor"><circle cx="128" cy="132" r="92" mask="url(#owl-cut)"/>'+circle(96,126,9)+circle(160,126,9)+'</g>'
owl_guides=circle(128,132,92)+circle(128,20,64)+circle(92,126,23)+circle(164,126,23)+circle(96,126,9)+circle(160,126,9)+line(128,8,128,242)+line(16,126,240,126)+line(36,224,220,224)
concepts.append(dict(id='03-lookout',name='Lookout',color='#215545',tag='A watchful companion for the fleet.',formula='R92 body · R64 brow · mirrored R23 eyes',mark=owl_mark,grid=owl_guides))
# 04: a three-level repository stack, each frame is the same rounded square.
stack_parts=[]
stack_guides=''
for i,(x,y) in enumerate([(34,34),(64,64),(94,94)]):
    # Only exposed upper and left frame edges remain; no hidden edges are invented.
    stack_parts.append(f'<path d="M{x+16} {y+128}H{x+12}A12 12 0 0 1 {x} {y+116}V{y+12}A12 12 0 0 1 {x+12} {y}H{x+116}A12 12 0 0 1 {x+128} {y+12}V{y+16}" fill="none" stroke="currentColor" stroke-width="18" stroke-linecap="round"/>')
    stack_guides+=rect(x,y,128,128,rx=12)+circle(x+12,y+12,12)+line(x+12,y+12,x+128,y+128)
stack_parts.append(rect(126,126,64,64,rx=12,fill='currentColor'))
stack_guides+=rect(126,126,64,64,rx=12)+line(12,12,240,240)
concepts.append(dict(id='04-workspaces',name='Workspaces',color='#7352C6',tag='Many repositories, held together.',formula='128-unit frames · 30-unit offset · R12 corners',mark=''.join(stack_parts),grid=stack_guides))
# 05: two equal circular rings meet a horizontal bridge; center cut is a single channel.
bridge_outer=circle(82,128,60)+circle(174,128,60)+rect(82,68,92,120)
bridge_cut=circle(82,128,28,fill='black')+circle(174,128,28,fill='black')+rect(82,100,92,56,fill='black')
# Split at one precise diagonal to make two cooperating parts, preserving silhouette.
bridge_cut+='<path d="M117 56H130L151 200H138Z" fill="black"/>'
bridge_mark=f'<defs><mask id="bridge-cut"><rect width="256" height="256" fill="white"/>{bridge_cut}</mask></defs><g fill="currentColor" mask="url(#bridge-cut)">{bridge_outer}</g>'
bridge_guides=circle(82,128,60)+circle(174,128,60)+circle(82,128,28)+circle(174,128,28)+line(12,68,244,68)+line(12,100,244,100)+line(12,156,244,156)+line(12,188,244,188)+line(82,20,82,236)+line(174,20,174,236)+line(117,56,138,200)+line(130,56,151,200)
concepts.append(dict(id='05-pair',name='Pair',color='#263B48',tag='Human intent meets agent execution.',formula='Equal R60 / R28 ends · one continuous channel',mark=bridge_mark,grid=bridge_guides))

def draw(c,x,y,size,construction=False):
    # Scope IDs because the same mark appears twice in a construction board.
    suffix=f'{x}-{y}-{size}'
    mark=c['mark']
    for mid in ['r-cut','owl-cut','bridge-cut','hub-cut']:
        mark=mark.replace(mid,mid+'-'+suffix)
    content=(f'<g opacity="0.12">{mark}</g>'+guides(c['grid'])) if construction else mark
    return f'<g transform="translate({x} {y}) scale({size/256})" color="{c["color"]}">{content}</g>'

manifest=[]
for c in concepts:
    stem=c['id']
    (OUT/f'{stem}.svg').write_text(svg(f'<g color="{c["color"]}">{c["mark"]}</g>',label='Repomon '+c['name']))
    art=rect(0,0,1440,900,fill=BG)
    art+=text(64,65,'REPOMON / GEOMETRIC EXPLORATION',16,weight=600)
    art+=text(1376,65,stem[:2],18,anchor='end')
    art+=draw(c,162,154,420)+draw(c,858,154,420,True)
    art+=text(372,638,'repomon',56,weight=600,anchor='middle')
    art+=text(1068,638,'CONSTRUCTION',14,fill='#68746D',weight=500,anchor='middle')
    art+=f'<path d="M64 714H1376" stroke="#D8DBD4"/>'
    art+=text(64,779,c['name'],34,weight=600)+text(64,823,c['tag'],21,fill='#58655D')
    art+=text(1376,804,c['formula'],18,fill='#58655D',anchor='end')
    (OUT/f'{stem}-study.svg').write_text(svg(art,1440,900,c['name']+' — mark and actual construction geometry'))
    subprocess.run(['rsvg-convert','-o',str(OUT/f'{stem}-study.png'),str(OUT/f'{stem}-study.svg')],check=True)
    manifest.append(dict(id=stem,title=c['name'],src=f'{stem}-study.png',output=f'{stem}-study.png'))

# One overview of all five original vector symbols and their actual construction.
overview=rect(0,0,1800,970,fill=BG)
overview+=text(62,72,'REPOMON',22,weight=650)+text(62,120,'Five marks. Built from geometry.',38,weight=500)
overview+=text(1738,72,'EXPLORATION / 02',15,anchor='end')
for i,c in enumerate(concepts):
    x=42+i*348
    overview+=draw(c,x+40,200,256)
    overview+=text(x+168,514,c['id'][:2]+' / '+c['name'],23,weight=500,anchor='middle')
    overview+=draw(c,x+68,594,200,True)
    overview+=text(x+168,847,'CIRCLES + LINES',12,fill='#68746D',anchor='middle')
overview+=text(62,933,'Editable SVG concepts · Construction guides follow the actual source shapes.',18,fill='#68746D')
(OUT/'overview.svg').write_text(svg(overview,1800,970,'Five geometric Repomon logo concepts'))
subprocess.run(['rsvg-convert','-o',str(OUT/'overview.png'),str(OUT/'overview.svg')],check=True)
(OUT/'manifest.json').write_text(json.dumps(manifest,indent=2))
print('Created five vector marks, five construction studies, and overview.')
