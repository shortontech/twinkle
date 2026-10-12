// Invoke with the copied app's Electron executable and an absolute main.cjs path.
const { app } = require('electron');
const fs = require('node:fs');
const output = process.env.TWINKLE_ELECTRON_PROBE_OUTPUT;
if (!output || !process.argv[2]) throw new Error('Set TWINKLE_ELECTRON_PROBE_OUTPUT and pass the Electron main.cjs path');
fs.mkdirSync(output, { recursive: false });
const timeout = setTimeout(() => { console.error('Electron probe timed out'); app.exit(1); }, 15000);
app.on('browser-window-created', (_, win) => {
  win.webContents.on('did-fail-load', (_, code, message) => { console.error(code, message); app.exit(1); });
  win.webContents.on('did-finish-load', async () => {
    try {
      await new Promise(resolve => setTimeout(resolve, 1200));
      const renderer = await win.webContents.executeJavaScript(`({cells: document.querySelectorAll('.cell').length, width: innerWidth, height: innerHeight, scale: devicePixelRatio, focused: document.hasFocus(), visible: document.visibilityState, diagnostics: document.querySelector('.diagnostics')?.innerText})`);
      const report = { versions: process.versions, renderer, gpuFeatureStatus: app.getGPUFeatureStatus(), gpuInfo: await app.getGPUInfo('complete') };
      fs.writeFileSync(`${output}/probe.json`, JSON.stringify(report, null, 2));
      fs.writeFileSync(`${output}/window.png`, (await win.webContents.capturePage()).toPNG());
      if (renderer.cells !== 200) throw new Error('Expected 200 board cells');
      console.log(JSON.stringify({renderer, gpuFeatureStatus: report.gpuFeatureStatus}));
      clearTimeout(timeout); app.exit(0);
    } catch (error) { console.error(error); app.exit(1); }
  });
});
require(process.argv[2]);
