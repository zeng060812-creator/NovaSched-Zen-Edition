const fs = require('fs');
const p = 'websocket.rs';
let s = fs.readFileSync(p, 'utf8');
let n = 0;
function rep(a, b) {
  if (!s.includes(a)) throw new Error('missing: ' + a.slice(0, 70));
  s = s.split(a).join(b);
  n++;
}
rep(
  '    fas_engine_fps: f32,\n    fas_engine_gpu: f32,',
  '    fas_engine_fps: f32,\n    fas_engine_target: u32,\n    fas_engine_gpu: f32,'
);
rep(
  '            data.fas_engine_fps = status.fps;\n            data.fas_engine_gpu = status.gpu;',
  '            data.fas_engine_fps = status.fps;\n            data.fas_engine_target = status.target;\n            data.fas_engine_gpu = status.gpu;'
);
fs.writeFileSync(p, s);
console.log('patched', n);
