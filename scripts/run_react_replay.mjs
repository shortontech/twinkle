#!/usr/bin/env node
// Owns a fresh Chrome app profile and a localhost-only production asset server.
// The measured process root is Chrome, not this server/CDP controller process.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import http from 'node:http';
import crypto from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';

const args=process.argv.slice(2),get=key=>args[args.indexOf(key)+1];
if(!args.includes('--trace')||!args.includes('--output'))throw Error('Usage: run_react_replay.mjs --trace FILE --output NEW_DIRECTORY [--states] [--keep-open] [--wayland-socket ABSOLUTE_SOCKET] [--window-size WIDTH,HEIGHT]');
const tracePath=path.resolve(get('--trace')),output=path.resolve(get('--output'));
fs.mkdirSync(output,{recursive:false});
const root=path.resolve(path.dirname(new URL(import.meta.url).pathname),'../benchmarks');
const trace=JSON.parse(fs.readFileSync(tracePath));
const profile=fs.mkdtempSync(path.join(os.tmpdir(),'twinkle-0012-chrome-'));
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms));
let browser=null,ws=null,session=null,report=null,failure=null,ready=null,sampler=null,samplerDone=null;
const servedAssets={};
let nextId=1;const pending=new Map();
function cdp(method,params={},sessionId=null) {
  return new Promise((resolve,reject)=>{
    const id=nextId++,timer=setTimeout(()=>{pending.delete(id);reject(Error(`CDP ${method} timed out`));},10000);
    pending.set(id,{resolve,reject,timer});
    ws.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));
  });
}
const server=http.createServer(async(req,res)=>{
  try {
    const url=new URL(req.url,'http://localhost');
    if(req.method==='GET'&&url.pathname==='/__bench-start'){
      if(!args.includes('--sample')){res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({originEpochMs:Date.now()+(trace.warmupMs??3000)}));return;}
      const readyPath=path.join(output,'process/sampling-ready.json'),until=Date.now()+30000;
      while(!fs.existsSync(readyPath)){if(Date.now()>until||sampler?.exitCode!==null&&sampler?.exitCode!==undefined)throw Error('Sampler startup failed');await delay(20);}
      const clock=JSON.parse(fs.readFileSync(readyPath));
      const originEpochMs=clock.samplingStartEpochMs+clock.warmupMs;
      fs.writeFileSync(path.join(output,'alignment.json'),JSON.stringify({...clock,originEpochMs,responseEpochMs:Date.now()},null,2));
      res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({originEpochMs}));return;
    }
    if(req.method==='POST'&&url.pathname.startsWith('/__bench-')) {
      let body='';for await(const chunk of req){body+=chunk;if(body.length>32*1024*1024)throw Error('report too large');}
      if(url.pathname==='/__bench-ready'){ready=JSON.parse(body);fs.writeFileSync(path.join(output,'page-ready.json'),JSON.stringify(ready,null,2));}
      else if(url.pathname==='/__bench-report'){report=JSON.parse(body);fs.writeFileSync(path.join(output,'replay.json'),JSON.stringify(report,null,2));}
      else if(url.pathname==='/__bench-step'){fs.appendFileSync(path.join(output,'steps.jsonl'),JSON.stringify(JSON.parse(body))+'\n');}
      else if(url.pathname==='/__bench-ownership'){
        if(!session)throw Error('CDP ownership session not ready');
        const observerStart=performance.now();
        const row=JSON.parse(body);row.browserMetrics=await cdp('Performance.getMetrics',{},session);row.dom=await cdp('Memory.getDOMCounters',{},session);row.serverObservationWallMs=performance.now()-observerStart;
        fs.appendFileSync(path.join(output,'ownership.jsonl'),JSON.stringify(row)+'\n');
      }
      else if(url.pathname==='/__bench-failure'){failure=body;fs.writeFileSync(path.join(output,'failure.txt'),body);}
      res.writeHead(200);res.end('ok');return;
    }
    const file=path.resolve(root,'.'+decodeURIComponent(url.pathname));
    if(!file.startsWith(root+path.sep))throw Error('invalid asset path');
    const actual=fs.statSync(file).isDirectory()?path.join(file,'index.html'):file;
    const type={'.html':'text/html','.js':'text/javascript','.css':'text/css','.json':'application/json','.png':'image/png'}[path.extname(actual)]??'application/octet-stream';
    const bytes=fs.readFileSync(actual);
    servedAssets[path.relative(root,actual)]=sha(bytes);
    res.writeHead(200,{'Content-Type':type,'Cache-Control':'no-store'});res.end(bytes);
  }catch(error){res.writeHead(404);res.end('not found');}
});
try {
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const params=new URLSearchParams({trace:'/0012/traces/'+path.basename(tracePath)});
  if(args.includes('--states'))params.set('states','1');
  if(args.includes('--ownership'))params.set('ownership','1');
  if(args.includes('--fixture'))params.set('fixture',get('--fixture'));
  const url=`http://127.0.0.1:${server.address().port}/0012/react/dist/?${params}`;
  const browserEnv={...process.env};
  let platform='x11';
  if(args.includes('--wayland-socket')) {
    const socket=get('--wayland-socket');
    if(!socket||!path.isAbsolute(socket)||!fs.statSync(socket).isSocket())throw Error('--wayland-socket must name an existing absolute compositor socket');
    browserEnv.XDG_RUNTIME_DIR=path.dirname(socket);
    browserEnv.WAYLAND_DISPLAY=path.basename(socket);
    delete browserEnv.DISPLAY;delete browserEnv.WAYLAND_SOCKET;
    platform='wayland';
  }
  const windowSize=args.includes('--window-size')?get('--window-size'):'800,720';
  if(!/^\d{3,4},\d{3,4}$/.test(windowSize))throw Error('--window-size must be WIDTH,HEIGHT');
  const command=[...(args.includes('--ownership')?['--enable-precise-memory-info']:[]),'--user-data-dir='+profile,'--remote-debugging-port=0','--ozone-platform='+platform,'--force-device-scale-factor=1',
    '--window-size='+windowSize,'--no-first-run','--no-default-browser-check','--disable-extensions','--app='+url];
  browser=spawn('/usr/bin/google-chrome',command,{env:browserEnv,detached:true,stdio:['ignore',fs.openSync(path.join(output,'stdout.log'),'w'),fs.openSync(path.join(output,'stderr.log'),'w')]});
  const deadline=Date.now()+30000;
  const portFile=path.join(profile,'DevToolsActivePort');
  while(!fs.existsSync(portFile)){if(browser.exitCode!==null)throw Error(`Chrome exited ${browser.exitCode}`);if(Date.now()>deadline)throw Error('Chrome startup timed out');await delay(100);}
  const [port,endpoint]=fs.readFileSync(portFile,'utf8').trim().split('\n');
  ws=new WebSocket(`ws://127.0.0.1:${port}${endpoint}`);
  ws.addEventListener('message',event=>{
    const message=JSON.parse(event.data);const callback=pending.get(message.id);
    if(callback){pending.delete(message.id);clearTimeout(callback.timer);message.error?callback.reject(Error(JSON.stringify(message.error))):callback.resolve(message.result);}
  });
  await new Promise((resolve,reject)=>{ws.addEventListener('open',resolve,{once:true});ws.addEventListener('error',reject,{once:true});});
  const version=await cdp('Browser.getVersion'),gpu=await cdp('SystemInfo.getInfo');
  const targets=await cdp('Target.getTargets');
  const target=targets.targetInfos.find(t=>t.type==='page'&&t.url.startsWith(url.split('?')[0]));
  if(!target)throw Error('App page target missing');
  ({sessionId:session}=await cdp('Target.attachToTarget',{targetId:target.targetId,flatten:true}));
  await cdp('Page.enable',{},session);await cdp('Performance.enable',{},session);
  await cdp('Page.bringToFront',{},session);
  const sourceFiles={};
  for(const relative of ['0012/react/package.json','0012/react/package-lock.json','0012/react/dist/index.html',...fs.readdirSync(path.join(root,'0012/react/dist/assets')).map(n=>'0012/react/dist/assets/'+n),'0012/traces/'+path.basename(tracePath)])
    sourceFiles[relative]=sha(fs.readFileSync(path.join(root,relative)));
  const sourceRevision=execFileSync('git',['-C',root,'rev-parse','HEAD'],{encoding:'utf8'}).trim();
  const sourceStatus=execFileSync('git',['-C',root,'status','--porcelain'],{encoding:'utf8'}).trim();
  const sourceSha256={};
  for(const name of fs.readdirSync(path.join(root,'0012/react/src')))sourceSha256['src/'+name]=sha(fs.readFileSync(path.join(root,'0012/react/src',name)));
  const executable=fs.realpathSync(`/proc/${browser.pid}/exe`);
  const manifest={schema:1,sourceRevision,sourceStatus,sourceSha256,executable,executableSha256:sha(fs.readFileSync(executable)),
    OS:{platform:process.platform,release:os.release(),arch:process.arch},controllerVersion:process.version,command:['/usr/bin/google-chrome',...command],rootPid:browser.pid,controllerPid:process.pid,
    controllerAndServerIncluded:false,compositorIncluded:false,displayContext:{platform,DISPLAY:browserEnv.DISPLAY??null,WAYLAND_DISPLAY:browserEnv.WAYLAND_DISPLAY??null,XDG_RUNTIME_DIR:browserEnv.XDG_RUNTIME_DIR??null},version,gpu,traceSha256:sha(fs.readFileSync(tracePath)),servedAssetSha256:sourceFiles,
    presentationSynchronization:{policy:'Chrome default compositor synchronization; no disable-gpu-vsync or disable-frame-rate-limit launch switches',physicalDisplayTimestamp:null,swapchainPresentMode:null,boundary:'Actual launch switches recorded; Chrome swapchain mode and physical compositor completion are not exposed by these CDP reports. RAF is a scheduling opportunity, not presentation.'},
    collectionPolicy:'natural; no forced GC',CDP:args.includes('--ownership')?'attribution only: precise heap info; 102 cycle/idle CDP heap and DOM counter checkpoints; no forced GC':'fresh dedicated Chrome app profile; initial GPU/version, final aggregate metrics and screenshot; no per-step CDP polling',
    limitations:['compositor display timestamps unavailable','React flushSync service excludes asynchronous layout/raster; process-tree CPU separately includes pipeline work']};
  fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest,null,2));
  fs.writeFileSync(path.join(output,'ready.json'),JSON.stringify({ready:true,rootPid:browser.pid},null,2));
  if(args.includes('--sample')){
    sampler=spawn('python3',[path.resolve(root,'../scripts/live_pipeline_sample.py'),'--output',path.join(output,'process'),'--label','react-controlled-replay','--attach',String(browser.pid),'--source',path.join(root,'0012/react'),'--warmup',String(Math.ceil((trace.warmupMs??3000)/1000)),'--seconds',String(Math.ceil(trace.durationMs/1000)+1)],{stdio:['ignore',fs.openSync(path.join(output,'sampler.log'),'w'),fs.openSync(path.join(output,'sampler-errors.log'),'w')]});
    samplerDone=new Promise(resolve=>sampler.once('exit',(code,signal)=>resolve({code,signal})));
  }
  while(!report&&!failure){if(Date.now()>deadline+trace.durationMs+trace.warmupMs+20000)throw Error('Replay timed out');if(browser.exitCode!==null)throw Error(`Chrome exited ${browser.exitCode}`);await delay(200);}
  if(failure)throw Error(failure);
  if(ready?.viewport?.[0]!==800||ready?.viewport?.[1]!==720||ready?.scale!==1)throw Error('Actual page viewport must be800x720 atscale1; adjust window size for platform decorations');
  manifest.actuallyServedSha256=servedAssets;
  manifest.actualPage=ready;
  manifest.assetsUnchanged=Object.entries(sourceFiles).every(([file,digest])=>sha(fs.readFileSync(path.join(root,file)))===digest);
  manifest.buildMode='Vite production assets; React 18.3.1';
  fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest,null,2));
  if(!manifest.assetsUnchanged)throw Error('Production assets changed during replay');
  if(samplerDone){
    const result=await samplerDone;
    if(result.code!==0)throw Error('Process sampler failed: '+JSON.stringify(result));
    manifest.processSamples='process/';
    manifest.samplingAlignment='Replay origin is sampler initial snapshot epoch plus its warmup; see alignment.json and per-step actualMs. Clock conversion has millisecond wall-clock precision; presenter/display timing remains unavailable.';
    fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest,null,2));
  }
  const metrics=await cdp('Performance.getMetrics',{},session);
  manifest.gpuAtCompletion=await cdp('SystemInfo.getInfo');
  manifest.gpuFeatureStatusChanged=JSON.stringify(gpu.gpu.featureStatus)!==JSON.stringify(manifest.gpuAtCompletion.gpu.featureStatus);
  fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify(manifest,null,2));
  const dom=await cdp('Memory.getDOMCounters',{},session);
  fs.writeFileSync(path.join(output,'browser-metrics.json'),JSON.stringify({metrics,dom},null,2));
  const screenshot=await cdp('Page.captureScreenshot',{},session);
  fs.writeFileSync(path.join(output,'window.png'),Buffer.from(screenshot.data,'base64'));
  console.log(JSON.stringify({status:report.status,steps:report.steps.length,rootPid:browser.pid,viewport:report.viewport,scale:report.scale,initialCellCount:report.initialCellCount}));
  if(args.includes('--keep-open'))await new Promise(resolve=>browser.on('exit',resolve));
}catch(error){fs.writeFileSync(path.join(output,'runner-failure.txt'),String(error.stack??error));console.error(error);process.exitCode=1;}
finally{
  const shutdown={rootPid:browser?.pid??null,browserCloseRequested:false,signalFallback:null,boundary:'After report, final GPU checks, screenshot and complete process sampling. Shutdown status does not describe presentation or sampled GPU feature status.'};
  if(sampler&&sampler.exitCode===null&&sampler.signalCode===null)sampler.kill('SIGTERM');
  if(browser&&browser.exitCode===null){
    // Let Chrome shut down its own renderer/GPU children before falling back
    // to signals for a failed/unresponsive benchmark instance.
    if(ws?.readyState===WebSocket.OPEN){shutdown.browserCloseRequested=true;try{await cdp('Browser.close');}catch{}}
    const exitDeadline=Date.now()+5000;
    while(browser.exitCode===null&&browser.signalCode===null&&Date.now()<exitDeadline)await delay(50);
    if(browser.exitCode===null&&browser.signalCode===null){
      shutdown.signalFallback='SIGTERM';try{process.kill(-browser.pid,'SIGTERM');}catch{}
      await delay(1000);
      if(browser.exitCode===null&&browser.signalCode===null){shutdown.signalFallback='SIGKILL';try{process.kill(-browser.pid,'SIGKILL');}catch{}}
    }
  }
  ws?.close();
  server.close();
  for(const callback of pending.values()){clearTimeout(callback.timer);callback.reject(Error('runner ended'));}
  fs.rmSync(profile,{recursive:true,force:true,maxRetries:10,retryDelay:100});
  fs.writeFileSync(path.join(output,'shutdown.json'),JSON.stringify({...shutdown,exitCode:browser?.exitCode??null,signalCode:browser?.signalCode??null,profileRemoved:!fs.existsSync(profile)},null,2));
}
