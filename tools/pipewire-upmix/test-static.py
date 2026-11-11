#!/usr/bin/env python3
"""Hardware/server-free checks for static PSD processing and final partial blocks."""
import array,json,math,os,subprocess,sys,tempfile
from pathlib import Path
binary=Path(sys.argv[1]);cfg=json.loads(Path(sys.argv[2]).read_text());modules=cfg['context.modules']
psd=next(m['args']['playback.props'] for m in modules if m['name']=='libpipewire-module-loopback')
graph=next(m['args']['filter.graph'] for m in modules if m.get('args',{}).get('capture.props',{}).get('node.name')=='a16.upmix.7.1')
with tempfile.TemporaryDirectory() as td:
 p=Path(td);(p/'psd.json').write_text(json.dumps(psd));(p/'graph.json').write_text(json.dumps({'filter.graph':graph,'filter-graph.n_inputs':8,'filter-graph.n_outputs':12}))
 args=[str(binary),str(p/'psd.json'),str(p/'graph.json')]
 def render(data):
  r=subprocess.run(args,input=data,stdout=subprocess.PIPE,stderr=subprocess.PIPE,env={**os.environ,'PIPEWIRE_REMOTE':'offline-no-server'},check=True)
  a=array.array('f');a.frombytes(r.stdout);return a
 frames=48017
 def signal(sign):return array.array('f',(v for i in range(frames) for v in (.01*math.sin(2*math.pi*1000*i/48000),sign*.01*math.sin(2*math.pi*1000*i/48000)))).tobytes()
 mono=render(signal(1));assert len(mono)==frames*16
 assert render(signal(1)).tobytes()==mono.tobytes(),'Nondeterministic output'
 opposite=render(signal(-1));assert len(opposite)==frames*16
 for data in [mono,opposite]:
  assert all(math.isfinite(v) for v in data)
  for c in [3,4,5,10,15]:assert all(v==0 for v in data[c::16]),('Unexpected channel',c)
  assert max(abs(v) for v in data[-17*16:])>.001,'Final partial block lost'
 def rms(a,c):return math.sqrt(sum(float(a[i*16+c])**2 for i in range(24000,47000))/23000)
 assert abs(rms(mono,2)/(.01/math.sqrt(2))-1)<.005,'Mono center extraction'
 assert rms(opposite,2)<1e-9,'Antiphase should cancel in center'
 for c in [8,9,11,12,13,14]:assert rms(opposite,c)>1e-5,('Missing derived channel',c)
 assert max(abs(v) for v in render(bytes(17*2*4)))==0
 bad=subprocess.run(args,input=b'1234567',stdout=subprocess.PIPE,stderr=subprocess.PIPE);assert bad.returncode!=0
print('Static DSP: frame count, final block, routing, center cancellation, derived channels, silence, determinism and truncated input passed.')
