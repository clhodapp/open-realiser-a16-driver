#!/usr/bin/env python3
"""Freeze factual layout rows from Smyth's configuration workbook; no Excel runtime needed."""
import argparse,hashlib,json,xml.etree.ElementTree as E,zipfile
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('workbook',type=Path);parser.add_argument('output',type=Path);args=parser.parse_args()
n={'m':'http://schemas.openxmlformats.org/spreadsheetml/2006/main'};rows=[];family=None
with zipfile.ZipFile(args.workbook) as z:
 strings=[''.join(x.itertext()) for x in E.fromstring(z.read('xl/sharedStrings.xml'))]
 for row in E.fromstring(z.read('xl/worksheets/sheet8.xml')).findall('m:sheetData/m:row',n):
  cells={}
  for c in row:
   v=c.find('m:v',n)
   if v is not None:cells[''.join(i for i in c.get('r') if i.isalpha())]=strings[int(v.text)] if c.get('t')=='s' else v.text
  if cells.get('B')=='Format':family=cells['A'];continue
  if family and cells.get('A','').isdigit() and 'B' in cells:
   limit=24 if family=='Atmos' else 16
   rows.append(dict(family=family,mode=cells['B'],code=int(cells['A']),row=int(row.get('r')),slots=[cells.get(chr(67+i),'').strip() for i in range(limit)]))
args.output.write_text(json.dumps(dict(source=args.workbook.name,sha256=hashlib.sha256(args.workbook.read_bytes()).hexdigest(),rows=rows),indent=2)+'\n')
print('Imported',len(rows),'rows')
