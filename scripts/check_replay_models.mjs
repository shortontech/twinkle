import fs from 'node:fs';
import {Game} from '../benchmarks/0012/react/src/model.js';
import {ReplayController} from '../benchmarks/0012/react/src/replay.js';
const [tracePath,nativePath,outputPath]=process.argv.slice(2);
const trace=JSON.parse(fs.readFileSync(tracePath));
let game=new Game(),menuOpen=false;
const api={get:()=>({game,menuOpen}),set:(g,m)=>{game=g;menuOpen=m;},focus:()=>{},visibility:()=>{},
  menu:()=>{if(!game.over)menuOpen=!menuOpen;},
  click:target=>{if(target==='restart'){game=new Game();menuOpen=false;}else if(target==='continue')menuOpen=false;else throw Error(target);}};
const controller=new ReplayController(api);
const states=trace.steps.map(step=>{controller.step(step);return controller.state();});
const native=JSON.parse(fs.readFileSync(nativePath));
const canonical=value=>Array.isArray(value)?value.map(canonical):value!==null&&typeof value==='object'?Object.fromEntries(Object.keys(value).sort().map(key=>[key,canonical(value[key])])):value;
const mismatches=[];
for(let i=0;i<states.length;i++)if(JSON.stringify(canonical(states[i]))!==JSON.stringify(canonical(native.steps[i].state)))mismatches.push({index:i,step:trace.steps[i],native:native.steps[i].state,reactModel:states[i]});
const result={status:mismatches.length?'failed':'passed',steps:states.length,mismatches,
  scope:'native canonical host trace versus exported frozen React game model/controller; live React DOM is a separate required check'};
if(outputPath)fs.writeFileSync(outputPath,JSON.stringify(result,null,2));
console.log(JSON.stringify({status:result.status,steps:result.steps,mismatches:mismatches.length}));
if(mismatches.length)process.exitCode=1;
