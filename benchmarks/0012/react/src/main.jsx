import React from 'react';
import {createRoot} from 'react-dom/client';
import {flushSync} from 'react-dom';
import App from './twinkle-blocks.jsx';
import Fixture from './fixtures.jsx';
import {ReplayController} from './replay.js';
import './style.css';

const params=new URLSearchParams(location.search);
const controlled=params.has('trace');
const fixtureName=params.get('fixture');
if(fixtureName)document.title=fixtureName.startsWith('cells-')?`React scaling fixture: ${fixtureName.slice(6)} cells (static)`:`React ${fixtureName} fixture (static)`;
// Production comparison intentionally follows the original StrictMode entrypoint.
createRoot(document.getElementById('root')).render(<React.StrictMode>{fixtureName?<Fixture name={fixtureName}/>:<App controlled={controlled}/>}</React.StrictMode>);
window.__benchReady=false;
window.__benchReport=null;
window.__benchFailure=null;

const delay=ms=>new Promise(resolve=>setTimeout(resolve,Math.max(0,ms)));
function verifyDOM(state) {
  const cells=[...document.querySelectorAll('.cell')];
  if(state.presentation.menuOpen||state.presentation.over) {
    if(!document.querySelector('.menu-screen'))throw Error('Menu did not reach the DOM');
  } else {
    if(cells.length!==200)throw Error(`Expected 200 cells, found ${cells.length}`);
    const colors=cells.map(cell=>[...cell.classList].find(name=>/^c\d$/.test(name))??'');
    if(JSON.stringify(colors)!==JSON.stringify(state.presentation.board))throw Error('DOM board differs from model');
    if(!document.querySelector('.topbar').innerText.includes(`Score ${state.model.score}`))throw Error('DOM score differs');
  }
}
async function replay() {
  const trace=await (await fetch(params.get('trace'))).json();
  const states=params.has('states');
  const ownership=params.has('ownership');
  while(!window.__blocksAPI) await delay(10);
  const api=window.__blocksAPI;
  const controller=new ReplayController({...api,
    menu:()=>window.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true})),
    click:target=>{const button=document.getElementById(target);if(!button)throw Error(`Missing ${target} button`);button.click();},
  });
  window.__benchReady=true;
  const initialHostNodeCount=document.querySelectorAll('*').length;
  const initialCellCount=document.querySelectorAll('.cell').length;
  await fetch('/__bench-ready',{method:'POST',body:JSON.stringify({viewport:[innerWidth,innerHeight],scale:devicePixelRatio,focused:document.hasFocus(),visible:document.visibilityState,initialHostNodeCount,initialCellCount})});
  const alignment=await (await fetch('/__bench-start')).json();
  const origin=performance.now()+alignment.originEpochMs-Date.now();
  const records=[];
  for(let index=0;index<trace.steps.length;index++) {
    const step=trace.steps[index];
    await delay(origin+step.atMs-performance.now());
    const observerStart=performance.now();
    const before=states?controller.state():null;
    const observerBeforeUs=(performance.now()-observerStart)*1000;
    const actualMs=performance.now()-origin;
    const beforePublications=controller.publications;
    const start=performance.now();
    let published=false;
    flushSync(()=>{published=controller.step(step);});
    const serviceUs=(performance.now()-start)*1000;
    const observerAfterStart=performance.now();
    const state=states?controller.state():null;
    if(states)verifyDOM(state);
    const observerUs=observerBeforeUs+(performance.now()-observerAfterStart)*1000;
    const record={index,atMs:step.atMs,label:step.label,kind:step.kind,actualMs,serviceUs,observerUs,
      poll:step.kind==='poll',attemptedPublication:published,publicationAttempts:controller.publications-beforePublications,
      observableChanged:states?JSON.stringify(before.presentation)!==JSON.stringify(state.presentation):null,state,
      paintOpportunityMs:null,presentationTimestamp:null};
    // A callback before a paint opportunity is explicitly not compositor display.
    if(published) record.paintOpportunityMs=await new Promise(resolve=>requestAnimationFrame(()=>resolve(performance.now()-start)));
    if(ownership){
      await fetch('/__bench-step',{method:'POST',body:JSON.stringify(record)});
      if(step.kind==='observe'||step.label==='cycle-restart'){
        const observerStart=performance.now();
        const row={index,label:step.label,cycle:step.cycle??null,hostNodeCount:document.querySelectorAll('*').length,cellCount:document.querySelectorAll('.cell').length,
          pageHeap:performance.memory?{used:performance.memory.usedJSHeapSize,committed:performance.memory.totalJSHeapSize,limit:performance.memory.jsHeapSizeLimit}:null,
          harnessStoredSteps:records.length,collectionPolicy:'natural; no forced GC; precise heap info and CDP counters are attribution only'};
        row.pageObserverUs=(performance.now()-observerStart)*1000;
        const response=await fetch('/__bench-ownership',{method:'POST',body:JSON.stringify(row)});
        if(!response.ok)throw Error('Ownership checkpoint failed');
      }
    }else records.push(record);
  }
  await delay(origin+trace.durationMs-performance.now());
  const finalState=controller.state();verifyDOM(finalState);
  window.__benchReport={schema:1,status:'complete',implementation:'react-chrome',trace,steps:records,streamedSteps:ownership,totalTraceSteps:trace.steps.length,finalState,
    alignment,initialHostNodeCount,initialCellCount,hostNodeCount:document.querySelectorAll('*').length,cellCount:document.querySelectorAll('.cell').length,
    viewport:[innerWidth,innerHeight],scale:devicePixelRatio,actualFocused:document.hasFocus(),actualVisible:document.visibilityState,
    collectionPolicy:'natural; no forced GC',
    timingBoundary:'serviceUs includes normalized model input and React flushSync through DOM commit; excludes observer, asynchronous style/layout/raster/GPU/compositor work. Whole-process CPU separately accounts browser pipeline work.',
    presentationTimestamp:null};
  await fetch('/__bench-report',{method:'POST',body:JSON.stringify(window.__benchReport)});
}
async function staticReplay(){
  const trace=await (await fetch(params.get('trace'))).json();
  while(!window.__fixtureReady)await delay(10);
  const initialHostNodeCount=document.querySelectorAll('*').length;
  const initialCellCount=document.querySelectorAll('.cell').length;
  await fetch('/__bench-ready',{method:'POST',body:JSON.stringify({viewport:[innerWidth,innerHeight],scale:devicePixelRatio,focused:document.hasFocus(),visible:document.visibilityState,initialHostNodeCount,initialCellCount})});
  const alignment=await (await fetch('/__bench-start')).json();
  await delay(alignment.originEpochMs+trace.durationMs-Date.now());
  window.__benchReport={status:'complete',fixture:fixtureName,steps:[],initialHostNodeCount,initialCellCount,hostNodeCount:document.querySelectorAll('*').length,cellCount:document.querySelectorAll('.cell').length,viewport:[innerWidth,innerHeight],scale:devicePixelRatio,alignment,fields:[...document.querySelectorAll('input,textarea,select,button')].map(n=>({id:n.id,value:n.value,label:n.getAttribute('aria-label')})),collectionPolicy:'natural; no forced GC',scope:'Static scene; no controller replay or per-update timings'};
  await fetch('/__bench-report',{method:'POST',body:JSON.stringify(window.__benchReport)});
}
if(controlled) (fixtureName?staticReplay():replay()).catch(error=>{window.__benchFailure=String(error.stack??error);console.error(error);fetch('/__bench-failure',{method:'POST',body:window.__benchFailure});});
else setTimeout(()=>{window.__benchReady=true;},100);
