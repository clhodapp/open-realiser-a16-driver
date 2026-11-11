#!/usr/bin/env python3
"""Generate USB profile catalog. Invalid workbook rows are audited, never silently trusted."""
import json,re,sys
from pathlib import Path
HERE=Path(__file__).resolve().parent
def publish(path,text):
 if '--check' in sys.argv:
  if not path.exists() or path.read_text()!=text:raise SystemExit(str(path)+' is stale; run generate.py')
 else:path.write_text(text)

# Semantic direction names; angles of the loaded capture remain authoritative.
CODES=dict(L='FL',R='FR',C='FC',SW='LFE',Ls='SL',Rs='SR',Lss='SL',Rss='SR',Lb='RL',Rb='RR',
 Cr='RC',SW2='LFE2',Lw='FLW',Rw='FRW',Lc='FLC',Rc='FRC',
 Lh='TFL',Rh='TFR',Ch='TFC',T='TC',Chr='TRC',Lhr='TRL',Rhr='TRR',Lhs1='TRL',Rhs1='TRR',Ltf='TFL',Rtf='TFR',Ltm='TSL',Rtm='TSR',
 Ltr='TRL',Rtr='TRR')
# Pulse codes have no explicit height-vs-top or extra-surround representation.
PULSE=set('FL FR FC LFE SL SR RL RR RC FLC FRC TC TFL TFR TFC TRL TRR TRC'.split())
rows=json.loads((HERE/'smyth-tables.json').read_text())['rows'];audit=[];catalog=[];seen={};aliases=[]
native_rooms=[]
for r in rows:
 if r['family']=='PCM':
  slots=r['slots'].copy()
  if r['mode']=='9.1.2m' and 'SW2' in slots:slots[slots.index('SW2')]=''
  native_rooms.append((r['mode'],{i:x for i,x in enumerate(slots) if x}))
native_rooms.sort(key=lambda r:len(r[1]))


def add(name,layout,slots,families,source,force_advanced=False):
 active=[(i,s) for i,s in enumerate(slots) if s]
 if len(active)>16:return None
 # A PCM Custom room can relocate a <=16-speaker source layout's higher-numbered slots.
 usb={i:s for i,s in active if i<16}
 for i,label in active:
  if i>=16:usb[next(j for j in range(16) if j not in usb)]=label
 items=sorted(usb.items());native=[];used=set();aux=0
 for slot,label in items:
  code=CODES.get(label)
  # When distinct top and height rows coexist, preserve separate native front
  # height identities; a single elevated row uses the conventional top-* names.
  if label=='Lh' and any(s=='Ltf' for i,s in items):code='FLH'
  if label=='Rh' and any(s=='Rtf' for i,s in items):code='FRH'
  if code is None or code in used:
   while 'AUX'+str(aux) in used:aux+=1
   code='AUX'+str(aux);aux+=1
  used.add(code);native.append(code)
 pulse_compatible=not any(c not in PULSE for c in native)
 matching_room=next((name for name,mapping in native_rooms if all(mapping.get(i)==label for i,label in items)),None)
 custom=matching_room is None
 advanced=force_advanced or custom or not pulse_compatible
 if custom and families!=['PCM Custom Bank']:
  audit.append(dict(source=source,mode=layout,action='not advertised',reason='Requires a PCM Custom bank; choose/configure one of the four bank profiles'))
  return None
 if custom and not pulse_compatible:
  audit.append(dict(source=source,mode=layout,action='excluded',reason='Custom profile has positions Pulse cannot represent'))
  return None
 key=tuple(items)
 if key in seen:
  existing=seen[key];existing['families']=sorted(set(existing['families']+families))
  existing['sources'].append(source)
  if name!=existing['name']:aliases.append(dict(name=name,target=existing['name']))
  return existing
 p=dict(name=name,layout=layout,advanced=advanced,custom=custom,pulse_compatible=pulse_compatible,matching_room=matching_room,available=True,channels=len(items),usb=[i for i,s in items],
        speakers=[s for i,s in items],positions=native,families=families,sources=[source])
 p['description']=('Custom: ' if custom else ('Advanced: ' if advanced else 'Compatibility: '))+layout+' + Stereo Return'
 p['room']=('PCM Custom room' if custom else 'PCM '+matching_room+' room (unused positions silent)')+' with USB '+', '.join(str(i+1)+'='+label for i,label in items)+'; other slots unused'
 catalog.append(p);seen[key]=p;return p

