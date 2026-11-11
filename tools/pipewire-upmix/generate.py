#!/usr/bin/env python3
"""Generate a standalone PipeWire graph; existing source channels are direct copies."""
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUTPUT = 'FL FR FC LFE SL SR RL RR TFL TFR TRL TRR'.split()

def height_graph(layout):
    nodes, links = [], []
    def node(name, label, **props):
        nodes.append(dict(type='builtin', name=name, label=label, **props))
    def link(output, input):
        links.append(dict(output=output, input=input))
    for c in layout:
        node(c, 'copy')
    outputs = {}
    for c in layout:
        node('out_'+c, 'copy')
        link(f'{c}:Out', f'out_{c}:In')
        outputs[c] = f'out_{c}:Out'
    def ambience(name, left, right, gain, delay, bandpass=False):
        node(name+'_mix', 'mixer', control={'Gain 1': gain, 'Gain 2': -gain})
        link(f'{left}:Out', name+'_mix:In 1')
        link(f'{right}:Out', name+'_mix:In 2')
        signal = name+'_mix:Out'
        if bandpass:
            node(name+'_eq', 'param_eq', config={'filters': [
                dict(type='bq_highpass', freq=250, q=0.70710678),
                dict(type='bq_lowpass', freq=7000, q=0.70710678)]})
            link(signal, name+'_eq:In 1')
            signal = name+'_eq:Out 1'
        node(name+'_delay', 'delay', config={'max-delay': .1}, control={'Delay (s)': delay})
        link(signal, name+'_delay:In')
        outputs[name] = name+'_delay:Out'
    if 'RL' not in layout:
        ambience('RL', 'SL', 'SR', .18, .012)
        ambience('RR', 'SR', 'SL', .18, .017)
    rear = ('RL', 'RR') if 'RL' in layout else ('SL', 'SR')
    for name,left,right,delay in [
        ('TFL','FL','FR',.007), ('TFR','FR','FL',.011),
        ('TRL',*rear,.019), ('TRR',rear[1],rear[0],.023)]:
        ambience(name,left,right,.12,delay,True)
    return dict(nodes=nodes, links=links,
                inputs=[f'{c}:In' for c in layout], outputs=[outputs[c] for c in OUTPUT])

def module(mode, layout):
    return dict(name='libpipewire-module-filter-chain', args={
        'node.description': f'Realiser A16 {mode} to 7.1.4',
        'audio.rate':48000, 'node.latency':'960/48000',
        'filter.graph':height_graph(layout),
        'capture.props': {
            'node.name':f'a16.upmix.{mode}', 'node.description':f'Realiser A16 {mode} to 7.1.4',
            'media.class':'Audio/Sink', 'audio.position':layout, 'priority.session':0,
            'channelmix.upmix':False, 'channelmix.mix-lfe':False,
        },
        'playback.props': {
            'node.name':f'a16.upmix.{mode}.output', 'target.object':'a16.playback',
            'audio.position':OUTPUT, 'node.passive':True, 'node.dont-fallback':True,
            'stream.dont-remix':True, 'channelmix.upmix':False, 'channelmix.mix-lfe':False,
        }})

def config():
    # Same PSD conversion as the validated reference, now targeting the height stage.
    stereo={
        'node.description':'Realiser A16 Stereo to 7.1.4 (PSD)',
        'audio.position':['FL','FR'], 'audio.rate':48000, 'node.latency':'960/48000',
        'capture.props':{'node.name':'a16.upmix.stereo','node.description':'Realiser A16 Stereo to 7.1.4 (PSD)',
                         'media.class':'Audio/Sink','priority.session':0,'channelmix.upmix':False},
        'playback.props':{'node.name':'a16.upmix.output','target.object':'a16.upmix.7.1',
            'audio.position':'FL FR FC LFE SL SR RL RR'.split(), 'node.passive':True,
            'node.dont-fallback':True,'stream.dont-remix':True,'channelmix.upmix':True,
            'channelmix.upmix-method':'psd','channelmix.lfe-cutoff':0,'channelmix.mix-lfe':False,
            'channelmix.fc-cutoff':12000,'channelmix.rear-delay':12.,'channelmix.hilbert-taps':63,
            'channelmix.stereo-widen':0.,'channelmix.normalize':True}}
    return {
        'context.properties':{'log.level':1},
        'context.spa-libs':{'audio.convert.*':'audioconvert/libspa-audioconvert','support.*':'support/libspa-support'},
        'context.modules':[
            {'name':'libpipewire-module-rt','flags':['ifexists','nofail']},
            {'name':'libpipewire-module-protocol-native'},
            {'name':'libpipewire-module-client-node'}, {'name':'libpipewire-module-adapter'},
            module('7.1','FL FR FC LFE SL SR RL RR'.split()),
            module('5.1','FL FR FC LFE SL SR'.split()),
            {'name':'libpipewire-module-loopback','args':stereo}]}

if __name__ == '__main__':
    text=json.dumps(config(),indent=2)+'\n'
    path=HERE/'a16-upmix.conf'
    if '--check' in sys.argv:
        if path.read_text()!=text:raise SystemExit('Regenerate a16-upmix.conf')
    else:path.write_text(text)
