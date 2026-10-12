#!/usr/bin/env python3
"""Freeze explicit application poll cadence; native GPU/caret scheduling is separate."""
import json
from pathlib import Path

steps=[]
def add(at,kind,label,**values):
    steps.append(dict(atMs=at,kind=kind,label=label,**values))
add(0,'observe','initial-idle')
add(100,'button','held-left-immediate',button='left',pressed=True)
for index, at in enumerate(range(150,701,50)):
    add(at,'poll','held-left-pre-repeat' if index<3 else 'held-left-wall-repeat')
add(750,'button','release-left',button='left',pressed=False)
add(1450,'poll','gravity')
add(2150,'poll','gravity')
add(2200,'button','rotate',button='rotate',pressed=True)
add(2210,'button','release-rotate',button='rotate',pressed=False)
add(2250,'button','rotate-again',button='rotate',pressed=True)
add(2260,'button','release-rotate',button='rotate',pressed=False)
add(2300,'button','soft-drop-immediate',button='down',pressed=True)
for at in range(2358,2707,58): add(at,'poll','held-soft-drop')
add(2720,'button','release-soft-drop',button='down',pressed=False)
add(2800,'menu','open-menu')
add(2850,'poll','paused-menu-no-op')
add(3000,'click','resume-menu',target='continue')
add(3100,'focus','blur',value=False)
add(3150,'poll','blur-no-op')
add(3200,'visibility','hidden',value=False)
add(3250,'poll','hidden-no-op')
add(3300,'visibility','visible',value=True)
add(3400,'focus','focus-return',value=True)
add(4100,'poll','gravity-after-focus')
add(4200,'button','hard-drop',button='drop',pressed=True)
add(4210,'button','release-drop',button='drop',pressed=False)
add(4400,'menu','menu-before-restart')
add(4600,'click','restart',target='restart')
add(5300,'poll','gravity-after-restart')
add(5500,'menu','idle-menu')
add(9000,'observe','return-to-idle')
root=Path(__file__).resolve().parents[1]/'benchmarks/0012/traces'
root.mkdir(parents=True,exist_ok=True)
def write(name,sequence,duration):
    data={'schema':1,'name':name,'clock':'explicit frozen application polls; natural native/GPU/browser presentation',
          'viewport':[800,720],'scale':1,'warmupMs':3000,'durationMs':duration,
          'collectionPolicy':'natural; no forced GC','steps':sequence}
    (root/(name+'.json')).write_text(json.dumps(data,indent=2)+'\n')
write('blocks-behavior',steps,10000)
cycles=list(steps[:-2])
for cycle in range(100):
    base=6000+cycle*120
    for offset,kind,label,extra in [
        (0,'button','cycle-hard-drop',{'button':'drop','pressed':True}),
        (2,'button','cycle-drop-release',{'button':'drop','pressed':False}),
        (20,'menu','cycle-menu',{}),(40,'click','cycle-restart',{'target':'restart'})]:
        cycles.append(dict(atMs=base+offset,kind=kind,label=label,cycle=cycle,**extra))
cycles.extend([dict(atMs=18500,kind='menu',label='post-cycle-idle-menu'),dict(atMs=22000,kind='observe',label='post-cycle-return-to-idle')])
write('blocks-lifecycle',cycles,23000)
print(f'Frozen {len(steps)} behavior and {len(cycles)} lifecycle steps (100 cycles)')