# Keep existing public IDs and index ordering stable; safe superset of current room.
base='L R C SW Lss Rss Lb Rb Lw Rw Ltf Rtf Ltm Rtm Ltr Rtr'.split()
compat=base.copy()
for i in [8,9,12,13]:compat[i]=''
add('7.1.4','7.1.4t Compatibility',compat,['PCM'],'validated live room and bridge')
add('9.1.6','9.1.6t',base,['PCM'],'validated live room and bridge')
auro=next(r for r in rows if r['family']=='Auro-3D' and r['mode']=='13.1')
# Auro 13.1 is omitted by policy: its side heights lack Pulse positions.
for row in sorted(rows,key=lambda r:['PCM','Auro-3D','DTS','Atmos'].index(r['family'])):
 family,mode,slots=row['family'],row['mode'],row['slots'].copy()
 source=f"workbook {family} code {row['code']} row {row['row']}"
 # Concrete workbook defect: PCM 9.1.2m includes an extra SW2, identical to 9.2.2m.
 if family=='PCM' and mode=='9.1.2m' and 'SW2' in slots:
  slots[slots.index('SW2')]='';audit.append(dict(source=source,action='corrected',reason='Removed extraneous SW2 from 9.1.2m; 9.2.2m retains it'))
 match=re.fullmatch(r'(\d+)\.(\d+)(?:\.(\d+))?([a-z]*)',mode)
 if match:
  expected=sum(int(x or 0) for x in match.groups()[:3])
  actual=sum(bool(x) for x in slots)
  if expected!=actual:
   audit.append(dict(source=source,mode=mode,action='excluded',reason=f'Label count {actual} differs from declared {expected}'));continue
 elif mode=='222' and family=='Auro-3D':
  mode='222'+('.1' if 'SW' in slots else '.0')
 else:
  audit.append(dict(source=source,mode=mode,action='excluded',reason='Unrecognized mode name'));continue
 if sum(bool(x) for x in slots)>16:
  audit.append(dict(source=source,mode=mode,action='excluded',reason='More than 16 independent PCM channels'));continue
 if len(set(x for x in slots if x))!=sum(bool(x) for x in slots):
  audit.append(dict(source=source,mode=mode,action='excluded',reason='Duplicate speaker labels in workbook row'));continue
 # 5.1.0 -> 5.1; unqualified four/six tops -> t. Retain middle (m), height (h), front (f).
 pretty=mode
 if match and match.group(3)=='0':pretty=f'{match.group(1)}.{match.group(2)}'
 elif match and match.group(3) in ['4','6','8','10'] and not match.group(4):pretty=mode+'t'
 name=('auro-' if family=='Auro-3D' else '')+pretty
 label=('Auro-style '+pretty+' PCM') if family=='Auro-3D' else pretty
 # The same named layout can have a different source-specific slot order. Expose
 # that recipe explicitly, rather than inventing an interchangeable alias.
 if any(p['name']==name for p in catalog):name=family.lower().replace('-','')+'-'+pretty
 add(name,label,slots,[family],source)
# Useful floor-only choices omitted by the PCM sheet are valid PCM Custom recipes.
floor=base.copy()
for i in range(10,16):floor[i]=''
add('9.1','9.1',floor,['PCM Custom'],'subset of validated 9.1.6t room')
# The physical A16 has four configurable PCM Custom banks. Each advertised
# custom profile corresponds to one bank, not an unlimited collection of rooms.
banks=json.loads((HERE/'custom-banks.json').read_text())['banks']
assert sorted(b['bank'] for b in banks)==[1,2,3,4]
for bank in banks:
 if not bank['slots']:
  catalog.append(dict(name='custom-'+str(bank['bank']),layout=bank['label'],
      description=f"Custom: PCM Custom #{bank['bank']} - Unassigned (spare)",
      advanced=True,custom=True,pulse_compatible=True,available=False,matching_room=None,
      channels=0,usb=[],speakers=[],positions=[],families=['PCM Custom Bank'],sources=['custom-banks.json'],
      room='Unassigned: configure this bank in custom-banks.json and on the A16 before use',bank=bank['bank']))
  continue
 assert len(bank['slots'])<=16
 assert len(set(x for x in bank['slots'] if x))==sum(bool(x) for x in bank['slots'])
 p=add('custom-'+str(bank['bank']),f"PCM Custom #{bank['bank']}: {bank['label']}",bank['slots'],
       ['PCM Custom Bank'],'custom-banks.json bank '+str(bank['bank']))
 assert p is not None and p['custom'] and p['pulse_compatible']
 p['room']=f"PCM Custom #{bank['bank']} with USB "+', '.join(str(i+1)+'='+label for i,label in enumerate(bank['slots']) if label)+'; other slots unused'
 p['bank']=bank['bank']
# Canonical IDs and unambiguous t aliases.
aliases.extend([dict(name='7.1.4t',target='7.1.4'),dict(name='9.1.6t',target='9.1.6')])
names={p['name'] for p in catalog};aliases=[a for a in aliases if a['name'] not in names]
alias_map={}
for a in aliases:alias_map.setdefault(a['name'],a['target'])
report=dict(source='smyth-tables.json',profiles=catalog,aliases=alias_map,audit=audit)
publish(HERE/'catalog.json',json.dumps(report,indent=2)+'\n')
lines=['/* Generated by layouts/generate.py; do not hand edit. SPDX-License-Identifier: MIT */','static const struct a16_profile a16_profiles[] = {']
for p in catalog:
 values=[p['name'],p['description'],'[ '+' '.join(p['positions'])+' ]',p['room'],' '.join(p['speakers'])]
 lines.append('    {'+', '.join(json.dumps(x) for x in values)+', '+str(p['advanced']).lower()+', '+str(p['custom']).lower()+', '+str(p['pulse_compatible']).lower()+', '+str(p['available']).lower()+', '+str(p['channels'])+', {'+', '.join(map(str,p['usb']))+'}, {'+', '.join('SPA_AUDIO_CHANNEL_'+c for c in p['positions'])+'}},')
lines+=['};','static const struct { const char *name, *target; } a16_profile_aliases[] = {']
lines.extend('    {'+json.dumps(k)+', '+json.dumps(v)+'},' for k,v in sorted(alias_map.items()));lines+=['};','']
publish(HERE.parent/'profiles_generated.inc','\n'.join(lines))
print(len(catalog),'profiles;',sum(p['pulse_compatible'] for p in catalog),'Pulse-compatible;',len(alias_map),'aliases;',len(audit),'audited source rows')
